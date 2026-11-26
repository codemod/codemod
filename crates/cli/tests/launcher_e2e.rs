//! The interactive launcher, end to end: the real binary in a pseudo
//! terminal, driven with keys.
#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

const WORKFLOW: &str = r#"
import { step, approval, forEachFile } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main({ dryRun }: { dryRun: boolean }) {
  if (!dryRun) await step(approval("Apply the change?"));
  await step("Replace var", forEachFile("src/**/*.ts", replace));
}
"#;

const REPLACE: &str = r#"
export default function replace(file: { text: string; write(text: string): void }) {
  if (file.text.includes("var ")) file.write(file.text.replaceAll("var ", "let "));
}
"#;

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Everything printed, without escape sequences: for the plain lines
/// printed after the screen closes.
fn plain(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let escapes = regex::Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|[()][0-9A-Za-z]|[=>78])").unwrap();
    escapes.replace_all(&text, "").into_owned()
}

const ROWS: u16 = 30;
const COLS: u16 = 110;

struct Session {
    output: Arc<Mutex<Vec<u8>>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    screen: vt100::Parser,
    /// How much of the output the screen has read.
    fed: usize,
}

impl Session {
    /// The launcher is opt-in: these sessions ask for it.
    fn start(home: &Path, store: &Path, cwd: &Path, args: &[&str]) -> Self {
        Self::start_with(home, store, cwd, args, Some("1"))
    }

    fn start_with(
        home: &Path,
        store: &Path,
        cwd: &Path,
        args: &[&str],
        launcher: Option<&str>,
    ) -> Self {
        Self::start_env(home, store, cwd, args, launcher, &[])
    }

