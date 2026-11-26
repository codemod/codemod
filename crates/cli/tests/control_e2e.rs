//! The control protocol and the run store, end to end on the real binary:
//! `--control stdio`, per-question approvals, rejected / paused / canceled
//! runs, the event journal and `workflow attach`, `workflow inspect`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::TempDir;

struct Fixture {
    package: TempDir,
    target: TempDir,
    store: TempDir,
}

impl Fixture {
    fn new(workflow: &str) -> Self {
        let fixture = Self {
            package: TempDir::new().unwrap(),
            target: TempDir::new().unwrap(),
            store: TempDir::new().unwrap(),
        };
        fixture.package_file(
            "scripts/replace.ts",
            r#"
export default function replace(file: { text: string; write(text: string): void }) {
  if (file.text.includes("var ")) file.write(file.text.replaceAll("var ", "let "));
}
"#,
        );
        fixture.package_file("workflow.ts", workflow);
        write(fixture.target.path(), "src/a.ts", "var a = 1;\n");
        write(fixture.target.path(), "src/b.ts", "var b = 2;\n");
        fixture
    }

    fn package_file(&self, path: &str, text: &str) {
        write(self.package.path(), path, text);
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.target.path().join(path)).unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codemod"));
        command
            .args(args)
            .env("CODEMOD_RUN_STORE_DIR", self.store.path())
            .env("DISABLE_ANALYTICS", "true")
            .env_remove("CI");
        command
    }

    fn codemod(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("failed to run codemod")
    }

    fn run_args(&self, extra: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = [
            "workflow",
            "run",
            "-w",
            self.package.path().to_str().unwrap(),
            "--target",
            self.target.path().to_str().unwrap(),
            "--no-interactive",
            "--allow-dirty",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        args.extend(extra.iter().map(|s| s.to_string()));
        args
    }

    fn run(&self, extra: &[&str]) -> Output {
        let args = self.run_args(extra);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.codemod(&args)
    }

    /// `args` with `--format jsonl --control stdio`, stdin and stdout piped.
    fn control(&self, args: &[String]) -> Controlled {
        let mut args: Vec<&str> = args.iter().map(String::as_str).collect();
        args.extend(["--format", "jsonl", "--control", "stdio"]);
        Controlled::spawn(self.command(&args))
    }

    fn status(&self, id: &str) -> Value {
        let output = self.codemod(&["workflow", "status", "--id", id, "--format", "json"]);
        assert!(output.status.success(), "{}", text(&output));
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

/// A child process driven over stdio, its stdout read line by line.
struct Controlled {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<Value>,
}

impl Controlled {
    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run codemod");
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            lines,
            seen: Vec::new(),
        }
    }

    /// Events until one matches, which is returned.
    fn until(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(120))
                .unwrap_or_else(|_| panic!("no matching event; saw {:#?}", self.seen));
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            self.seen.push(event.clone());
            if matches(&event) {
                return event;
            }
        }
    }

    fn event(&mut self, name: &str) -> Value {
        self.until(|event| event["event"] == name)
    }

    fn send(&mut self, command: Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{command}").unwrap();
        stdin.flush().unwrap();
    }

    /// Wait for the process to end: its exit code, after reading the rest.
    fn finish(mut self) -> (Option<i32>, Vec<Value>, String) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            if let Ok(event) = serde_json::from_str::<Value>(&line) {
                self.seen.push(event);
            }
        }
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
        }
        (status.code(), self.seen, stderr)
    }
}

