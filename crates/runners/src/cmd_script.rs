//! Windows `run` steps are written to a temporary batch file and run by cmd,
//! the way `sh -c` runs them on Unix.
//!
//! Two cmd behaviors need handling around that:
//!
//! - A batch file that runs another batch file by name hands control to it
//!   and never returns, so with `npm install` (npm is `npm.cmd`) followed by
//!   another line, the step would stop after npm. A command that resolves to
//!   a `.bat` or `.cmd` file is therefore run with `call`, and so is one
//!   that is not found. Commands are looked up from the directory the step
//!   has changed to with `cd`, `chdir` or `pushd`. After a change the lookup
//!   cannot follow (`popd`, a `PATH` change, a directory named by a
//!   variable), every command that is not a built-in is run with `call`.
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
    let mut dirs = vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))];
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

/// Builds the batch file for `command`. `dirs` starts with the directory the
/// step runs in, followed by the `PATH` directories.
pub(crate) fn build_script(command: &str, dirs: &[PathBuf], pathext: &[String]) -> String {
    let mut script = String::from("@echo off\r\nchcp 65001 >nul\r\n");
    let mut lookup = Lookup {
        dirs: dirs.to_vec(),
        pathext,
        unknown: dirs.is_empty(),
    };
    let mut lines: Vec<&str> = Vec::new();
    for line in command.lines() {
        lines.push(line);
        // cmd joins a line that ends in an unescaped `^` with the next one,
        // so the commands are found in the joined text.
        if !ends_with_continuation(&lines.concat()) {
            push_command(&mut script, &lines, &mut lookup);
            lines.clear();
        }
    }
    if !lines.is_empty() {
        push_command(&mut script, &lines, &mut lookup);
    }
    script
}

/// Where the script's commands are found. It follows the step's `cd`s.
struct Lookup<'a> {
    /// The current directory, which cmd searches first, then `PATH`.
    dirs: Vec<PathBuf>,
    pathext: &'a [String],
    /// Set once the step changes the directory or `PATH` in a way this
    /// lookup does not follow. A later command can then resolve to any
    /// file, so every one that is not a built-in is run with `call`.
    unknown: bool,
}

impl Lookup<'_> {
    /// Whether a command is run with `call`: it resolves to a batch file, or
    /// it is not found. Built-ins, labels and commands named by a variable
    /// are not.
    fn needs_call(&self, name: &str) -> bool {
        if name.is_empty()
            || name.starts_with(':')
            || name.contains(['%', '!'])
            || BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
        {
            return false;
        }
        if self.unknown {
            return true;
        }

        let bases: Vec<PathBuf> = if name.contains(['\\', '/', ':']) {
            vec![self.dirs[0].join(name)]
        } else {
            self.dirs.iter().map(|dir| dir.join(name)).collect()
        };

        let has_known_ext = Path::new(name)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                self.pathext
                    .iter()
                    .any(|known| known.trim_start_matches('.').eq_ignore_ascii_case(ext))
            });

        for base in bases {
            if has_known_ext && base.is_file() {
                return is_batch_file(&base);
            }
            for ext in self.pathext {
                let mut candidate = base.clone().into_os_string();
                candidate.push(ext);
                let candidate = PathBuf::from(candidate);
                if candidate.is_file() {
                    return is_batch_file(&candidate);
                }
            }
        }
        true
    }

    /// Follows a command that changes the directory or `PATH`. `args` is the
    /// text after the command name, up to the next command. A conditional
    /// command (after `&&` or `||`) may not run.
    fn follow(&mut self, name: &str, args: &str, conditional: bool) {
        let is = |builtin: &str| name.eq_ignore_ascii_case(builtin);
        let args = args.trim();
        if is("cd") || is("chdir") || is("pushd") {
            // `cd` without `/D` keeps the current drive, so a path on another
            // drive does not change the current directory.
            let (switch_drive, dir) = match args.get(..2) {
                Some(flag) if flag.eq_ignore_ascii_case("/d") => (true, args[2..].trim_start()),
                _ => (is("pushd"), args),
            };
            // A redirection or a closing `(` group can follow the directory.
            let dir = dir.split(['<', '>', ')']).next().unwrap_or("");
            let dir = dir.trim().trim_matches('"');
            if dir.is_empty() {
                // Without an argument, `cd` prints the current directory.
                return;
            }
            let other_drive = dir.as_bytes().get(1) == Some(&b':');
            let target = self.dirs[0].join(dir);
            // The directory changes only if the command runs and the target
            // exists. When either is uncertain, so is the directory.
            if conditional
                || dir.contains(['%', '!', '^'])
                || (other_drive && !switch_drive)
                || !target.is_dir()
            {
                self.unknown = true;
            } else {
                self.dirs[0] = target;
            }
        } else if is("popd")
            || (is("path") && !args.is_empty())
            || (is("set") && {
                let assignment = args.trim_start_matches('"');
                assignment
                    .get(..5)
                    .is_some_and(|start| start.eq_ignore_ascii_case("path="))
            })
        {
            self.unknown = true;
        }
    }
}

/// Writes the lines of one command, which cmd joins into one, with `call`
/// before each batch command found in the joined text.
fn push_command(script: &mut String, lines: &[&str], lookup: &mut Lookup) {
    let joined = lines.concat();
    let mut calls = batch_command_starts(&joined, lookup).into_iter().peekable();
    let mut offset = 0;
    for line in lines {
        let end = offset + line.len();
        let mut copied = 0;
        while let Some(&pos) = calls.peek() {
            if pos >= end {
                break;
            }
            script.push_str(&line[copied..pos - offset]);
            script.push_str("call ");
            copied = pos - offset;
            calls.next();
        }
        script.push_str(&line[copied..]);
        script.push_str("\r\n");
        offset = end;
    }
}

