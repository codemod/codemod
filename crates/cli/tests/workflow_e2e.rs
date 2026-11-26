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
            .env_remove("CODEMOD_V1")
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

#[test]
fn inspect_marks_step_kinds_it_cannot_determine_statically() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step, sleep, approval } from "codemod/workflow";
export default async function main({ params }) {
  throw new Error("inspect must not run the workflow");
  await step("Wait", sleep(1));
  await step("Choose", params.wait ? sleep(1) : approval("Continue?"));
}
"#,
    );
    let workflow = fixture.package.path().join("workflow.ts");
    let args = ["workflow", "inspect", "-w", workflow.to_str().unwrap()];
    let output = fixture.codemod(&[&args[..], &["--format", "json"]].concat());
    assert!(output.status.success(), "{}", text(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["steps"],
        serde_json::json!([
            { "label": "Wait", "kind": "sleep" },
            { "label": "Choose", "kind": "dynamic" },
        ])
    );
    let output = fixture.codemod(&args);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("Choose (dynamic)"));
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

const YAML_PACKAGE_MANIFEST: &str =
    "schema_version: \"1.0\"\nname: test\nversion: 0.1.0\ndescription: test\nauthor: test\n";

fn assert_file_failure(fixture: &Fixture, file: &str) -> String {
    let output = fixture.run(&["--format", "jsonl"]);
    let shown = text(&output);
    assert_eq!(output.status.code(), Some(1), "{shown}");
    let finished = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| event["event"] == "run_finished")
        .unwrap_or_else(|| panic!("{shown}"));
    assert_eq!(finished["status"], "failed", "{shown}");
    assert_eq!(finished["steps"][0]["status"], "failed", "{shown}");
    assert_eq!(finished["failed_files"], 1, "{shown}");
    let error = finished["error"].as_str().unwrap();
    assert!(error.contains(file), "{shown}");
    error.to_owned()
}

fn assert_parse_failure(fixture: &Fixture, file: &str) {
    let error = assert_file_failure(fixture, file);
    assert!(error.to_ascii_lowercase().contains("parse"), "{error}");
}

#[test]
fn a_malformed_transform_module_fails_the_workflow_step_and_counts_the_file() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.target.path().join("src/b.ts")).unwrap();
    fixture.package_file("scripts/bad.js", "export default function transform( {;");
    fixture.package_file(
        "workflow.ts",
        r#"
import { step, forEachFile } from "codemod/workflow";
import transform from "./scripts/bad.js";
export default async function main() {
  await step("Transform", forEachFile("src/a.ts", transform));
}
"#,
    );
    assert_parse_failure(&fixture, "bad.js");
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");
}

#[test]
fn a_malformed_legacy_transform_module_fails_the_workflow_step_and_counts_the_file() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.target.path().join("src/b.ts")).unwrap();
    fixture.package_file("scripts/bad.js", "export default function transform( {;");
    fixture.package_file("workflow.yaml", &YAML.replace("codemod.ts", "bad.js"));
    assert_parse_failure(&fixture, "bad.js");
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");
}

#[test]
fn a_missing_legacy_transform_dependency_fails_the_step_and_counts_the_file() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.target.path().join("src/b.ts")).unwrap();
    fixture.package_file("workflow.yaml", YAML);
    fixture.package_file(
        "scripts/codemod.ts",
        r#"export { default } from "./missing.js";"#,
    );
    let error = assert_file_failure(&fixture, "missing.js");
    assert!(error.contains("codemod.ts"), "{error}");
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");
}

#[test]
fn a_legacy_transform_does_not_treat_malformed_input_as_a_noop() {
    let fixture = Fixture::new();
    fixture.target_file("src/bad.js", "foo({;\n");
    fixture.package_file("workflow.yaml", &YAML.replace("**/*.ts", "**/*.js"));
    assert_parse_failure(&fixture, "bad.js");
    assert_eq!(fixture.read("src/bad.js"), "foo({;\n");
}

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
    // It is not saved, so no run id to look up.
    assert!(
        text(&output).contains("Dry run (not saved)"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("Run "), "{}", text(&output));
}

#[test]
fn a_container_runtime_is_refused_before_anything_runs() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: where
    name: Where
    runtime:
      type: docker
      image: alpine
    steps:
      - name: Touch
        run: touch ran.txt