#[cfg(unix)]
#[test]
fn killed_shell_requires_a_resume_choice() {
    for choice in ["--rerun-step", "--skip-step", "control"] {
        let fixture = Fixture::new(
            r#"
import { step, shell } from "codemod/workflow";
export default async function main() {
  await step("Before", () => 42);
  const result = await step("Publish", shell("if test -e shell.pid; then touch repeated; else echo $$ > shell.pid; exec sleep 30; fi"));
  await step("After", shell("touch continued"));
  return result === null;
}
"#,
        );
        let args = fixture.run_args(&["--format", "jsonl"]);
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        let mut original = Controlled::spawn(fixture.command(&args));
        let start = original.event("run_started");
        let id = start["run_id"].as_str().unwrap();
        let pid_path = fixture.target.path().join("shell.pid");
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let pid: Option<libc::pid_t> = loop {
            if let Some(pid) = fs::read_to_string(&pid_path)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                break Some(pid);
            }
            if std::time::Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        original.child.kill().unwrap();
        if let Some(pid) = pid.filter(|&pid| pid > 0) {
            // SAFETY: the fixture's command just recorded its PID before
            // exec'ing sleep. SIGKILL the orphan so the test leaves no work.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let (_, events, stderr) = original.finish();
        assert!(pid.is_some(), "shell never started: {events:?}\n{stderr}");

        let status = fixture.status(id);
        assert_eq!(status["status"], "interrupted", "{status}");
        assert_eq!(status["steps"][1]["status"], "started", "{status}");
        let resume = ["workflow", "resume", "--id", id, "--no-interactive"];
        let waiting = fixture.codemod(&[resume.as_slice(), &["--approve"]].concat());
        let shown = text(&waiting);
        assert_eq!(waiting.status.code(), Some(3), "{shown}");
        assert!(
            shown.contains("Publish")
                && shown.contains("--rerun-step 1")
                && shown.contains("--skip-step 1"),
            "{shown}"
        );
        assert!(!fixture.target.path().join("repeated").exists());
        assert!(!fixture.target.path().join("continued").exists());

        if choice == "control" {
            let args = [resume.as_slice(), &["--approve"]]
                .concat()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let mut run = fixture.control(&args);
            let question = run.event("question");
            assert_eq!(
                question["approval"]["kind"], "interrupted-step",
                "{question}"
            );
            assert_eq!(question["id"], "interrupted-step:1", "{question}");
            run.send(json!({
                "command": "answer",
                "question_id": "interrupted-step:1",
                "value": "reject"
            }));
            run.event("run_finished");
            let (code, _, stderr) = run.finish();
            assert_eq!(code, Some(0), "{stderr}");
        } else {
            let result = fixture.codemod(&[resume.as_slice(), &[choice, "1"]].concat());
            assert!(result.status.success(), "{}", text(&result));
        }
        let reran = choice == "--rerun-step";
        assert_eq!(fixture.target.path().join("repeated").exists(), reran);
        assert!(fixture.target.path().join("continued").exists());
        let status = fixture.status(id);
        assert_eq!(
            status["steps"][1]["status"],
            if reran { "succeeded" } else { "skipped" }
        );
        assert_eq!(status["output"], !reran);
    }
}

fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn events(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn named<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["event"] == name)
        .collect()
}

fn run_id(events: &[Value]) -> String {
    named(events, "run_started")[0]["run_id"]
        .as_str()
        .unwrap()
        .to_string()
}

const TWO_APPROVALS: &str = r#"
import { step, approval, forEachFile } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  await step("Replace a", forEachFile("src/a.ts", replace));
  await step(approval("Go on to b?"));
  await step("Replace b", forEachFile("src/b.ts", replace));
  await step(approval("Finish?"));
  return "done";
}
"#;

#[test]
fn control_stdio_approves_one_approval_and_rejects_the_next() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let mut run = fixture.control(&fixture.run_args(&[]));

    let hello = run.event("hello");
    assert_eq!(hello["protocol"], 1, "{hello}");
    let id = hello["run_id"]
        .as_str()
        .expect("hello names the run")
        .to_string();
    assert_eq!(hello["dry_run"], false);

    let first = run.event("question");
    assert_eq!(first["kind"], "approval", "{first}");
    assert_eq!(first["message"], "Go on to b?");
    assert_eq!(first["id"], "approval:1");
    assert_eq!(first["step"], "1");
    // The run waits: it is still running, nothing after the approval ran.
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
    assert_eq!(fixture.read("src/b.ts"), "var b = 2;\n");
    run.send(json!({ "command": "approve", "question_id": "approval:1" }));
    let closed = run.event("question_closed");
    assert_eq!(closed["id"], "approval:1");
    assert_eq!(closed["answer"], "approved");

    let second = run.event("question");
    assert_eq!(second["id"], "approval:3", "{second}");
    // The camelCase spelling works too.
    run.send(json!({ "command": "reject", "questionId": "approval:3" }));
    let closed = run.event("question_closed");
    assert_eq!(closed["answer"], "rejected");

    let finished = run.event("run_finished");
    assert_eq!(finished["status"], "rejected", "{finished}");
    let (code, events, stderr) = run.finish();
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(run_id(&events), id);
    // Every event after hello is numbered, in order.
    let seqs: Vec<u64> = events
        .iter()
        .skip(1)
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    let rejected = events
        .iter()
        .find(|event| event["event"] == "step_finished" && event["step"] == "3")
        .unwrap();
    assert_eq!(rejected["status"], "rejected", "{rejected}");
    assert_eq!(rejected["kind"], "approval");

    // Earlier steps' changes stay.
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
    assert_eq!(fixture.read("src/b.ts"), "let b = 2;\n");
    let status = fixture.status(&id);
    assert_eq!(status["status"], "rejected", "{status}");

    // A rejected run is over, unless resumed with --force.
    let resumed = fixture.codemod(&["workflow", "resume", "--id", &id, "--no-interactive"]);
    assert!(!resumed.status.success(), "{}", text(&resumed));
    assert!(text(&resumed).contains("--force"), "{}", text(&resumed));
}

