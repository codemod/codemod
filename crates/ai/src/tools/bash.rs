//! Cross-platform shell execution tool

use async_trait::async_trait;
use coro_core::error::Result;
use coro_core::impl_tool_factory;
use coro_core::tools::utils::maybe_truncate;
use coro_core::tools::{Tool, ToolCall, ToolExample, ToolResult};
use serde_json::json;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout, Duration};

/// Warning information for potentially dangerous commands
#[derive(Debug)]
struct CommandWarning {
    risk: String,
    alternatives: String,
}

/// Check if a Windows command might be dangerous (cause timeouts or excessive output)
fn check_windows_command_safety(command: &str) -> Option<CommandWarning> {
    let cmd_lower = command.to_lowercase();

    // Check for recursive dir commands that might cause timeouts
    if cmd_lower.contains("dir") && cmd_lower.contains("/s") {
        // Allow specific file type searches
        if cmd_lower.contains("*.") {
            return None; // File type specific searches are usually safe
        }

        return Some(CommandWarning {
            risk: "Recursive directory listing can cause timeouts on large projects".to_string(),
            alternatives: "• Use 'dir' first to see the current directory structure\n\
                          • Use 'dir /ad' to see only subdirectories\n\
                          • Use 'dir *.rs /s' to search for specific file types\n\
                          • Use 'dir /ad | findstr /v /i \"target node_modules .git\"' to exclude large folders".to_string(),
        });
    }

    // Check for other potentially problematic recursive operations
    if (cmd_lower.contains("tree") && !cmd_lower.contains("/f"))
        || (cmd_lower.contains("forfiles") && cmd_lower.contains("/s"))
    {
        return Some(CommandWarning {
            risk: "Recursive operations can be slow on large directory structures".to_string(),
            alternatives:
                "• Start with non-recursive commands to understand the structure\n\
                          • Use specific paths instead of full recursive searches\n\
                          • Consider excluding large directories like target/, node_modules/, .git/"
                    .to_string(),
        });
    }

    None
}

/// Shell configuration for different operating systems
#[derive(Debug, Clone)]
struct ShellConfig {
    command: String,
    args: Vec<String>,
    sentinel: String,
    is_windows: bool,
}

impl ShellConfig {
    fn new() -> Self {
        if cfg!(target_os = "windows") {
            Self {
                command: "cmd.exe".to_string(),
                args: vec![
                    "/Q".to_string(),
                    "/K".to_string(),
                    "chcp 65001 >nul".to_string(),
                ],
                sentinel: ",,,,shell-command-exit-__ERROR_CODE__-banner,,,,".to_string(),
                is_windows: true,
            }
        } else {
            Self {
                command: "/bin/bash".to_string(),
                args: vec![],
                sentinel: ",,,,shell-command-exit-__ERROR_CODE__-banner,,,,".to_string(),
                is_windows: false,
            }
        }
    }
}

/// A session of a cross-platform shell
struct ShellSession {
    process: Option<Child>,
    started: bool,
    timed_out: bool,
    config: ShellConfig,
    output_delay: Duration,
    timeout: Duration,
}

impl ShellSession {
    fn new() -> Self {
        Self {
            process: None,
            started: false,
            timed_out: false,
            config: ShellConfig::new(),
            output_delay: Duration::from_millis(200),
            timeout: Duration::from_secs(120),
        }
    }

