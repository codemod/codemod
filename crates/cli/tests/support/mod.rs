//! Running `codemod workflow run -w <workflow.ts>` on a scratch repository.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

/// `packages/orchestration/tests/fixtures/<path>`.
pub fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/orchestration/tests/fixtures")
        .join(path)
}

/// The orchestration sources the fixtures import.
pub fn runtime_sources() -> PathBuf {
    fixture("../../src")
}

pub fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(path, content)| (path.to_string(), content.to_string()))
        .collect()
}

pub fn write_files(root: &Path, files: &BTreeMap<String, String>) {
    for (path, content) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }
}

/// Every regular file under `root` (symlinked directories are not entered).
pub fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let relative = entry.path().strip_prefix(root).unwrap().to_owned();
                files.insert(
                    relative.to_string_lossy().replace('\\', "/"),
                    String::from_utf8_lossy(&fs::read(entry.path()).unwrap()).into_owned(),
                );
            }
        }
    }
    files
}

/// A scratch directory holding the target repository, the bundle cache and
/// a home directory, so runs share nothing with the machine or each other.
pub struct Scratch {
    pub dir: TempDir,
}

impl Scratch {
    /// A target repository holding `files`.
    pub fn with(files: &BTreeMap<String, String>) -> Self {
        let scratch = Self {
            dir: TempDir::new().unwrap(),
        };
        fs::create_dir_all(scratch.target()).unwrap();
        write_files(&scratch.target(), files);
        scratch
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn target(&self) -> PathBuf {
        self.path("repo")
    }

    pub fn files(&self) -> BTreeMap<String, String> {
        tree(&self.target())
    }

    /// `codemod workflow run -w <workflow> --target <repo> <extra>`, with
    /// the fixtures' runtime sources.
    pub fn command(&self, workflow: &Path, extra: &[&str]) -> Command {
        let mut command = self.bare_command(workflow, extra);
        command.env("CODEMOD_ORCHESTRATION_SRC", runtime_sources());
        command
    }

    /// As [`Scratch::command`], on the runtime built into the binary.
    pub fn bare_command(&self, workflow: &Path, extra: &[&str]) -> Command {
        let home = self.path("home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_codemod"));
        command
            .arg("--disable-analytics")
            .args(["workflow", "run", "-w"])
            .arg(workflow)
            .arg("--target")
            .arg(self.target())
            .args(extra)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("CODEMOD_WORKFLOW_CACHE", self.path("cache"))
            .env("NO_COLOR", "1")
            .env_remove("CODEMOD_ORCHESTRATION_SRC")
            .env_remove("CODEMOD_AGENT_BIN");
        command
    }

    pub fn run(&self, workflow: &Path, extra: &[&str]) -> Output {
        self.command(workflow, extra).output().unwrap()
    }

    /// Starts a run in the background, for signal tests.
    pub fn spawn(&self, mut command: Command) -> Child {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
}

/// The JSON a successful run printed.
pub fn output(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// What a failed run reported.
pub fn stderr(output: &Output) -> String {
    assert!(
        !output.status.success(),
        "the run succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Polls `ready` until it holds or ten seconds pass.
pub fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn signal(child: &Child, signal: libc::c_int) {
    // SAFETY: kill has no memory-safety preconditions.
    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
}

pub fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