"#,
    );
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("node 'where' runs its steps in a docker container")
            && text(&output).matches("codemod run --v1").count() == 1,
        "{}",
        text(&output)
    );
    assert!(!fixture.target.path().join("ran.txt").exists());

    let package = fixture.package.path().to_str().unwrap();
    fixture.package_file("codemod.yaml", YAML_PACKAGE_MANIFEST);
    let output = fixture.codemod(&["run", package, "--no-interactive"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert_eq!(text(&output).matches("codemod run --v1").count(), 1);

    let output = fixture.codemod(&["workflow", "upgrade", package]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(!fixture.package.path().join("workflow.ts").exists());
}

#[test]
fn a_legacy_fs_write_points_to_v1_without_running_it() {
    let fixture = Fixture::new();
    fixture.package_file("codemod.yaml", YAML_PACKAGE_MANIFEST);
    fixture.package_file("workflow.yaml", YAML);
    fixture.package_file(
        "scripts/codemod.ts",
        r#"
import { writeFileSync } from "node:fs";
export default function transform(root) {
  if (root.root().text().startsWith("var")) writeFileSync("extra.txt", "no");
  return null;
}
"#,
    );
    let package = fixture.package.path().to_str().unwrap();
    let target = fixture.target.path().to_str().unwrap();
    let output = fixture.codemod(&[
        "run",
        package,
        "--target",
        target,
        "--no-interactive",
        "--allow-dirty",
    ]);
    let message = text(&output);
    assert_eq!(output.status.code(), Some(2), "{message}");
    assert!(
        message.contains("fs.writeFileSync is not available in workflow callbacks"),
        "{message}"
    );
    assert_eq!(message.matches("codemod run --v1").count(), 1, "{message}");
    assert!(!fixture.target.path().join("extra.txt").exists());
}

#[test]
fn a_param_declared_as_string_arrives_as_text() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import type { WorkflowContext } from "codemod/workflow";
interface Params {
  version: string;
  count?: number;
}
export default async function main({ params }: WorkflowContext<Params>) {
  return { version: params.version, count: params.count, extra: params.extra };
}
"#,
    );
    let output = fixture.run(&[
        "--format",
        "jsonl",
        "--param",
        "version=4",
        "--param",
        "count=4",
        "--param",
        "extra=true",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        run_finished(&output)["output"],
        serde_json::json!({ "version": "4", "count": 4, "extra": true })
    );
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

/// Fails on files without `var `: src/b.ts in the fixture.
const PICKY: &str = r#"
export default function picky(file: { text: string; write(text: string): void }) {
  if (!file.text.includes("var ")) throw new Error("no var here");
  file.write(file.text.replaceAll("var ", "let "));
}
"#;

const TS_SOME_FILES_FAIL: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import picky from "./scripts/picky.ts";
export default async function main() {
  await step("Replace var", forEachFile("src/**/*.ts", picky));
}
"#;

/// The YAML transform that fails on src/b.ts.
const PICKY_TRANSFORM: &str = r#"
export default function transform(root: any) {
  const text = root.root().text();
  if (!text.includes("var ")) throw new Error("no var here");
  return text.replaceAll("var ", "let ");
}
"#;

/// The last line of stderr that is not blank.
fn last_line(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(|line| console::strip_ansi_codes(line).trim().to_string())
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
}

fn some_files_fail() -> Fixture {
    let fixture = Fixture::new();
    fixture.package_file("scripts/picky.ts", PICKY);
    fixture.package_file("workflow.ts", TS_SOME_FILES_FAIL);
    fixture
}

#[test]
fn a_run_where_some_files_failed_exits_2_and_says_so_last() {
    let fixture = some_files_fail();
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
    assert_eq!(
        last_line(&output),
        "1 file failed; changes to the other files were written.",
        "{}",
        text(&output)
    );
}

#[test]
fn a_dry_run_where_some_files_failed_exits_2_and_says_nothing_was_written() {
    let fixture = some_files_fail();
    let output = fixture.run(&["--dry-run"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");
    assert_eq!(
        last_line(&output),
        "1 file failed; dry run: nothing was written.",
        "{}",
        text(&output)
    );
}

#[test]
fn a_hard_failure_still_exits_1() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.ts", TS_FAILING);
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
}

#[test]
fn run_finished_counts_failed_files_in_jsonl() {
    let fixture = some_files_fail();
    let output = fixture.run(&["--format", "jsonl"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let finished: serde_json::Value = stdout
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .rfind(|event| event["event"] == "run_finished")
        .unwrap_or_else(|| panic!("no run_finished in {stdout}"));
    assert_eq!(finished["status"], "succeeded", "{finished}");
    assert_eq!(finished["failed_files"], 1, "{finished}");
    assert_eq!(
        finished["partial_steps"],
        serde_json::json!([{ "position": 0, "label": "Replace var", "failed_files": 1, "files": 2 }]),
        "{finished}"
    );
    // JSONL has no text summary on stderr.
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("file failed"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_workflow_yaml_run_where_some_files_failed_exits_2() {
    let fixture = Fixture::new();
    fixture.package_file("scripts/codemod.ts", PICKY_TRANSFORM);
    fixture.package_file("workflow.yaml", YAML);
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
    assert_eq!(
        last_line(&output),
        "1 file failed; changes to the other files were written.",
        "{}",
        text(&output)
    );
}

#[test]
fn a_resumed_run_with_failed_files_exits_2() {
    let fixture = Fixture::new();
    fixture.package_file("scripts/picky.ts", PICKY);
    let failing = TS_SOME_FILES_FAIL.replace(
        "forEachFile(\"src/**/*.ts\", picky));",
        "forEachFile(\"src/**/*.ts\", picky));\n  await step(\"Check\", () => { throw new Error(\"not ready\"); });",
    );
    fixture.package_file("workflow.ts", &failing);
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));

    let id = latest_run_id(&fixture);
    fixture.package_file(
        "workflow.ts",
        &failing.replace("throw new Error(\"not ready\");", "return 1;"),
    );
    let output = fixture.codemod(&["workflow", "resume", "--id", &id, "--no-interactive"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert_eq!(
        last_line(&output),
        "1 file failed; changes to the other files were written.",
        "{}",
        text(&output)
    );
}

#[test]
fn codemod_run_where_some_files_failed_exits_2() {
    let fixture = some_files_fail();
    fixture.package_file(
        "codemod.yaml",
        "schema_version: \"1.0\"\nname: picky\nversion: 0.1.0\ndescription: picky\nauthor: test\nworkflow: workflow.ts\n",
    );
    let package = fixture.package.path().to_str().unwrap();
    let target = fixture.target.path().to_str().unwrap();
    let output = fixture.codemod(&[
        "run",
        package,
        "--target",
        target,
        "--no-interactive",
        "--allow-dirty",
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
    assert_eq!(
        last_line(&output),
        "1 file failed; changes to the other files were written.",
        "{}",
        text(&output)
    );
    // A local directory is not looked up in the registry.
    assert!(
        text(&output).contains(&format!("Using local package: {package}"))
            && !text(&output).contains("from registry"),
        "{}",
        text(&output)
    );
}

#[test]
fn resume_refuses_removed_1x_flags_with_what_replaced_them() {
    let fixture = Fixture::new();
    let resume = |extra: &[&str]| {
        let mut args = vec!["workflow", "resume", "--id", "some-run"];
        args.extend_from_slice(extra);
        fixture.codemod(&args)
    };

    let output = resume(&["--tasks_ids", "abc"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("--tasks_ids was removed")
            && text(&output).contains("--approve <question-id>"),
        "{}",
        text(&output)
    );

    let output = resume(&["--dry-run"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("workflow run --dry-run"),
        "{}",
        text(&output)
    );

    // The run remembers its workflow and target: these still parse.
    let output = resume(&["-w", "workflow.yaml", "-t", "."]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("not found"), "{}", text(&output));
}

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
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
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

/// The events of a `--format jsonl` run.
fn events(output: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn run_finished(output: &Output) -> serde_json::Value {
    events(output)
        .into_iter()
        .rfind(|event| event["event"] == "run_finished")
        .unwrap_or_else(|| panic!("no run_finished in {}", text(output)))
}

/// Reads, never writes: one check that fails on src/a.ts's `var`.
const TS_CHECK: &str = r#"
import { fail, forEachFile, step } from "codemod/workflow";
export default async function main() {
  const found = await step("Find var", forEachFile("src/**/*.ts", (file) =>
    file.text.includes("var ") ? file.path : undefined));
  const report = { checks: [{ id: "no-var", status: found.outputs.length ? "fail" : "pass", files: found.outputs }] };
  if (found.outputs.length) fail("1 check failed: no-var", { output: report });
  return report;
}
"#;

#[test]
fn run_finished_carries_the_output_and_step_results() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  const n = await step("Count", () => 2);
  return { checks: [{ id: "count", n }] };
}
"#,
    );
    for extra in [&[][..], &["--dry-run"][..]] {
        let mut args = vec!["--format", "jsonl"];
        args.extend_from_slice(extra);
        let output = fixture.run(&args);
        assert!(output.status.success(), "{}", text(&output));
        let finished = run_finished(&output);
        assert_eq!(
            finished["output"],
            serde_json::json!({ "checks": [{ "id": "count", "n": 2 }] }),
            "{extra:?}: {finished}"
        );
        assert_eq!(
            finished["steps"],
            serde_json::json!([{ "position": 0, "label": "Count", "status": "succeeded", "result": 2 }]),
            "{extra:?}: {finished}"
        );
    }
}

#[test]
fn a_huge_output_is_left_out_of_run_finished_and_says_so() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  const big = await step("Big", () => "x".repeat(2 * 1024 * 1024));
  return { big };
}
"#,
    );
    let output = fixture.run(&["--format", "jsonl", "--dry-run"]);
    assert!(output.status.success(), "{}", text(&output));
    let finished = run_finished(&output);
    assert_eq!(finished["output"], serde_json::Value::Null);
    assert!(
        finished["output_omitted"]["bytes"].as_u64().unwrap() > 2 * 1024 * 1024,
        "{finished}"
    );
    assert_eq!(finished["steps"][0]["result"], serde_json::Value::Null);
    assert!(
        finished["steps"][0]["result_omitted"]["bytes"]
            .as_u64()
            .unwrap()
            > 2 * 1024 * 1024
    );
}

#[test]
fn a_text_run_ends_with_its_output() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        "export default async function main() { return { ok: true, n: 1 }; }\n",
    );
    for extra in [&[][..], &["--dry-run"][..]] {
        let output = fixture.run(extra);
        assert!(output.status.success(), "{}", text(&output));
        assert!(
            text(&output).contains(r#"Output: {"ok":true,"n":1}"#),
            "{extra:?}: {}",
            text(&output)
        );
    }
}

#[test]
fn fail_ends_the_run_with_exit_2_and_keeps_the_output() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.ts", TS_CHECK);
    let report = serde_json::json!({
        "checks": [{ "id": "no-var", "status": "fail", "files": ["src/a.ts"] }]
    });

    for extra in [&[][..], &["--dry-run"][..]] {
        let mut args = vec!["--format", "jsonl"];
        args.extend_from_slice(extra);
        let output = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2), "{}", text(&output));
        let finished = run_finished(&output);
        assert_eq!(finished["status"], "failed", "{finished}");
        assert_eq!(finished["error"], "1 check failed: no-var", "{finished}");
        assert_eq!(finished["output"], report, "{finished}");
    }

    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    let shown = text(&output);
    assert!(shown.contains("1 check failed: no-var"), "{shown}");
    assert!(shown.contains(r#"Output: {"checks":"#), "{shown}");

    let id = latest_run_id(&fixture);
    let status = fixture.codemod(&["workflow", "status", "--id", &id, "--format", "json"]);
    assert!(status.status.success(), "{}", text(&status));
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["id"], id.as_str());
    assert_eq!(status["status"], "failed");
    assert_eq!(status["error"], "1 check failed: no-var");
    assert_eq!(status["exitCode"], 2);
    assert_eq!(status["output"], report);
    assert_eq!(status["steps"][0]["label"], "Find var");
    assert_eq!(status["steps"][0]["status"], "succeeded");
    assert_eq!(
        status["steps"][0]["result"]["outputs"],
        serde_json::json!(["src/a.ts"])
    );
    assert!(status["durationMs"].as_u64().is_some(), "{status}");
}

#[test]
fn fail_takes_an_exit_code() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        &TS_CHECK.replace("{ output: report }", "{ output: report, exitCode: 3 }"),
    );
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
}

#[test]
fn workflow_list_and_status_print_json() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step } from "codemod/workflow";
export default async function main() { return await step("One", () => 1); }
"#,
    );
    let output = fixture.run(&[]);
    assert!(output.status.success(), "{}", text(&output));

    let list = fixture.codemod(&["workflow", "list", "--format", "json"]);
    assert!(list.status.success(), "{}", text(&list));
    let list: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let id = list[0]["id"].as_str().unwrap().to_string();
    assert_eq!(list[0]["status"], "succeeded");

    let status = fixture.codemod(&["workflow", "status", "--id", &id, "--format", "json"]);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["status"], "succeeded");
    assert_eq!(status["output"], 1);
    assert_eq!(status["exitCode"], serde_json::Value::Null);
    assert_eq!(status["steps"][0]["result"], 1);
}