#[test]
fn stopping_over_control_pauses_the_run_and_resume_continues_it() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let mut run = fixture.control(&fixture.run_args(&[]));
    let id = run.event("hello")["run_id"].as_str().unwrap().to_string();
    run.event("question");
    run.send(json!({ "command": "stop", "mode": "pause" }));
    let closed = run.event("question_closed");
    assert_eq!(closed["by"], "stop", "{closed}");
    let finished = run.event("run_finished");
    assert_eq!(finished["status"], "paused", "{finished}");
    // Paused at the question: the run still waits for it.
    assert_eq!(finished["question"], "approval:1", "{finished}");
    assert_eq!(finished["awaiting"], "Go on to b?", "{finished}");
    let (code, events, stderr) = run.finish();
    assert_eq!(code, Some(1), "{stderr}");
    // The approval step waits; it was not canceled.
    let step = named(&events, "step_finished")
        .into_iter()
        .find(|event| event["step"] == "1")
        .unwrap_or_else(|| panic!("{events:#?}"));
    assert_eq!(step["status"], "awaiting-approval", "{step}");
    let status = fixture.status(&id);
    assert_eq!(status["status"], "paused", "{status}");
    assert_eq!(status["question"], "approval:1", "{status}");
    assert_eq!(status["awaiting"], "Go on to b?", "{status}");
    assert_eq!(
        status["steps"][1]["status"], "awaiting-approval",
        "{status}"
    );

    // `--approve <id>` gives the question the paused run waits for.
    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "approval:1",
        "--no-interactive",
    ]);
    assert_eq!(
        fixture.read("src/b.ts"),
        "let b = 2;\n",
        "{}",
        text(&output)
    );
    let status = fixture.status(&id);
    assert_eq!(status["question"], "approval:3", "{status}");

    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--no-interactive",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.status(&id)["status"], "succeeded");
}

#[test]
fn canceling_from_another_terminal_ends_a_run_waiting_at_a_question() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let mut run = fixture.control(&fixture.run_args(&[]));
    let id = run.event("hello")["run_id"].as_str().unwrap().to_string();
    run.event("question");
    let canceled = fixture.codemod(&["workflow", "cancel", "--id", &id]);
    assert!(canceled.status.success(), "{}", text(&canceled));
    assert_eq!(run.event("question_closed")["by"], "stop");
    assert_eq!(run.event("run_finished")["status"], "canceled");
    let (code, _, stderr) = run.finish();
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(fixture.status(&id)["status"], "canceled");
}

#[test]
fn closing_stdin_pauses_the_run() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let mut run = fixture.control(&fixture.run_args(&[]));
    run.event("question");
    drop(run.stdin.take());
    let closed = run.event("question_closed");
    assert_eq!(closed["by"], "eof", "{closed}");
    let finished = run.event("run_finished");
    assert_eq!(finished["status"], "paused", "{finished}");
    assert_eq!(finished["question"], "approval:1", "{finished}");
}

