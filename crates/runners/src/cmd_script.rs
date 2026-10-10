//! Windows `run` steps are written to a temporary batch file and run by cmd,
//! the way `sh -c` runs them on Unix.
//!
//! Two cmd behaviors need handling around that:
//!
//! - A batch file that runs another batch file by name hands control to it
//!   and never returns, so with `npm install` (npm is `npm.cmd`) followed by
//!   another line, the step would stop after npm. A command that resolves to
//!   a `.bat` or `.cmd` file is therefore run with `call`.
//! - cmd reads a batch file in the console code page, so the script is
//!   written as UTF-8 and switches the code page to 65001 first. The console
//!   is shared with this process, so its code pages are restored once no
//!   script is running.

// Only the Windows runner uses these outside tests.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// cmd built-ins, which cmd runs instead of a file of the same name.
const BUILTINS: &[&str] = &[
    "assoc", "break", "call", "cd", "chdir", "cls", "color", "copy", "date", "del", "dir", "echo",
    "endlocal", "erase", "exit", "for", "ftype", "goto", "if", "md", "mkdir", "mklink", "move",
    "path", "pause", "popd", "prompt", "pushd", "rd", "rem", "ren", "rename", "rmdir", "set",
    "setlocal", "shift", "start", "time", "title", "type", "ver", "verify", "vol",
];

const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// A temporary batch file, removed when dropped, including when the step's
/// future is dropped before the command finishes.
pub(crate) struct TempScript {
    path: PathBuf,
}

impl TempScript {
    pub(crate) fn create(contents: &str) -> std::io::Result<Self> {
        let path =
            std::env::temp_dir().join(format!("butterflow-script-{}.cmd", uuid::Uuid::new_v4()));
        std::fs::write(&path, contents)?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// Builds the batch file for a `run` command, resolving commands against the
/// step's `PATH` and `PATHEXT`, and the current directory, which cmd searches
/// first.
pub(crate) fn script_for(command: &str, env: &HashMap<String, String>) -> String {
    let mut dirs: Vec<PathBuf> = std::env::current_dir().into_iter().collect();
    if let Some(path) = env_value(env, "PATH") {
        dirs.extend(std::env::split_paths(path));
    }
    let pathext: Vec<String> = env_value(env, "PATHEXT")
        .unwrap_or(DEFAULT_PATHEXT)
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(str::to_string)
        .collect();
    build_script(command, &dirs, &pathext)
}

fn env_value<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

pub(crate) fn build_script(command: &str, dirs: &[PathBuf], pathext: &[String]) -> String {
    let mut script = String::from("@echo off\r\nchcp 65001 >nul\r\n");
    let mut continued = false;
    for line in command.lines() {
        if continued {
            script.push_str(line);
        } else {
            script.push_str(&call_batch_commands(line, dirs, pathext));
        }
        script.push_str("\r\n");
        continued = line.trim_end().ends_with('^');
    }
    script
}

/// Prefixes `call` to each command of the line that resolves to a batch
/// file: the first command, and each one after `&`, `&&` or `||`. Commands
/// inside `if` or `for`, or after `(` mid-line, are left as written.
fn call_batch_commands(line: &str, dirs: &[PathBuf], pathext: &[String]) -> String {
    let bytes = line.as_bytes();
    let mut starts = vec![0];
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b'^' if !in_quotes => i += 1,
            op @ (b'&' | b'|') if !in_quotes => {
                let doubled = bytes.get(i + 1) == Some(&op);
                if doubled {
                    i += 1;
                }
                // A single `|` is a pipe: each side runs in its own cmd.
                if op == b'&' || doubled {
                    starts.push(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }

    let mut out = String::with_capacity(line.len() + 5 * starts.len());
    let mut copied = 0;
    for start in starts {
        let mut pos = start;
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'@' | b'(') {
            pos += 1;
        }
        if is_batch_command(command_name(&line[pos..]), dirs, pathext) {
            out.push_str(&line[copied..pos]);
            out.push_str("call ");
            copied = pos;
        }
    }
    out.push_str(&line[copied..]);
    out
}

fn command_name(rest: &str) -> &str {
    if let Some(quoted) = rest.strip_prefix('"') {
        return quoted.split('"').next().unwrap_or("");
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || "&|<>()^\"".contains(c))
        .unwrap_or(rest.len());
    &rest[..end]
}

fn is_batch_command(name: &str, dirs: &[PathBuf], pathext: &[String]) -> bool {
    if name.is_empty()
        || name.contains(['%', '!'])
        || BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
    {
        return false;
    }

    let bases: Vec<PathBuf> = if name.contains(['\\', '/', ':']) {
        vec![PathBuf::from(name)]
    } else {
        dirs.iter().map(|dir| dir.join(name)).collect()
    };

    let has_known_ext = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            pathext
                .iter()
                .any(|known| known.trim_start_matches('.').eq_ignore_ascii_case(ext))
        });

    for base in bases {
        if has_known_ext && base.is_file() {
            return is_batch_file(&base);
        }
        for ext in pathext {
            let mut candidate = base.clone().into_os_string();
            candidate.push(ext);
            let candidate = PathBuf::from(candidate);
            if candidate.is_file() {
                return is_batch_file(&candidate);
            }
        }
    }
    false
}

fn is_batch_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"))
}