/// A git repository with one commit and an uncommitted change.
fn dirty_repo(fixture: &Fixture) {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(fixture.target.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {}", text(&output));
    };
    git(&["init", "-q"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "init",
    ]);
    fixture.target_file("src/c.ts", "const c = 3;\n");
}

/// `workflow run` without `--allow-dirty`.
fn run_strict(fixture: &Fixture, extra: &[&str]) -> Output {
    let package = fixture.package.path().to_str().unwrap().to_string();
    let target = fixture.target.path().to_str().unwrap().to_string();
    let mut args = vec![
        "workflow",
        "run",
        "-w",
        &package,
        "--target",
        &target,
        "--no-interactive",
    ];
    args.extend_from_slice(extra);
    fixture.codemod(&args)
}

#[test]
fn a_run_that_only_reads_is_not_stopped_by_uncommitted_changes() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.ts", TS_CHECK);
    dirty_repo(&fixture);
    let output = run_strict(&fixture, &[]);
    // The check's own failure, not the dirty tree's.
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        !text(&output).contains("uncommitted changes"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_run_that_writes_stops_at_uncommitted_changes_before_writing() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { forEachFile, step } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  await step("Look", forEachFile("src/**/*.ts", (file) => file.path));
  await step("Replace var", forEachFile("src/**/*.ts", replace));
}
"#,
    );
    dirty_repo(&fixture);
    let output = run_strict(&fixture, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("uncommitted changes") && text(&output).contains("--allow-dirty"),
        "{}",
        text(&output)
    );
    // Said once, on the step's line; the run's ending only names the step.
    assert_eq!(
        text(&output).matches("uncommitted changes").count(),
        1,
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("The run failed: step 'Replace var' failed (see above)"),
        "{}",
        text(&output)
    );
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");

    let output = run_strict(&fixture, &["--allow-dirty"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
}

#[test]
fn a_shell_step_counts_as_a_write() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { shell, step } from "codemod/workflow";
export default async function main() {
  await step("Touch", shell("echo hi > touched.txt"));
}
"#,
    );
    dirty_repo(&fixture);
    let output = run_strict(&fixture, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("uncommitted changes"),
        "{}",
        text(&output)
    );
    assert!(!fixture.target.path().join("touched.txt").exists());
}