#[test]
fn a_canceled_run_is_resumed_only_with_force() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let output = fixture.run(&["--format", "jsonl"]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let id = run_id(&events(&output));

    let canceled = fixture.codemod(&["workflow", "cancel", "--id", &id]);
    assert!(canceled.status.success(), "{}", text(&canceled));
    let resumed = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--no-interactive",
    ]);
    assert!(!resumed.status.success(), "{}", text(&resumed));
    assert!(text(&resumed).contains("--force"), "{}", text(&resumed));
    assert_eq!(fixture.status(&id)["status"], "canceled");

    let forced = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--force",
        "--no-interactive",
    ]);
    assert!(forced.status.success(), "{}", text(&forced));
    assert_eq!(fixture.status(&id)["status"], "succeeded");
}

#[test]
fn resume_approves_only_the_question_it_names() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let output = fixture.run(&["--format", "jsonl"]);
    let id = run_id(&events(&output));
    let status = fixture.status(&id);
    assert_eq!(status["status"], "awaiting-approval");
    assert_eq!(status["question"], "approval:1", "{status}");

    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "approval:1",
        "--no-interactive",
    ]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    assert!(
        text(&output).contains("--approve approval:3"),
        "{}",
        text(&output)
    );
    assert_eq!(fixture.read("src/b.ts"), "let b = 2;\n");
    let status = fixture.status(&id);
    assert_eq!(status["status"], "awaiting-approval");
    assert_eq!(status["question"], "approval:3");

    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "approval:3",
        "--no-interactive",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.status(&id)["status"], "succeeded");
}

#[test]
fn a_run_waiting_for_an_approval_exits_3_with_the_command_that_continues_it() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let shown = String::from_utf8_lossy(&output.stderr);
    assert!(
        shown.contains("codemod workflow resume --id") && shown.contains("--approve approval:1"),
        "{shown}"
    );

    // JSONL: stdout stays events; one line on stderr says how to go on.
    let output = fixture.run(&["--format", "jsonl"]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let id = run_id(&events(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "codemod workflow resume --id {id} --approve approval:1"
        )),
        "{stderr}"
    );
    assert_eq!(fixture.status(&id)["status"], "awaiting-approval");
}

/// Rewrite a file in the run store.
fn edit_store(fixture: &Fixture, path: &str, edit: impl FnOnce(String) -> String) {
    let path = fixture.store.path().join(path);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, edit(text)).unwrap();
}

#[test]
fn a_run_another_version_made_is_resumed_only_when_its_steps_still_match() {
    let fixture = Fixture::new(
        r#"
import { step, approval } from "codemod/workflow";
export default async function main() {
  await step("Count", () => 1);
  await step(approval("Go on?"));
  return "done";
}
"#,
    );
    let output = fixture.run(&["--format", "jsonl"]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let id = run_id(&events(&output));
    let status = fixture.status(&id);
    let version = status["codemodVersion"].as_str().unwrap().to_string();
    assert!(!version.is_empty(), "{status}");

    // Made by another version whose function steps hash the same: resumed.
    edit_store(&fixture, &format!("{id}/run.json"), |text| {
        text.replace(&version, "0.0.1-older")
    });
    let output = fixture.codemod(&["workflow", "resume", "--id", &id, "--no-interactive"]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    assert_eq!(fixture.status(&id)["codemodVersion"], version.as_str());

    // ... and one whose functions it built differently: refused, naming
    // the version and the steps, unless forced.
    edit_store(&fixture, &format!("{id}/run.json"), |text| {
        text.replace(&version, "0.0.1-older")
    });
    edit_store(&fixture, &format!("{id}/steps.jsonl"), |text| {
        regex::Regex::new("cb_[0-9a-f]{16}")
            .unwrap()
            .replace_all(&text, "cb_0000000000000000")
            .into_owned()
    });
    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--no-interactive",
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let shown = text(&output);
    assert!(
        shown.contains("codemod 0.0.1-older")
            && shown.contains("Count")
            && shown.contains("--force"),
        "{shown}"
    );
    assert_eq!(fixture.status(&id)["status"], "awaiting-approval");

    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--force",
        "--no-interactive",
        "--format",
        "jsonl",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    // The step ran again.
    assert_eq!(named(&events(&output), "step_started")[0]["label"], "Count");
    assert_eq!(fixture.status(&id)["status"], "succeeded");
}

#[test]
fn runs_this_version_cannot_read_are_listed_as_unreadable() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let dir = fixture.store.path().join("from-the-future");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("run.json"),
        r#"{"id":"from-the-future","status":"teleported","codemodVersion":"9.9.9","startedAt":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let output = fixture.codemod(&["workflow", "list"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("unreadable (made by codemod 9.9.9)"),
        "{}",
        text(&output)
    );
    let output = fixture.codemod(&["workflow", "list", "--format", "json"]);
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed[0]["status"], "unreadable", "{listed}");
    assert_eq!(listed[0]["codemodVersion"], "9.9.9");
    let output = fixture.codemod(&["workflow", "status", "--id", "from-the-future"]);
    assert!(!output.status.success());
    assert!(text(&output).contains("codemod 9.9.9"), "{}", text(&output));
}

