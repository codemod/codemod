//! Asking an installed agent what its version supports, for flags that
//! changed between versions. Answers are cached per executable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    let text = run_help(executable, help_args).unwrap_or_default();
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, text.clone());
    text
}

fn run_help(executable: &Path, help_args: &[&str]) -> Option<String> {
    let mut child = Command::new(executable)
        .args(help_args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    Some(format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}