#[test]
fn resuming_a_run_stopped_at_uncommitted_changes_checks_again() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { forEachFile, step } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export default async function main() {
  await step("Replace var", forEachFile("src/**/*.ts", replace));
}
"#,
    );
    dirty_repo(&fixture);
    let output = run_strict(&fixture, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));

    let id = latest_run_id(&fixture);
    let output = fixture.codemod(&["workflow", "resume", "--id", &id, "--no-interactive"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("uncommitted changes"),
        "{}",
        text(&output)
    );
    assert_eq!(fixture.read("src/a.ts"), "var a = 1;\n");

    let output = fixture.codemod(&[
        "workflow",
        "resume",
        "--id",
        &id,
        "--no-interactive",
        "--allow-dirty",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("src/a.ts"), "let a = 1;\n");
}

#[test]
fn a_git_step_without_managed_git_does_not_stop_at_uncommitted_changes() {
    let fixture = Fixture::new();
    fixture.package_file(
        "workflow.ts",
        r#"
import { step } from "codemod/workflow";
import { branch } from "codemod/git";
export default async function main() {
  await step(branch("codemod/x"));
  return "ok";
}
"#,
    );
    dirty_repo(&fixture);
    let output = run_strict(&fixture, &[]);
    assert!(output.status.success(), "{}", text(&output));
}

#[cfg(unix)]
#[test]
fn missing_current_directory_is_reported_without_panicking() {
    let fixture = Fixture::new();
    fixture.package_file("workflow.ts", "export default function main() {}");
    let cwd = TempDir::new().unwrap();
    let output = Command::new("sh")
        .args([
            "-c",
            "cd \"$1\" && rmdir \"$1\" && shift && exec \"$@\"",
            "sh",
        ])
        .arg(cwd.path())
        .arg(env!("CARGO_BIN_EXE_codemod"))
        .args(["workflow", "run", "--no-interactive", "-w"])
        .arg(fixture.package.path().join("workflow.ts"))
        .env("DISABLE_ANALYTICS", "true")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("the current directory is unavailable"));
    assert!(!text(&output).contains("panicked"));
}