#[test]
fn one_run_at_a_time_changes_a_target() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let mut first = fixture.control(&fixture.run_args(&[]));
    let id = first.event("hello")["run_id"].as_str().unwrap().to_string();
    first.event("question");

    // A second run on the same target fails fast, naming the first.
    let second = fixture.run(&["--approve"]);
    assert_eq!(second.status.code(), Some(1), "{}", text(&second));
    // As one line: the error is wrapped to the terminal's width.
    let shown = regex::Regex::new("\x1b\\[[0-9;]*m")
        .unwrap()
        .replace_all(&text(&second), "")
        .replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        shown.contains(&format!("another run ({id})"))
            && shown.contains(&format!("codemod workflow attach --id {id}"))
            && shown.contains(&format!("codemod workflow cancel --id {id}")),
        "{shown}"
    );
    assert_eq!(fixture.read("src/b.ts"), "var b = 2;\n");

    // A killed run's lock goes with its process.
    first.child.kill().unwrap();
    let (_, _, _) = first.finish();
    let second = fixture.run(&["--approve"]);
    assert!(second.status.success(), "{}", text(&second));
    assert_eq!(fixture.read("src/b.ts"), "let b = 2;\n");
    // And the killed run can be resumed once the target is free.
    let resumed = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--no-interactive",
    ]);
    assert!(resumed.status.success(), "{}", text(&resumed));
}

#[test]
fn seq_keeps_increasing_when_a_run_is_resumed() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let first = events(&fixture.run(&["--format", "jsonl"]));
    let id = run_id(&first);
    let last = first.last().unwrap()["seq"].as_u64().unwrap();

    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--no-interactive",
        "--format",
        "jsonl",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    let second = events(&output);
    assert_eq!(second[0]["event"], "run_started");
    assert!(
        second[0]["seq"].as_u64().unwrap() > last,
        "resumed at {} after {last}",
        second[0]["seq"]
    );
}

#[test]
fn send_refuses_to_stand_in_for_an_approval() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let id = run_id(&events(&fixture.run(&["--format", "jsonl"])));
    let output = fixture.codemod(&["workflow", "send", "approval", "--id", &id]);
    assert!(!output.status.success(), "{}", text(&output));
    // The error names the question to approve.
    let shown = text(&output);
    assert!(
        shown.contains("workflow resume") && shown.contains("approval:1"),
        "{shown}"
    );
}

#[test]
fn a_dry_run_that_reaches_an_approval_says_how_to_get_past_it() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let output = fixture.run(&["--dry-run"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("--approve"), "{}", text(&output));
    assert!(
        !text(&output).contains("workflow resume"),
        "{}",
        text(&output)
    );

    // Over the control channel, a dry run waits like a real one.
    let mut run = fixture.control(&fixture.run_args(&["--dry-run"]));
    assert_eq!(run.event("hello")["dry_run"], true);
    let question = run.event("question");
    run.send(json!({ "command": "approve", "question_id": question["id"] }));
    let question = run.event("question");
    run.send(json!({ "command": "answer", "question_id": question["id"], "value": "approve" }));
    assert_eq!(run.event("run_finished")["status"], "succeeded");
    let (code, events, stderr) = run.finish();
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(named(&events, "run_diff").len(), 2);
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");
}