    async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }

        let mut cmd = Command::new(&self.config.command);

        // Add shell-specific arguments
        if !self.config.args.is_empty() {
            cmd.args(&self.config.args);
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // On Unix-like systems, set process group
        #[cfg(unix)]
        {
            #[allow(unused_imports)]
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        self.process = Some(cmd.spawn()?);
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) {
        if !self.started {
            return;
        }

        if let Some(mut process) = self.process.take() {
            if process.try_wait().unwrap_or(None).is_none() {
                std::mem::drop(process.kill());
            }
        }
        self.started = false;
    }

    async fn run(&mut self, command: &str) -> Result<(i32, String, String)> {
        if !self.started || self.process.is_none() {
            return Err("Session has not started.".into());
        }

        if self.timed_out {
            return Err(format!(
                "timed out: shell has not returned in {} seconds and must be restarted",
                self.timeout.as_secs()
            )
            .into());
        }

        let process = self.process.as_mut().unwrap();

        // Check if process is still alive
        if let Ok(Some(status)) = process.try_wait() {
            return Err(format!(
                "shell has exited with returncode {}. tool must be restarted.",
                status.code().unwrap_or(-1)
            )
            .into());
        }

        let _error_code = 0;
        let (sentinel_before, sentinel_after) = self
            .config
            .sentinel
            .split_once("__ERROR_CODE__")
            .ok_or("Invalid sentinel format")?;

        // Build command based on shell type
        let full_command = if self.config.is_windows {
            // CMD syntax
            let errcode_retriever = "%ERRORLEVEL%";
            format!(
                "{} & echo {}\r\n",
                command,
                self.config
                    .sentinel
                    .replace("__ERROR_CODE__", errcode_retriever)
            )
        } else {
            // Bash syntax
            let errcode_retriever = "$?";
            let command_sep = ";";
            format!(
                "(\n{}\n){} echo {}\n",
                command,
                command_sep,
                self.config
                    .sentinel
                    .replace("__ERROR_CODE__", errcode_retriever)
            )
        };

        // Send command to the process
        if let Some(stdin) = process.stdin.as_mut() {
            stdin.write_all(full_command.as_bytes()).await?;
            stdin.flush().await?;
        } else {
            return Err("No stdin available".into());
        }

        // Read output from the process until sentinel is found
        let result = timeout(self.timeout, async {
            let mut output = String::new();
            let mut error_code = 0;

            if let Some(stdout) = process.stdout.as_mut() {
                let mut reader = BufReader::new(stdout);
                let mut buffer = Vec::new();

                loop {
                    sleep(self.output_delay).await;

                    // Try to read available data
                    match reader.read_until(b'\n', &mut buffer).await {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buffer);
                            output.push_str(&line);
                            buffer.clear();

                            if output.contains(sentinel_before) {
                                // Extract the sentinel and error code
                                if let Some(pos) = output.rfind(sentinel_before) {
                                    let content = output[..pos].to_string();
                                    let rest = &output[pos..];

                                    // Extract error code from the sentinel
                                    if let Some(code_start) = rest.find(sentinel_before) {
                                        let code_part = &rest[code_start + sentinel_before.len()..];
                                        if let Some(code_end) = code_part.find(sentinel_after) {
                                            let code_str = &code_part[..code_end];
                                            error_code = code_str.trim().parse().unwrap_or(-1);
                                        }
                                    }

                                    output = content;
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }

            Ok::<(i32, String, String), coro_core::error::Error>((
                error_code,
                output,
                String::new(),
            ))
        })
        .await;

        match result {
            Ok(Ok((exit_code, stdout, stderr))) => {
                let stdout_clean = if stdout.ends_with('\n') {
                    stdout.trim_end_matches('\n').to_string()
                } else {
                    stdout
                };
                Ok((exit_code, stdout_clean, stderr))
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                self.timed_out = true;
                Err(format!(
                    "timed out: bash has not returned in {} seconds and must be restarted",
                    self.timeout.as_secs()
                )
                .into())
            }
        }
    }
}

/// Tool for executing shell commands with session management
pub struct BashTool {
    session: Arc<Mutex<Option<ShellSession>>>,
}

impl BashTool {
    /// Create a new shell tool
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        if cfg!(target_os = "windows") {
            "Run commands in Windows Command Prompt (cmd.exe)\n\
             * When invoking this tool, the contents of the \"command\" parameter does NOT need to be XML-escaped.\n\
             * State is persistent across command calls and discussions with the user.\n\
             * Uses Windows Command Prompt with UTF-8 encoding for proper Chinese character support.\n\
             * Supports both Windows built-in commands and external programs.\n\
             * IMPORTANT: Avoid recursive operations like 'dir /s' on large directories (target/, node_modules/, .git/).\n\
             * Start with simple 'dir' to see directory structure before using recursive commands.\n\
             * For large projects, use specific paths or exclude large folders to prevent timeouts.\n\
             * Please avoid commands that may produce a very large amount of output.\n\
             * Please run long lived commands in the background when appropriate."
        } else {
            "Run commands in a bash shell\n\
             * When invoking this tool, the contents of the \"command\" parameter does NOT need to be XML-escaped.\n\
         * You have access to a mirror of common linux and python packages via apt and pip.\n\
             * State is persistent across command calls and discussions with the user.\n\
         * To inspect a particular line range of a file, e.g. lines 10-25, try 'sed -n 10,25p /path/to/the/file'.\n\
             * Please avoid commands that may produce a very large amount of output.\n\
         * Please run long lived commands in the background, e.g. 'sleep 10 &' or start a server in the background."
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let command_description = if cfg!(target_os = "windows") {
            "The Windows command to run (cmd.exe syntax)."
        } else {
            "The bash command to run."
        };

        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": command_description
                },
                "restart": {
                    "type": "boolean",
                    "description": "Set to true to restart the shell session."
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult> {
        let restart: bool = call.get_parameter_or("restart", false);

        if restart {
            // Handle restart without holding lock across await
            {
                let mut session_guard = self.session.lock().await;
                if let Some(ref mut session) = *session_guard {
                    session.stop();
                }
                *session_guard = Some(ShellSession::new());
            }

            // Start the new session
            {
                let mut session_guard = self.session.lock().await;
                if let Some(ref mut session) = *session_guard {
                    session.start().await?;
                }
            }

            return Ok(ToolResult::success(
                &call.id,
                &"tool has been restarted.".to_string(),
            ));
        }

        let command: String = call.get_parameter("command")?;

        // Windows-specific safety check for potentially dangerous recursive commands
        if cfg!(target_os = "windows") {
            if let Some(warning) = check_windows_command_safety(&command) {
                return Ok(ToolResult::error(
                    &call.id,
                    &format!(
                        "⚠️  Potentially dangerous command detected: {}\n\nSafer alternatives:\n{}",
                        warning.risk, warning.alternatives
                    ),
                ));
            }
        }

        // Ensure session exists and is started
        let needs_start = {
            let mut session_guard = self.session.lock().await;
            if session_guard.is_none() {
                *session_guard = Some(ShellSession::new());
                true
            } else if let Some(ref session) = *session_guard {
                !session.started
            } else {
                false
            }
        };

        if needs_start {
            let mut session_guard = self.session.lock().await;
            if let Some(ref mut session) = *session_guard {
                session.start().await?;
            }
        }

        // Execute command
        let result = {
            let mut session_guard = self.session.lock().await;
            if let Some(ref mut session) = *session_guard {
                session.run(&command).await
            } else {
                return Err("No session available".into());
            }
        };

        match result {
            Ok((exit_code, stdout, stderr)) => {
                let mut output = String::new();

                if !stdout.is_empty() {
                    output.push_str(&maybe_truncate(&stdout, None));
                }

                if !stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&maybe_truncate(&stderr, None));
                }

                if output.is_empty() {
                    output = format!("Command completed with exit code: {}", exit_code);
                }

                Ok(ToolResult::success(&call.id, &output).with_data(json!({
                    "exit_code": exit_code,
                    "stdout": stdout,
                    "stderr": stderr
                })))
            }
            Err(e) => Ok(ToolResult::error(
                &call.id,
                &format!("Error running shell command: {}", e),
            )),
        }
    }

    fn requires_confirmation(&self) -> bool {
        true // Bash commands can be dangerous
    }

    fn examples(&self) -> Vec<ToolExample> {
        if cfg!(target_os = "windows") {
            vec![
                ToolExample {
                    description: "List files in current directory (safe, fast)".to_string(),
                    parameters: json!({"command": "dir"}),
                    expected_result: "Directory listing with file details".to_string(),
                },
                ToolExample {
                    description: "List only directories to understand structure".to_string(),
                    parameters: json!({"command": "dir /ad"}),
                    expected_result: "Directory listing showing only subdirectories".to_string(),
                },
                ToolExample {
                    description: "Search specific file types (safer than full recursive)"
                        .to_string(),
                    parameters: json!({"command": "dir *.rs /s"}),
                    expected_result: "Recursive listing of .rs files only".to_string(),
                },
                ToolExample {
                    description: "Check disk space and system info".to_string(),
                    parameters: json!({"command": "dir | findstr bytes"}),
                    expected_result: "Summary line showing total files and bytes".to_string(),
                },
                ToolExample {
                    description: "Safe way to explore large projects".to_string(),
                    parameters: json!({"command": "dir /ad | findstr /v /i \"target node_modules .git\""}),
                    expected_result: "Directory listing excluding common large folders".to_string(),
                },
            ]
        } else {
            vec![
                ToolExample {
                    description: "List files in current directory (Unix)".to_string(),
                    parameters: json!({"command": "ls -la"}),
                    expected_result: "Directory listing with file details".to_string(),
                },
                ToolExample {
                    description: "Check Bash version".to_string(),
                    parameters: json!({"command": "bash --version"}),
                    expected_result: "Bash version information".to_string(),
                },
                ToolExample {
                    description: "Restart shell session".to_string(),
                    parameters: json!({"command": "echo 'restarting'", "restart": true}),
                    expected_result: "Session restarted message".to_string(),
                },
                ToolExample {
                    description: "Run a command with persistent state".to_string(),
                    parameters: json!({"command": "export MY_VAR=hello && echo $MY_VAR"}),
                    expected_result: "Variable set and echoed in persistent session".to_string(),
                },
            ]
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

impl_tool_factory!(
    BashToolFactory,
    BashTool,
    "bash",
    if cfg!(target_os = "windows") {
        "Execute Windows commands using cmd.exe"
    } else {
        "Execute bash commands on Unix-like systems"
    }
);