/// Whether `text` ends in a `^` that escapes the line break: one outside
/// quotes that is not itself escaped.
fn ends_with_continuation(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b'^' if !in_quotes => {
                if i + 1 == bytes.len() {
                    return true;
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// The byte offsets of the commands in `line` that need `call`: the first
/// command, and each one after `&`, `&&` or `||`. Commands inside `if` or
/// `for`, or after `(` mid-line, are left as written.
fn batch_command_starts(line: &str, lookup: &mut Lookup) -> Vec<usize> {
    let bytes = line.as_bytes();
    // Each command runs from its start up to the operator after it, and is
    // conditional when the operator before it is `&&` or `||`.
    let mut commands = vec![(0, line.len(), false)];
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b'^' if !in_quotes => i += 1,
            op @ (b'&' | b'|') if !in_quotes => {
                let operator = i;
                let doubled = bytes.get(i + 1) == Some(&op);
                if doubled {
                    i += 1;
                }
                // A single `|` is a pipe: each side runs in its own cmd.
                if op == b'&' || doubled {
                    if let Some(last) = commands.last_mut() {
                        last.1 = operator;
                    }
                    commands.push((i + 1, line.len(), doubled));
                }
            }
            _ => {}
        }
        i += 1;
    }

    let mut calls = Vec::new();
    for (start, end, conditional) in commands {
        let mut pos = start;
        while pos < end && matches!(bytes[pos], b' ' | b'\t' | b'@' | b'(') {
            pos += 1;
        }
        let name = command_name(&line[pos..end]);
        if lookup.needs_call(name) {
            calls.push(pos);
        }
        if let Some(args) = line[pos..end].strip_prefix(name) {
            lookup.follow(name, args, conditional);
        }
    }
    calls
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

    #[test]
    fn calls_commands_that_are_not_found() {
        // `subtool.cmd` is only found from `sub`, and `newtool` only on the
        // changed PATH, so neither resolves when the script is written.
        let tools = Tools::new();
        std::fs::create_dir(tools.0.join("sub")).unwrap();
        std::fs::write(tools.0.join("sub").join("subtool.cmd"), "").unwrap();
        assert_eq!(
            tools.script(
                "cd sub && subtool install && echo after\nset PATH=C:\\new;%PATH%\nnewtool"
            ),
            [
                "cd sub && call subtool install && echo after",
                "set PATH=C:\\new;%PATH%",
                "call newtool",
            ]
        );
    }

    #[test]
    fn follows_the_current_directory() {
        // `realtool.exe` is found from the starting directory, but from `sub`
        // the name resolves to `sub\realtool.cmd`.
        let tools = Tools::new();
        std::fs::create_dir(tools.0.join("sub")).unwrap();
        std::fs::write(tools.0.join("sub").join("realtool.cmd"), "").unwrap();
        assert_eq!(
            tools.script(
                "cd sub && realtool && echo after\nrealtool\ncd ..\nrealtool\npushd \"sub\"\nrealtool"
            ),
            [
                "cd sub && call realtool && echo after",
                "call realtool",
                "cd ..",
                "realtool",
                "pushd \"sub\"",
                "call realtool",
            ]
        );
    }

    #[test]
    fn calls_every_command_after_a_directory_change_that_may_not_happen() {
        // `cd ..` after `||` runs only if `cd sub` fails, and `cd missing`
        // fails, so the directory is unknown after either line.
        let tools = Tools::new();
        std::fs::create_dir(tools.0.join("sub")).unwrap();
        for change in ["cd sub || cd ..", "cd sub && cd ..", "cd missing"] {
            assert_eq!(
                tools.script(&format!("{change}\nrealtool && echo after")),
                [change, "call realtool && echo after"],
                "after {change}"
            );
        }
    }

    #[test]
    fn calls_every_command_after_an_untracked_change() {
        let tools = Tools::new();
        for change in [
            "popd",
            "cd %DIR%",
            "set PATH=C:\\new;%PATH%",
            "path C:\\new",
        ] {
            assert_eq!(
                tools.script(&format!("realtool\n{change}\nrealtool\necho done")),
                ["realtool", change, "call realtool", "echo done"],
                "after {change}"
            );
        }
    }

    #[test]
    fn leaves_labels_alone() {
        let tools = Tools::new();
        assert_eq!(
            tools.script("goto done\nfaketool\n:done\n:: comment\necho end"),
            [
                "goto done",
                "call faketool",
                ":done",
                ":: comment",
                "echo end"
            ]
        );
    }

    #[test]
    fn calls_batch_commands_on_continued_lines() {
        let tools = Tools::new();
        assert_eq!(
            tools.script("echo setup ^\n&& faketool install"),
            ["echo setup ^", "&& call faketool install"]
        );
    }

    #[test]
    fn an_escaped_caret_does_not_continue_the_line() {
        let tools = Tools::new();
        assert_eq!(
            tools.script("echo a ^^\nfaketool\necho \"b ^\"\nfaketool"),
            [
                "echo a ^^",
                "call faketool",
                "echo \"b ^\"",
                "call faketool"
            ]
        );
    }
}