#[test]
fn real_runs_report_each_steps_diff_and_steps_carry_keys_and_kinds() {
    let fixture = Fixture::new(
        r#"
import { step, forEachFile } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  console.log("from the body");
  await step("Replace", forEachFile("src/**/*.ts", replace));
}
"#,
    );
    let output = fixture.run(&["--format", "jsonl", "--param", "level=2"]);
    assert!(output.status.success(), "{}", text(&output));
    let events = events(&output);

    let started = named(&events, "run_started")[0];
    assert_eq!(started["dry_run"], false, "{started}");
    assert_eq!(started["params"], json!({ "level": 2 }));
    assert!(started["settings"]["capabilities"].is_array(), "{started}");
    assert!(started["target"].is_string());

    let step = named(&events, "step_started")[0];
    assert_eq!(step["step"], "0", "{step}");
    assert_eq!(step["parent"], Value::Null);
    assert_eq!(step["kind"], "forEachFile");
    assert_eq!(named(&events, "step_finished")[0]["kind"], "forEachFile");

    let log = named(&events, "run_log")[0];
    assert_eq!(log["line"], "from the body");
    assert_eq!(log["step"], Value::Null);

    let mut diffs: Vec<&Value> = named(&events, "step_diff");
    diffs.sort_by_key(|diff| diff["path"].as_str().unwrap().to_string());
    assert_eq!(diffs.len(), 2, "{events:#?}");
    assert_eq!(diffs[0]["path"], "src/a.ts");
    assert_eq!(diffs[0]["step"], "0");
    assert_eq!(diffs[0]["additions"], 1);
    assert!(
        diffs[0]["diff"].as_str().unwrap().contains("+let a = 1;"),
        "{}",
        diffs[0]
    );
    // Before the step finishes.
    let at = |name: &str| events.iter().position(|e| e["event"] == name).unwrap();
    assert!(at("step_diff") < at("step_finished"));
    assert!(named(&events, "run_diff").is_empty());
}

#[test]
fn a_nested_codemods_steps_name_their_parent() {
    let fixture = Fixture::new(
        r#"
import { step, codemod } from "codemod/workflow";
export default async function main() {
  await step("Child", codemod("./child"));
}
"#,
    );
    fixture.package_file(
        "child/codemod.yaml",
        "schema_version: \"1.0\"\nname: child\nversion: 0.0.1\nworkflow: workflow.ts\n",
    );
    fixture.package_file(
        "child/workflow.ts",
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  console.log("child body");
  await step("Inner", () => 1);
}
"#,
    );
    let output = fixture.run(&["--format", "jsonl"]);
    assert!(output.status.success(), "{}", text(&output));
    let events = events(&output);
    let inner = named(&events, "step_started")
        .into_iter()
        .find(|event| event["label"] == "Inner")
        .unwrap_or_else(|| panic!("{events:#?}"));
    assert_eq!(inner["step"], "0/0", "{inner}");
    assert_eq!(inner["parent"], "0");
    let log = named(&events, "run_log")
        .into_iter()
        .find(|event| event["line"] == "child body")
        .unwrap();
    assert_eq!(log["step"], "0", "{log}");

    // The text output still shows them under the parent step.
    let output = fixture.run(&[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("✓ Inner"), "{}", text(&output));
}

#[test]
fn attach_replays_a_finished_run() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let output = fixture.run(&["--format", "jsonl", "--approve"]);
    assert!(output.status.success(), "{}", text(&output));
    let original = events(&output);
    let id = run_id(&original);

    let attached = fixture.codemod(&["workflow", "attach", "--id", &id, "--format", "jsonl"]);
    assert!(attached.status.success(), "{}", text(&attached));
    let replayed = events(&attached);
    assert_eq!(replayed[0]["event"], "hello");
    assert_eq!(replayed[0]["run_id"], id.as_str());
    let strip = |event: &Value| {
        let mut event = event.clone();
        event.as_object_mut().unwrap().remove("ts");
        event
    };
    assert_eq!(
        replayed[1..].iter().map(strip).collect::<Vec<_>>(),
        original.iter().map(strip).collect::<Vec<_>>()
    );
}