#[cfg(windows)]
mod console {
    use std::sync::Mutex;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleCP() -> u32;
        fn GetConsoleOutputCP() -> u32;
        fn SetConsoleCP(code_page: u32) -> i32;
        fn SetConsoleOutputCP(code_page: u32) -> i32;
    }

    /// Running scripts, and the code pages from before the first of them.
    static ACTIVE: Mutex<(usize, Option<(u32, u32)>)> = Mutex::new((0, None));

    /// Restores the console code pages when the last running script ends.
    pub(crate) struct CodePageGuard;

    impl CodePageGuard {
        pub(crate) fn acquire() -> Self {
            let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            if active.0 == 0 {
                // SAFETY: plain Win32 calls without pointers; they return 0
                // when the process has no console.
                active.1 = Some(unsafe { (GetConsoleCP(), GetConsoleOutputCP()) });
            }
            active.0 += 1;
            CodePageGuard
        }
    }

    impl Drop for CodePageGuard {
        fn drop(&mut self) {
            let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            active.0 -= 1;
            if active.0 == 0 {
                if let Some((input, output)) = active.1.take() {
                    // SAFETY: as above; 0 means there was no console.
                    unsafe {
                        if input != 0 {
                            SetConsoleCP(input);
                        }
                        if output != 0 {
                            SetConsoleOutputCP(output);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
pub(crate) use console::CodePageGuard;

#[cfg(test)]
mod tests {
    use super::*;

    struct Tools(PathBuf);

    impl Tools {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("butterflow-tools-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            for file in ["faketool.cmd", "oldtool.bat", "realtool.exe"] {
                std::fs::write(dir.join(file), "").unwrap();
            }
            Self(dir)
        }

        fn script(&self, command: &str) -> Vec<String> {
            let pathext = [".com", ".exe", ".bat", ".cmd"].map(String::from);
            build_script(command, std::slice::from_ref(&self.0), &pathext)
                .split("\r\n")
                .skip(2)
                .filter(|line| !line.is_empty())
                .map(String::from)
                .collect()
        }
    }

    impl Drop for Tools {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn starts_by_switching_to_utf8() {
        let tools = Tools::new();
        let pathext = [".exe".to_string()];
        assert!(
            build_script("echo x", std::slice::from_ref(&tools.0), &pathext)
                .starts_with("@echo off\r\nchcp 65001 >nul\r\necho x\r\n")
        );
    }

    #[test]
    fn calls_commands_that_resolve_to_batch_files() {
        let tools = Tools::new();
        assert_eq!(
            tools.script("faketool install\noldtool\n  @faketool x\n(faketool y)\nfaketool.cmd z"),
            [
                "call faketool install",
                "call oldtool",
                "  @call faketool x",
                "(call faketool y)",
                "call faketool.cmd z",
            ]
        );
    }

    #[test]
    fn calls_each_chained_batch_command() {
        let tools = Tools::new();
        assert_eq!(
            tools.script("faketool a && faketool b || realtool c & faketool d | faketool e"),
            ["call faketool a && call faketool b || realtool c & call faketool d | faketool e"]
        );
    }

    #[test]
    fn leaves_everything_else_as_written() {
        let tools = Tools::new();
        let lines = [
            "realtool x",
            "echo faketool",
            "call faketool",
            "echo \"a & faketool\"",
            "echo a ^& faketool",
            "%TOOL% x",
            "unknown x",
            "if exist x faketool",
        ];
        assert_eq!(tools.script(&lines.join("\n")), lines);
    }

    #[test]
    fn leaves_continuation_lines_alone() {
        let tools = Tools::new();
        assert_eq!(
            tools.script("echo a ^\nfaketool\nfaketool"),
            ["echo a ^", "faketool", "call faketool"]
        );
    }
}
