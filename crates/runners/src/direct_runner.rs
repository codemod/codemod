use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use async_trait::async_trait;
use tokio::task;

use butterflow_models::Error;
use butterflow_models::Result;

use crate::Runner;

/// Direct runner (runs commands directly on the host)
pub struct DirectRunner;

impl DirectRunner {
    /// Create a new direct runner
    pub fn new() -> Self {
        Self
    }

    /// Execute a command with streaming output
    async fn execute_with_streaming(&self, mut cmd: Command) -> Result<String> {
        // Configure the command to pipe stdout and stderr
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        // Spawn the command
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Runtime(format!("Failed to spawn command: {e}")))?;

        // Get handles to stdout and stderr
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Runtime("Failed to capture stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Runtime("Failed to capture stderr".to_string()))?;

        // Create readers
        let stdout_reader = BufReader::new(stdout);
        let stderr_reader = BufReader::new(stderr);

        // Handle stdout in a separate task
        let stdout_handle = task::spawn_blocking(move || {
            let mut collected_output = String::new();
            for line in stdout_reader.lines() {
                match line {
                    Ok(line) => {
                        // Print to stdout in real-time
                        println!("{}", line);
                        collected_output.push_str(&line);
                        collected_output.push('\n');
                    }
                    Err(e) => {
                        eprintln!("Error reading stdout: {}", e);
                        break;
                    }
                }
            }
            collected_output
        });

        // Handle stderr in a separate task
        let stderr_handle = task::spawn_blocking(move || {
            let mut collected_output = String::new();
            for line in stderr_reader.lines() {
                match line {
                    Ok(line) => {
                        // Print to stderr in real-time
                        eprintln!("{}", line);
                        collected_output.push_str(&line);
                        collected_output.push('\n');
                    }
                    Err(e) => {
                        eprintln!("Error reading stderr: {}", e);
                        break;
                    }
                }
            }
            collected_output
        });

        // Wait for the process to complete and collect outputs
        let (exit_status, stdout_output, stderr_output) = tokio::try_join!(
            async {
                child
                    .wait()
                    .map_err(|e| Error::Runtime(format!("Failed to wait for command: {e}")))
            },
            async {
                stdout_handle
                    .await
                    .map_err(|e| Error::Runtime(format!("Failed to read stdout: {e}")))
            },
            async {
                stderr_handle
                    .await
                    .map_err(|e| Error::Runtime(format!("Failed to read stderr: {e}")))
            }
        )?;

        if !exit_status.success() {
            return Err(Error::Runtime(format!(
                "Command failed with exit code {}: {}",
                exit_status.code().unwrap_or(-1),
                stderr_output
            )));
        }

        Ok(stdout_output)
    }
}

impl Default for DirectRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runner for DirectRunner {
    async fn run_command(&self, command: &str, env: &HashMap<String, String>) -> Result<String> {
        // Check if the command starts with a shebang line
        if command.starts_with("#!/") {
            // Create a temporary file for the script
            let temp_dir = std::env::temp_dir();
            let file_name = format!("butterflow-script-{}.sh", uuid::Uuid::new_v4());
            let script_path = temp_dir.join(file_name);

            // Write the script to the temporary file
            std::fs::write(&script_path, command).map_err(|e| {
                Error::Runtime(format!("Failed to write script to temporary file: {e}"))
            })?;

            // Make the script executable
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&script_path)
                    .map_err(|e| Error::Runtime(format!("Failed to get file permissions: {e}")))?
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&script_path, perms)
                    .map_err(|e| Error::Runtime(format!("Failed to set file permissions: {e}")))?;
            }

            // Create the command
            let mut cmd = Command::new(&script_path);

            // Add environment variables
            for (key, value) in env {
                cmd.env(key, value);
            }

            // Execute the command with streaming
            let result = self.execute_with_streaming(cmd).await;

            // Clean up the temporary file
            std::fs::remove_file(&script_path).ok();

            // Return the result
            result
        } else {
            // Determine the shell to use
            let shell = if cfg!(target_os = "windows") {
                "cmd"
            } else {
                "sh"
            };

            let shell_arg = if cfg!(target_os = "windows") {
                "/C"
            } else {
                "-c"
            };

            // Create the command
            let mut cmd = Command::new(shell);
            cmd.arg(shell_arg).arg(command);

            // Add environment variables
            for (key, value) in env {
                cmd.env(key, value);
            }

            // Execute the command with streaming
            self.execute_with_streaming(cmd).await
        }
    }
}
