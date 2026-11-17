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
    ) -> Result<CommandOutput> {
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
    ) -> Result<CommandOutput> {
        unreachable!("cmd scripts only run on Windows")
    }

    /// Run `cmd`, passing each line of its output to `output_callback`
    /// tagged `[stdout] ` or `[stderr] `, and keep both streams.
    async fn execute_with_streaming(
        &self,
        mut cmd: Command,
        output_callback: Option<OutputCallback>,
    ) -> Result<CommandOutput> {
        #[cfg(unix)]
        let (child, stdout, stderr) = {
            use std::os::unix::io::FromRawFd;

            // A command started on another thread while these pipes are
            // being set up would inherit them and keep them open: this
            // command's output would not end, and so the command would not
            // be done, until that one exits. The pipes are closed on exec,
            // and commands are started one at a time for the moment before
            // that is set.
            static STARTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());

            let pipe = || -> Result<(std::fs::File, std::fs::File)> {
                let mut fds = [0i32; 2];
                if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
                    return Err(Error::Runtime("Failed to create output pipe".to_string()));
                }
                for fd in fds {
                    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
                }
                // SAFETY: both descriptors were just opened and are owned here.
                Ok(unsafe {
                    (
                        std::fs::File::from_raw_fd(fds[0]),
                        std::fs::File::from_raw_fd(fds[1]),
                    )
                })
            };
            let (stdout_read, stdout_write) = pipe()?;
            let (stderr_read, stderr_write) = pipe()?;
            cmd.stdout(Stdio::from(stdout_write))
                .stderr(Stdio::from(stderr_write));
            let child = cmd
                .spawn()
                .map_err(|e| Error::Runtime(format!("Failed to spawn command: {e}")));
            // Dropping the command closes this side's write ends.
            drop(cmd);
            drop(starting);
            (child?, stdout_read, stderr_read)
        };
        #[cfg(not(unix))]
        let (child, stdout, stderr) = {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = cmd
                .spawn()
                .map_err(|e| Error::Runtime(format!("Failed to spawn command: {e}")))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| Error::Runtime("Failed to capture stdout".to_string()))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| Error::Runtime("Failed to capture stderr".to_string()))?;
            (child, stdout, stderr)
        };

        let quiet = self.quiet;
        let stdout_handle = {
            let callback = output_callback.clone();
            std::thread::spawn(move || collect(stdout, "stdout", callback, quiet))
        };
        let stderr_handle =
            std::thread::spawn(move || collect(stderr, "stderr", output_callback, quiet));

        // A stopped command's output is not waited for: something it
        // started may still hold the pipes open.
        let Some(exit_status) = self.wait(child).await? else {
            return Err(Error::Canceled);
        };
        let join = |handle: std::thread::JoinHandle<Captured>| {
            handle
                .join()
                .map_err(|_| Error::Runtime("Failed to join output reader".to_string()))
        };
        let stdout = join(stdout_handle)?;
        let stderr = join(stderr_handle)?;
        Ok(CommandOutput {
            stdout: stdout.text,
            stderr: stderr.text,
            exit_code: exit_status.code().unwrap_or(-1),
            cut: stdout.cut || stderr.cut,
        })
    }
}

/// What a command printed, and how it exited. A command that exits with an
/// error is not an error here; the caller decides.
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    /// `-1` when the command was ended by a signal.
    pub exit_code: i32,
    /// Whether either stream was longer than [`KEPT_OUTPUT`] and lost its start.
    pub cut: bool,
}

/// How much of each stream is kept: its end, where a command usually
/// reports its result.
pub const KEPT_OUTPUT: usize = 1024 * 1024;

struct Captured {
    text: String,
    cut: bool,
}

/// Read `stream` to its end, a line at a time, keeping its last
/// [`KEPT_OUTPUT`] bytes in whole lines.
fn collect(
    stream: impl std::io::Read,
    tag: &str,
    callback: Option<OutputCallback>,
    quiet: bool,
) -> Captured {
    let mut lines: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut kept = 0;
    let mut cut = false;
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if let Some(callback) = &callback {
            callback(format!("[{tag}] {line}"));
        }
        if !quiet {
            if tag == "stderr" {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
        kept += line.len() + 1;
        lines.push_back(line);
        while kept > KEPT_OUTPUT && lines.len() > 1 {
            kept -= lines.pop_front().map_or(0, |line| line.len() + 1);
            cut = true;
        }
        // A single line longer than the limit keeps its end, cut at a
        // character boundary.
        if kept > KEPT_OUTPUT {
            if let Some(line) = lines.back_mut() {
                let mut start = line.len() + 1 - KEPT_OUTPUT;
                while !line.is_char_boundary(start) {
                    start += 1;
                }
                line.drain(..start);
                kept = line.len() + 1;
                cut = true;
            }
        }
    }
    let mut text = String::with_capacity(kept);
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }
    Captured { text, cut }
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
    ) -> Result<CommandOutput> {
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
