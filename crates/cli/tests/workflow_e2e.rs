//! `codemod workflow ...` end to end, on the real binary.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    package: TempDir,
    target: TempDir,
    store: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            package: TempDir::new().unwrap(),
            target: TempDir::new().unwrap(),
            store: TempDir::new().unwrap(),
        };
        fixture.package_file(
            "scripts/codemod.ts",
            r#"
export default function transform(root: any) {
  const text = root.root().text();
  return text.includes("var ") ? text.replaceAll("var ", "let ") : null;
}
"#,
        );
        // The same change as a callback, for workflow.ts.
        fixture.package_file(
            "scripts/replace.ts",
            r#"
export default function replace(file: { text: string; write(text: string): void }) {
  if (file.text.includes("var ")) file.write(file.text.replaceAll("var ", "let "));
}
"#,
        );
        fixture.target_file("src/a.ts", "var a = 1;\n");
        fixture.target_file("src/b.ts", "const b = 2;\n");
        fixture
    }

    fn package_file(&self, path: &str, text: &str) {
        write(self.package.path(), path, text);
    }

    fn target_file(&self, path: &str, text: &str) {
        write(self.target.path(), path, text);
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.target.path().join(path)).unwrap()
    }

    fn codemod(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_codemod"))
            .args(args)
            .env("CODEMOD_RUN_STORE_DIR", self.store.path())
            .env("DISABLE_ANALYTICS", "true")
            .output()
            .expect("failed to run codemod")
    }

    fn run(&self, extra: &[&str]) -> Output {
        let package = self.package.path().to_str().unwrap().to_string();
        let target = self.target.path().to_str().unwrap().to_string();
        let mut args = vec![
            "workflow",
            "run",
            "-w",
            &package,
            "--target",
            &target,
            "--no-interactive",
            "--allow-dirty",
        ];
        args.extend_from_slice(extra);
        self.codemod(&args)
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

/// The run id from `workflow list`, most recent first.
fn latest_run_id(fixture: &Fixture) -> String {
    let output = fixture.codemod(&["workflow", "list"]);
    let listing = text(&output);
    listing
        .lines()
        .find_map(|line| {
            line.split('|')
                .map(str::trim)
                .find(|cell| cell.len() == 36 && cell.matches('-').count() == 4)
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("no run in listing:\n{listing}"))
}

const YAML: &str = r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Replace var
        js-ast-grep:
          js_file: scripts/codemod.ts
          include: ["src/**/*.ts"]
"#;

#[test]
fn a_workflow_yaml_package_runs() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.yaml", YAML);

    let output = fixture.run(&[]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
    assert_eq!(fixture.read("src/b.ts"), "const b = 2;\n");
    assert!(text(&output).contains("Replace var"), "{}", text(&output));
}

#[test]
fn upgrade_converts_the_package_and_the_result_runs_the_same() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.yaml", YAML);

    let package = fixture.package.path().to_str().unwrap();
    let output = fixture.codemod(&["workflow", "upgrade", package]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(fixture.package.path().join("workflow.ts").is_file());
    assert!(!fixture.package.path().join("workflow.yaml").exists());

    let output = fixture.codemod(&["workflow", "validate", "-w", package]);
    assert!(output.status.success(), "{}", text(&output));

    let output = fixture.run(&[]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
}

#[test]
fn a_dry_run_changes_nothing() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.yaml", YAML);
    let output = fixture.run(&["--dry-run"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");
}

#[test]
fn jsonl_output_has_one_event_per_line() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.yaml", YAML);
    let output = fixture.run(&["--format", "jsonl"]);
    assert!(output.status.success(), "{}", text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let kinds: Vec<&str> = events.iter().filter_map(|e| e["event"].as_str()).collect();
    assert_eq!(kinds.first(), Some(&"run_started"), "{stdout}");
    assert!(kinds.contains(&"step_finished"), "{stdout}");
    assert_eq!(kinds.last(), Some(&"run_finished"), "{stdout}");
}

const TS_FAILING: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  await step("Replace var", forEachFile("src/**/*.ts", replace));
  await step("Check", () => { throw new Error("not ready"); });
}
"#;

#[test]
fn a_failed_run_is_listed_and_resumes_after_the_fix() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.ts", TS_FAILING);

    let output = fixture.run(&[]);
    assert!(!output.status.success());
    assert!(text(&output).contains("not ready"), "{}", text(&output));

    let id = latest_run_id(&fixture);
    let status = fixture.codemod(&["workflow", "status", "--id", &id]);
    let status_text = text(&status);
    assert!(status_text.contains("failed"), "{status_text}");
    assert!(status_text.contains("Replace var"), "{status_text}");

    fixture.package_file(
        "workflow.ts",
        &TS_FAILING.replace("throw new Error(\"not ready\");", "return 1;"),
    );
    let output = fixture.codemod(&["workflow", "resume", "--id", &id, "--no-interactive"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("done earlier"), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
}

#[test]
fn an_event_sent_to_a_run_reaches_the_step_waiting_for_it() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step, waitForEvent } from "codemod/workflow";
export default async function main() {
  const ci = await waitForEvent("ci-finished", { timeout: "300ms" });
  await step("Report", () => { console.log(`passed: ${ci.passed}`); });
}
"#,
    );

    let output = fixture.run(&[]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("no 'ci-finished' event came within 300ms"),
        "{}",
        text(&output)
    );

    let id = latest_run_id(&fixture);
    let refused = fixture.codemod(&["workflow", "send", "ci finished", "--id", &id]);
    assert!(!refused.status.success());
    assert!(text(&refused).contains("cannot name an event"));
    let sent = fixture.codemod(&[
        "workflow",
        "send",
        "ci-finished",
        "--id",
        &id,
        "--data",
        r#"{"passed":true}"#,
    ]);
    assert!(sent.status.success(), "{}", text(&sent));

    let output = fixture.codemod(&["workflow", "resume", "--id", &id, "--no-interactive"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("passed: true"), "{}", text(&output));
}

#[test]
fn a_manual_step_waits_until_resumed_with_approval() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step, approval, forEachFile } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  await step(approval("Apply the change?"));
  await step(forEachFile("src/**/*.ts", replace));
}
"#,
    );

    let output = fixture.run(&[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("Waiting for approval"),
        "{}",
        text(&output)
    );
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");

    let id = latest_run_id(&fixture);
    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--approve",
        "--no-interactive",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");

    let output = fixture.codemod(&["workflow", "cancel", "--id", &id]);
    assert!(
        text(&output).contains("already succeeded"),
        "{}",
        text(&output)
    );
}

