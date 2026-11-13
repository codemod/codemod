use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use butterflow_models::Error;
use butterflow_models::Result;

use crate::{OutputCallback, Runner};

/// Direct runner (runs commands directly on the host)
pub struct DirectRunner {
    /// When true, suppress real-time stdout/stderr printing
    quiet: bool,
    /// Set to stop a command that is running.
    cancel: Option<Arc<AtomicBool>>,
}

/// How often a running command checks whether it was asked to stop.
const CANCEL_POLL: Duration = Duration::from_millis(50);
/// How long a stopped command gets to exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);

impl DirectRunner {
    /// Create a new direct runner
    pub fn new() -> Self {
        Self {
            quiet: false,
            cancel: None,
        }
    }

    /// Create a new direct runner with quiet mode
    pub fn with_quiet(quiet: bool) -> Self {
        Self {
            quiet,
            cancel: None,
        }
    }

    /// Stop the running command, and everything it started, when `cancel`
    /// is set; `run_command` then fails with [`Error::Canceled`].
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Wait for the command, stopping it if the runner is canceled. `None`
    /// when it was stopped.
    async fn wait(&self, mut child: Child) -> Result<Option<ExitStatus>> {
        let cancel = self.cancel.clone();
        tokio::task::spawn_blocking(move || loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(Some(status)),
                Ok(None) => {}
                Err(e) => return Err(Error::Runtime(format!("Failed to wait for command: {e}"))),
            }
            if cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) {
                stop_tree(&mut child);
                return Ok(None);
            }
            std::thread::sleep(CANCEL_POLL);
        })
        .await
        .map_err(|e| Error::Runtime(format!("Failed to join wait task: {e}")))?
    }

    /// Run a step on Windows. `cmd /C` runs only the first line of its
    /// argument, and Rust's backslash escaping of that argument is not
    /// understood by cmd, so the step is written to a batch file instead
    /// (see `cmd_script`).
    #[cfg(windows)]
    async fn run_cmd_script(
        &self,
        command: &str,
        env: &HashMap<String, String>,
        output_callback: Option<OutputCallback>,
    ) -> Result<String> {
        use std::os::windows::process::CommandExt;

        let script =
            crate::cmd_script::TempScript::create(&crate::cmd_script::script_for(command, env))
                .map_err(|e| {
                    Error::Runtime(format!("Failed to write script to temporary file: {e}"))
                })?;
        let _code_page = crate::cmd_script::CodePageGuard::acquire();

        // With /S, cmd drops only the outer quotes and keeps the quoted path
        // whole, so a temporary directory containing `&` or `^` still works.
        let mut cmd = Command::new("cmd");
        cmd.raw_arg(format!("/S /C \"\"{}\"\"", script.path().display()));

        cmd.env_clear();
        for (key, value) in env {
            cmd.env(key, value);
        }

        self.execute_with_streaming(cmd, output_callback).await
    }

    #[cfg(not(windows))]
    async fn run_cmd_script(
        &self,
        _command: &str,
        _env: &HashMap<String, String>,
        _output_callback: Option<OutputCallback>,
    ) -> Result<String> {
        unreachable!("cmd scripts only run on Windows")
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

            let child = cmd
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

            // A stopped command's output is not waited for: something it
            // started may still hold the pipe open.
            let Some(exit_status) = self.wait(child).await? else {
                return Err(Error::Canceled);
            };

            let combined_output = reader_handle
                .join()
                .map_err(|_| Error::Runtime("Failed to join output reader".to_string()))?;

            if !exit_status.success() {
                return Err(Error::ShellCommandFailed {
                    exit_code: exit_status.code().unwrap_or(-1),
                    output: combined_output,
                });
            }

            Ok(combined_output)
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
            let Some(exit_status) = self.wait(child).await? else {
                return Err(Error::Canceled);
            };

            let stdout_output = stdout_handle
                .join()
                .map_err(|_| Error::Runtime("Failed to join stdout reader".to_string()))?;
            let stderr_output = stderr_handle
                .join()
                .map_err(|_| Error::Runtime("Failed to join stderr reader".to_string()))?;

            if !exit_status.success() {
                return Err(Error::ShellCommandFailed {
                    exit_code: exit_status.code().unwrap_or(-1),
                    output: stderr_output,
                });
            }

            Ok(stdout_output)
        }
    }
}

/// Stop a command and every process it started: ask them to exit, then kill
/// what is left after a grace period.
fn stop_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let tree = process_tree(child.id());
        let signal_all = |signal: libc::c_int| {
            for pid in &tree {
                // SAFETY: signalling processes by id; one that is gone is an error we ignore.
                unsafe { libc::kill(*pid as libc::pid_t, signal) };
            }
        };
        signal_all(libc::SIGTERM);
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(CANCEL_POLL);
        }
        signal_all(libc::SIGKILL);
        let _ = child.wait();
    }
    #[cfg(not(unix))]
    {
        // /T takes the processes the command started with it.
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && !matches!(child.try_wait(), Ok(Some(_))) {
            std::thread::sleep(CANCEL_POLL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// `root` and its descendants, parents before children.
#[cfg(unix)]
fn process_tree(root: u32) -> Vec<u32> {
    let listing = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in listing.lines() {
        let mut fields = line.split_whitespace().map(str::parse::<u32>);
        if let (Some(Ok(pid)), Some(Ok(parent))) = (fields.next(), fields.next()) {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut tree = vec![root];
    let mut next = 0;
    while next < tree.len() {
        if let Some(found) = children.get(&tree[next]) {
            tree.extend(found);
        }
        next += 1;
    }
    tree
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
            cmd.env_clear();
            for (key, value) in env {
                cmd.env(key, value);
            }

            // Execute the command with streaming
            let result = self.execute_with_streaming(cmd, output_callback).await;

            // Clean up the temporary file
            std::fs::remove_file(&script_path).ok();

            // Return the result
            result
        } else if cfg!(windows) {
            self.run_cmd_script(command, env, output_callback).await
        } else {
            // Create the command
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(command);

            // Add environment variables
            cmd.env_clear();
            for (key, value) in env {
                cmd.env(key, value);
            }

            // Execute the command with streaming
            self.execute_with_streaming(cmd, output_callback).await
        }
    }
}
