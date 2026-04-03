use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use async_trait::async_trait;
use butterflow_models::Error;
use butterflow_models::Result;

use crate::{OutputCallback, Runner};

/// Direct runner (runs commands directly on the host)
pub struct DirectRunner {
    /// When true, suppress real-time stdout/stderr printing
    quiet: bool,
}

impl DirectRunner {
    /// Create a new direct runner
    pub fn new() -> Self {
        Self { quiet: false }
    }

    /// Create a new direct runner with quiet mode
    pub fn with_quiet(quiet: bool) -> Self {
        Self { quiet }
    }

    /// Execute a command with streaming output
    async fn execute_with_streaming(
        &self,
        mut cmd: Command,
        output_callback: Option<OutputCallback>,
    ) -> Result<String> {
        #[cfg(unix)]
        {
            use std::os::unix::io::FromRawFd;

            let mut pipe_fds = [0i32; 2];
            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } == -1 {
                return Err(Error::Runtime("Failed to create output pipe".to_string()));
            }
            let read_fd = pipe_fds[0];
            let write_fd = pipe_fds[1];
            let stdout_file = unsafe { std::fs::File::from_raw_fd(write_fd) };
            let stderr_file = stdout_file
                .try_clone()
                .map_err(|e| Error::Runtime(format!("Failed to clone output pipe: {e}")))?;

            cmd.stdout(Stdio::from(stdout_file))
                .stderr(Stdio::from(stderr_file));

            let mut child = cmd
                .spawn()
                .map_err(|e| Error::Runtime(format!("Failed to spawn command: {e}")))?;
            drop(cmd);

            let quiet = self.quiet;
            let callback = output_callback;
            let reader_handle = std::thread::spawn(move || {
                let read_file = unsafe { std::fs::File::from_raw_fd(read_fd) };
                let reader = BufReader::new(read_file);
                let mut collected_output = String::new();
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            if let Some(callback) = &callback {
                                callback(format!("[stdio] {}", line));
                            }
                            if !quiet {
                                println!("{}", line);
                            }
                            collected_output.push_str(&line);
                            collected_output.push('\n');
                        }
                        Err(e) => {
                            if !quiet {
                                eprintln!("Error reading process output: {}", e);
                            }
                            break;
                        }
                    }
                }
                collected_output
            });

            let exit_status = tokio::task::spawn_blocking(move || {
                child
                    .wait()
                    .map_err(|e| Error::Runtime(format!("Failed to wait for command: {e}")))
            })
            .await
            .map_err(|e| Error::Runtime(format!("Failed to join wait task: {e}")))??;

            let combined_output = reader_handle
                .join()
                .map_err(|_| Error::Runtime("Failed to join output reader".to_string()))?;

            if !exit_status.success() {
                return Err(Error::Runtime(format!(
                    "Command failed with exit code {}: {}",
                    exit_status.code().unwrap_or(-1),
                    combined_output
                )));
            }

            return Ok(combined_output);
        }

        #[cfg(not(unix))]
        {
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

        let quiet = self.quiet;
        let stdout_callback = output_callback.clone();

        // Handle stdout in a separate task
        let stdout_handle = std::thread::spawn(move || {
            let mut collected_output = String::new();
            for line in stdout_reader.lines() {
                match line {
                    Ok(line) => {
                        if let Some(callback) = &stdout_callback {
                            callback(format!("[stdout] {}", line));
                        }
                        if !quiet {
                            println!("{}", line);
                        }
                        collected_output.push_str(&line);
                        collected_output.push('\n');
                    }
                    Err(e) => {
                        if !quiet {
                            eprintln!("Error reading stdout: {}", e);
                        }
                        break;
                    }
                }
            }
            collected_output
        });

        // Handle stderr in a separate task
        let stderr_callback = output_callback;
        let stderr_handle = std::thread::spawn(move || {
            let mut collected_output = String::new();
            for line in stderr_reader.lines() {
                match line {
                    Ok(line) => {
                        if let Some(callback) = &stderr_callback {
                            callback(format!("[stderr] {}", line));
                        }
                        if !quiet {
                            eprintln!("{}", line);
                        }
                        collected_output.push_str(&line);
                        collected_output.push('\n');
                    }
                    Err(e) => {
                        if !quiet {
                            eprintln!("Error reading stderr: {}", e);
                        }
                        break;
                    }
                }
            }
            collected_output
        });

        // Wait for the process to complete and collect outputs
        let exit_status = tokio::task::spawn_blocking(move || {
            child
                .wait()
                .map_err(|e| Error::Runtime(format!("Failed to wait for command: {e}")))
        })
        .await
        .map_err(|e| Error::Runtime(format!("Failed to join wait task: {e}")))??;

        let stdout_output = stdout_handle
            .join()
            .map_err(|_| Error::Runtime("Failed to join stdout reader".to_string()))?;
        let stderr_output = stderr_handle
            .join()
            .map_err(|_| Error::Runtime("Failed to join stderr reader".to_string()))?;

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
}

impl Default for DirectRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runner for DirectRunner {
    async fn run_command(
        &self,
        command: &str,
        env: &HashMap<String, String>,
        output_callback: Option<OutputCallback>,
    ) -> Result<String> {
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
            let result = self.execute_with_streaming(cmd, output_callback).await;

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
            self.execute_with_streaming(cmd, output_callback).await
        }
    }
}