    /// `env` is set last, e.g. `CI=1`.
    fn start_env(
        home: &Path,
        store: &Path,
        cwd: &Path,
        args: &[&str],
        launcher: Option<&str>,
        env: &[(&str, &str)],
    ) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_codemod"));
        command.args(args);
        command.cwd(cwd);
        command.env("HOME", home);
        command.env("XDG_CONFIG_HOME", home.join(".config"));
        command.env("CODEMOD_RUN_STORE_DIR", store);
        command.env("DISABLE_ANALYTICS", "true");
        command.env("TERM", "xterm-256color");
        // Nothing to fetch the model list from.
        command.env("TYPESAFE_BASE_URL", "http://127.0.0.1:9");
        command.env_remove("CI");
        match launcher {
            Some(value) => command.env("CODEMOD_LAUNCHER", value),
            None => command.env_remove("CODEMOD_LAUNCHER"),
        }
        for (key, value) in env {
            command.env(key, value);
        }
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);

        let output = Arc::new(Mutex::new(Vec::new()));
        let mut reader = pair.master.try_clone_reader().unwrap();
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buffer[..read]);
            }
        });
        let writer = pair.master.take_writer().unwrap();
        // The master must outlive the child; leak it for the test's length.
        std::mem::forget(pair.master);
        Self {
            output,
            writer,
            child,
            screen: vt100::Parser::new(ROWS, COLS, 0),
            fed: 0,
        }
    }

    fn text(&self) -> String {
        plain(&self.output.lock().unwrap())
    }

    /// What the terminal shows now.
    fn screen(&mut self) -> String {
        let output = self.output.lock().unwrap();
        self.screen.process(&output[self.fed..]);
        self.fed = output.len();
        drop(output);
        self.screen.screen().contents()
    }

    /// Wait until the terminal shows `needle`.
    fn wait_for(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let screen = self.screen();
            if screen.contains(needle) {
                // Let the frame finish drawing.
                std::thread::sleep(Duration::from_millis(200));
                return;
            }
            if Instant::now() > deadline {
                panic!("timed out waiting for {needle:?}; the screen shows:\n{screen}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn keys(&mut self, keys: &str) {
        self.writer.write_all(keys.as_bytes()).unwrap();
        self.writer.flush().unwrap();
        std::thread::sleep(Duration::from_millis(150));
    }

    /// Wait for the process to exit: its exit code, and what it showed.
    fn finish(mut self) -> (u32, String) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                std::thread::sleep(Duration::from_millis(200));
                return (status.exit_code(), self.text());
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("codemod did not exit; output:\n{}", self.text());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn saved_config(home: &Path) -> serde_json::Value {
    let candidates = [
        home.join(".config/codemod/config.json"),
        home.join("Library/Application Support/codemod/config.json"),
    ];
    let path = candidates
        .iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| panic!("no config saved in {}", home.display()));
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

const UP: &str = "\x1b[A";
const RIGHT: &str = "\x1b[C";

#[test]
fn a_dry_run_is_reviewed_applied_and_approved_in_the_launcher() {
    let package = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    write(package.path(), "workflow.ts", WORKFLOW);
    write(package.path(), "scripts/replace.ts", REPLACE);
    write(target.path(), "src/a.ts", "var a = 1;\n");

    let mut session = Session::start(
        home.path(),
        store.path(),
        target.path(),
        &[
            "workflow",
            "run",
            "-w",
            package.path().to_str().unwrap(),
            "--target",
            target.path().to_str().unwrap(),
            "--allow-dirty",
        ],
    );

    // Setup: the focus starts on Start. Pick the next model, then turn on
    // the dry run (fields: target, dry run, agent, model).
    session.wait_for("Start run");
    session.keys(UP);
    session.keys(RIGHT);
    for _ in 0..2 {
        session.keys(UP);
    }
    session.keys(" ");
    session.wait_for("Start dry run");
    assert!(session.screen().contains("systemone/jev-preview"));
    session.keys("s");

    // The dry run ends with its diff; nothing is written yet.
    session.wait_for("done");
    assert_eq!(
        fs::read_to_string(target.path().join("src/a.ts")).unwrap(),
        "var a = 1;\n"
    );
    session.keys("\t");
    session.wait_for("+let a = 1;");

    // Apply it for real: the approval is answered in place.
    session.keys("a");
    session.wait_for("Approval needed");
    session.wait_for("Apply the change?");
    session.keys("y");
    session.wait_for("done");
    session.keys("q");

    // The plain record stays in the terminal; then the report question.
    session.wait_for("View results in the browser");
    session.keys("n\r");
    let (code, output) = session.finish();
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("approval: Apply the change?"), "{output}");
    assert!(output.contains("1 file changed"), "{output}");
    assert_eq!(
        fs::read_to_string(target.path().join("src/a.ts")).unwrap(),
        "let a = 1;\n"
    );

    // The choices are remembered for the next run.
    let config = saved_config(home.path());
    assert_eq!(
        config["decisionModel"],
        serde_json::json!({ "provider": "systemone", "model": "jev-preview" })
    );
}

#[test]
fn pausing_at_an_approval_leaves_a_run_to_resume() {
    let package = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    write(package.path(), "workflow.ts", WORKFLOW);
    write(package.path(), "scripts/replace.ts", REPLACE);
    write(target.path(), "src/a.ts", "var a = 1;\n");

    let mut session = Session::start(
        home.path(),
        store.path(),
        target.path(),
        &[
            "workflow",
            "run",
            "-w",
            package.path().to_str().unwrap(),
            "--target",
            target.path().to_str().unwrap(),
            "--allow-dirty",
        ],
    );
    session.wait_for("Start run");
    session.keys("s");
    session.wait_for("Apply the change?");
    // Later: the run pauses, and quitting prints how to resume it.
    session.keys("l");
    session.wait_for("waiting for approval");
    session.keys("q");
    let (code, output) = session.finish();
    // Waiting for input: exit code 3.
    assert_eq!(code, 3, "{output}");
    let id = regex::Regex::new(r"workflow resume --id ([0-9a-f-]+) --approve")
        .unwrap()
        .captures(&output)
        .unwrap_or_else(|| panic!("no resume command in:\n{output}"))[1]
        .to_string();
    assert_eq!(
        fs::read_to_string(target.path().join("src/a.ts")).unwrap(),
        "var a = 1;\n"
    );

    // Resuming in the launcher asks again, in place.
    let mut session = Session::start(
        home.path(),
        store.path(),
        target.path(),
        &["workflow", "resume", "--id", &id],
    );
    session.wait_for("Apply the change?");
    session.keys("y");
    session.wait_for("done");
    session.keys("q");
    let (code, output) = session.finish();
    assert_eq!(code, 0, "{output}");
    assert_eq!(
        fs::read_to_string(target.path().join("src/a.ts")).unwrap(),
        "let a = 1;\n"
    );
}

/// In a terminal without `CODEMOD_LAUNCHER=1`, a run asks with plain
/// prompts and prints plain lines: no full-screen launcher.
#[test]
fn without_the_opt_in_a_terminal_run_uses_plain_prompts() {
    let package = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    write(package.path(), "workflow.ts", WORKFLOW);
    write(package.path(), "scripts/replace.ts", REPLACE);
    write(target.path(), "src/a.ts", "var a = 1;\n");

    let mut session = Session::start_with(
        home.path(),
        store.path(),
        target.path(),
        &[
            "workflow",
            "run",
            "-w",
            package.path().to_str().unwrap(),
            "--target",
            target.path().to_str().unwrap(),
            "--allow-dirty",
        ],
        None,
    );
    session.wait_for("Apply the change?");
    assert!(!session.text().contains("Start run"), "{}", session.text());
    session.keys("y\r");
    session.wait_for("View results in the browser");
    session.keys("n\r");
    // Never switched to the alternate screen.
    let raw = String::from_utf8_lossy(&session.output.lock().unwrap()).into_owned();
    assert!(!raw.contains("\x1b[?1049h"), "{raw}");
    let (success, output) = session.finish();
    assert!(success, "{output}");
    assert_eq!(
        fs::read_to_string(target.path().join("src/a.ts")).unwrap(),
        "let a = 1;\n"
    );
}

/// With `CI=1`, even on a terminal, nothing asks: the no-command menu prints
/// next steps, a package with several workflows runs its default one, and
/// `ai` sets up with its defaults.
#[test]
fn ci_never_prompts_even_on_a_terminal() {
    let home = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let ci = [("CI", "1")];

    let session = Session::start_env(home.path(), store.path(), cwd.path(), &[], None, &ci);
    let (success, output) = session.finish();
    assert!(!success, "{output}");
    assert!(output.contains("Next steps"), "{output}");
    assert!(!output.contains("What would you like to do?"), "{output}");

    let package = TempDir::new().unwrap();
    write(
        package.path(),
        "codemod.yaml",
        "schema_version: \"1.0\"\nname: two\nversion: 0.1.0\ndescription: two\nauthor: test\nworkflows:\n  - name: first\n    path: first.ts\n    default: true\n  - name: second\n    path: second.ts\n",
    );
    for name in ["first", "second"] {
        write(
            package.path(),
            &format!("{name}.ts"),
            &format!("export default async function main() {{ return \"ran {name}\"; }}\n"),
        );
    }
    let session = Session::start_env(
        home.path(),
        store.path(),
        cwd.path(),
        &[
            "run",
            package.path().to_str().unwrap(),
            "--target",
            cwd.path().to_str().unwrap(),
            "--allow-dirty",
        ],
        None,
        &ci,
    );
    let (success, output) = session.finish();
    assert!(success, "{output}");
    assert!(output.contains("ran first"), "{output}");
    assert!(!output.contains("Select a workflow"), "{output}");

    let session = Session::start_env(
        home.path(),
        store.path(),
        cwd.path(),
        &["ai", "--harness", "claude"],
        None,
        &ci,
    );
    let (_, output) = session.finish();
    assert!(!output.contains("Set it up for:"), "{output}");
}

#[test]
fn quitting_the_setup_runs_nothing() {
    let package = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    write(package.path(), "workflow.ts", WORKFLOW);
    write(package.path(), "scripts/replace.ts", REPLACE);
    write(target.path(), "src/a.ts", "var a = 1;\n");

    let mut session = Session::start(
        home.path(),
        store.path(),
        target.path(),
        &[
            "workflow",
            "run",
            "-w",
            package.path().to_str().unwrap(),
            "--target",
            target.path().to_str().unwrap(),
            "--allow-dirty",
        ],
    );
    session.wait_for("Start run");
    session.keys("q");
    let (code, output) = session.finish();
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("Nothing ran."), "{output}");
    assert_eq!(
        fs::read_to_string(target.path().join("src/a.ts")).unwrap(),
        "var a = 1;\n"
    );
}