#[test]
fn attach_follows_a_live_run_until_it_ends() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let mut run = fixture.control(&fixture.run_args(&[]));
    let id = run.event("hello")["run_id"].as_str().unwrap().to_string();
    let question = run.event("question");

    let mut attached = Controlled::spawn(
        fixture.command(&["workflow", "attach", "--id", &id, "--format", "jsonl"]),
    );
    let replayed_question = attached.event("question");
    assert_eq!(replayed_question["id"], question["id"]);

    run.send(json!({ "command": "approve", "question_id": question["id"] }));
    let question = run.event("question");
    run.send(json!({ "command": "approve", "question_id": question["id"] }));
    assert_eq!(run.event("run_finished")["status"], "succeeded");
    let (code, _, stderr) = run.finish();
    assert_eq!(code, Some(0), "{stderr}");

    let finished = attached.event("run_finished");
    assert_eq!(finished["status"], "succeeded");
    let (code, _, stderr) = attached.finish();
    assert_eq!(code, Some(0), "{stderr}");
}

#[test]
fn inspect_describes_a_workflow_for_a_setup_form() {
    let fixture = Fixture::new(
        r#"
import type { WorkflowContext } from "codemod/workflow";
import { step, approval, forEachFile } from "codemod/workflow";
import replace from "./scripts/replace.ts";
interface Params {
  /** Only this package. */
  package?: string;
  /**
   * How hard to try.
   * @default 2
   */
  level: number;
}
export default async function main({ params }: WorkflowContext<Params>) {
  await step("Replace", forEachFile("src/**/*.ts", replace));
  await step(approval("Ship it?"));
}
"#,
    );
    let workflow = fixture.package.path().to_str().unwrap();
    let output = fixture.codemod(&["workflow", "inspect", "-w", workflow, "--format", "json"]);
    assert!(output.status.success(), "{}", text(&output));
    let inspected: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        inspected["params"],
        json!([
            { "name": "package", "type": "string", "optional": true, "description": "Only this package.", "default": null },
            { "name": "level", "type": "number", "optional": false, "description": "How hard to try.", "default": 2 }
        ]),
        "{inspected}"
    );
    let modules = inspected["modules"].as_array().unwrap();
    assert!(modules.contains(&json!("codemod/workflow")), "{inspected}");
    assert_eq!(
        inspected["steps"],
        json!([
            { "label": "Replace", "kind": "forEachFile" },
            { "label": null, "kind": "approval" }
        ]),
        "{inspected}"
    );
    assert_eq!(inspected["uses"]["approval"], true);
    assert_eq!(inspected["uses"]["agent"], false);
    assert!(inspected["capabilities"].is_array());
}

#[test]
fn list_json_counts_failed_files_and_status_json_counts_files_and_attempts() {
    let fixture = Fixture::new(
        r#"
import { step, forEachFile } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  await step("Replace", forEachFile("src/**/*.ts", replace));
  await step("Picky", forEachFile("src/**/*.ts", (file) => {
    if (file.path.endsWith("b.ts")) throw new Error("no");
  }));
  await step("Count", () => 1);
}
"#,
    );
    write(fixture.target.path(), "src/c.ts", "const c = 3;\n");
    let output = fixture.run(&["--format", "jsonl"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    let id = run_id(&events(&output));

    let list = fixture.codemod(&["workflow", "list", "--format", "json"]);
    assert!(list.status.success(), "{}", text(&list));
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list[0]["id"], id.as_str());
    assert_eq!(list[0]["failedFiles"], 1, "{list}");
    assert!(list[0].get("steps").is_none(), "{list}");

    let status = fixture.status(&id);
    let steps = status["steps"].as_array().unwrap();
    assert_eq!(
        steps[0]["files"],
        json!({ "failed": 0, "total": 3 }),
        "{status}"
    );
    assert_eq!(steps[0]["attempts"], 1, "{status}");
    assert_eq!(
        steps[1]["files"],
        json!({ "failed": 1, "total": 3 }),
        "{status}"
    );
    // A step that is not about files has none.
    assert_eq!(steps[2]["files"], Value::Null, "{status}");
    assert_eq!(steps[2]["attempts"], 1, "{status}");
}

#[test]
fn control_needs_jsonl() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let output = fixture.run(&["--control", "stdio"]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("--format jsonl"),
        "{}",
        text(&output)
    );
}
