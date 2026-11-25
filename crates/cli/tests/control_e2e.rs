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
    let (code, _, stderr) = run.finish();
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(fixture.status(&id)["status"], "paused");

    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--no-interactive",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("src/b.ts"), "let b = 2;\n");
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
    assert_eq!(run.event("run_finished")["status"], "paused");
}

#[test]
fn a_canceled_run_is_resumed_only_with_force() {
    let fixture = Fixture::new(TWO_APPROVALS);
    let output = fixture.run(&["--format", "jsonl"]);
    assert!(output.status.success(), "{}", text(&output));
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
    assert!(output.status.success(), "{}", text(&output));
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