#[test]
fn the_harness_runs_one_test_from_a_spec_on_stdin() {
    use std::io::Write as _;

    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { forEachFile, step } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  const found = await step("Find", forEachFile("**/*.ts", (file) => file.path));
  await step("Replace var", forEachFile("**/*.ts", replace));
  await step("Never", forEachFile("**/*.ts", () => { throw new Error("never"); }));
  return found;
}
"#,
    );
    let spec = serde_json::json!({
        "workflow": fixture.package.path(),
        "target": fixture.target.path(),
        "until": "Replace var",
        "given": { "Find": { "given": true } },
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_codemod"))
        .args(["workflow", "harness"])
        .env("DISABLE_ANALYTICS", "true")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(spec.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", text(&output));

    let result: serde_json::Value =
        serde_json::from_slice(output.stdout.trim_ascii_end()).expect("one JSON line");
    assert_eq!(result["status"], "stopped");
    assert_eq!(result["stoppedAt"], "Replace var");
    assert_eq!(result["steps"][0]["given"], true);
    assert_eq!(
        result["steps"][0]["result"],
        serde_json::json!({ "given": true })
    );
    assert_eq!(result["steps"].as_array().unwrap().len(), 2);
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
}

#[test]
fn a_workflow_that_assesses_fails_before_any_step_without_permission_or_a_key() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step, forEachFile } from "codemod/workflow";
import { assess, yesNo } from "codemod/assess";
export default async function main() {
  await step("Mark", forEachFile("src/a.ts", (file) => { file.write("marked\n"); }));
  await step("Ask", forEachFile("src/*.ts", (file) => assess(file, { questions: { ok: yesNo("Fine?") } })));
}
"#,
    );
    let home = TempDir::new().unwrap();
    let run = |extra: &[&str]| {
        let package = fixture.package.path().to_str().unwrap();
        let target = fixture.target.path().to_str().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_codemod"));
        command
            .args(["workflow", "run", "-w", package, "--target", target])
            .args(["--no-interactive", "--allow-dirty"])
            .args(extra)
            .env("CODEMOD_RUN_STORE_DIR", fixture.store.path())
            .env("DISABLE_ANALYTICS", "true")
            // No user config, and no key.
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("APPDATA", home.path());
        for name in [
            "TYPESAFE_API_KEY",
            "OPENAI_API_KEY",
            "DECISION_API_KEY",
            "DECISION_PROVIDER",
            "DECISION_MODEL",
        ] {
            command.env_remove(name);
        }
        command.output().expect("failed to run codemod")
    };

    let output = run(&[]);
    assert!(!output.status.success());
    assert!(text(&output).contains("--allow-ai"), "{}", text(&output));

    let output = run(&["--allow-ai"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("set TYPESAFE_API_KEY (or DECISION_API_KEY)"),
        "{}",
        text(&output)
    );
    // Neither run got to the first step.
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");
}
