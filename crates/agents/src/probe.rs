//! Asking an installed agent what its version supports, for flags that
//! changed between versions. Answers are cached per executable.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::run::spawn_group;

type Key = (PathBuf, Vec<String>);

/// Whether `executable <help_args>` prints `flag`.
pub(crate) fn supports_flag(executable: &Path, help_args: &[&str], flag: &str) -> bool {
    help_text(executable, help_args).contains(flag)
}

/// What `executable <help_args>` prints (stdout and stderr), or nothing when
/// it does not answer within a few seconds.
pub(crate) fn help_text(executable: &Path, help_args: &[&str]) -> String {
    static CACHE: OnceLock<Mutex<HashMap<Key, String>>> = OnceLock::new();
    let key = (
        executable.to_path_buf(),
        help_args.iter().map(|arg| arg.to_string()).collect(),
    );
    let cache = CACHE.get_or_init(Default::default);
    if let Some(text) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return text.clone();
    }
    let text = run_help(executable, help_args, Duration::from_secs(10)).unwrap_or_default();
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, text.clone());
    text
}

fn run_help(executable: &Path, help_args: &[&str], timeout: Duration) -> Option<String> {
    let mut command = Command::new(executable);
    command
        .args(help_args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, group) = spawn_group(&mut command).ok()?;
    let deadline = Instant::now() + timeout;
    let stdout = read_output(child.stdout.take().expect("piped stdout"));
    let stderr = read_output(child.stderr.take().expect("piped stderr"));
    let result = (|| {
        let remaining = || deadline.saturating_duration_since(Instant::now());
        let stdout = stdout.recv_timeout(remaining()).ok()?.ok()?;
        let stderr = stderr.recv_timeout(remaining()).ok()?.ok()?;
        loop {
            if child.try_wait().ok()?.is_some() {
                return Some(format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                ));
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            std::thread::sleep(remaining.min(Duration::from_millis(50)));
        }
    })();
    // The child may have exited while a descendant still owns a pipe.
    group.stop(&mut child);
    result
}

fn read_output(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let result = pipe.read_to_end(&mut output).map(|_| output);
        let _ = sender.send(result);
    });
    receiver
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_help_deadline_applies_to_inherited_output_pipes() {
        let script = "printf '%s\\n' --supported; sleep 7 & exit 0";
        let start = Instant::now();
        assert!(run_help(
            Path::new("/bin/sh"),
            &["-c", script],
            Duration::from_millis(200)
        )
        .is_none());
        assert!(start.elapsed() < Duration::from_secs(6));
    }
}
