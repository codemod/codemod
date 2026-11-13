mod common;

use std::sync::Arc;

use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{Ending, MemoryStore, RunStatus, Services};
use common::{output, Harness};
use serde_json::{json, Value};

fn approving() -> Harness {
    Harness::with(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: true }),
    )
}

#[test]
fn shell_steps_get_the_state_contract_and_write_state() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step, shell, getState, setState } from "codemod:workflow";
export default async function main() {
  setState("greeting", "hi");
  const run = await step(shell(
    'echo "seen=$(cat "$CODEMOD_STATE")" && echo "count=2" >> "$STATE_OUTPUTS" && echo "items@=\"a\"" >> "$STATE_OUTPUTS" && echo "$CODEMOD_STATE_GREETING $EXTRA"',
    { env: { EXTRA: "there" } },
  ));
  return { output: run.output, count: getState("count"), items: getState("items") };
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert!(
        result["output"]
            .as_str()
            .unwrap()
            .contains(r#"seen={"greeting":"hi"}"#),
        "{result}"
    );
    assert!(
        result["output"].as_str().unwrap().contains("\"hi\" there"),
        "{result}"
    );
    assert_eq!(result["count"], json!(2));
    assert_eq!(result["items"], json!(["a"]));
    assert_eq!(report.steps[0].state.as_ref().unwrap()["count"], json!(2));
}

#[test]
fn a_failing_shell_command_fails_the_step() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step, shell } from "codemod:workflow";
export default async function main() { await step("Fail", shell("echo nope && exit 3")); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("step 'Fail' failed"), "{error}");
}

#[test]
fn shell_steps_are_skipped_in_a_dry_run() {
    let h = Harness::new();
    let report = h.dry_run(
        r#"
import { step, shell } from "codemod:workflow";
export default async function main() { return step(shell("touch should-not-exist")); }
"#,
    );
    assert_eq!(output(&report).unwrap(), json!({ "skipped": true }));
    assert!(!h.exists("should-not-exist"));
}

const NEEDS_APPROVAL: &str = r#"
import { step, approval, forEachFile } from "codemod:workflow";
export default async function main() {
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "1"); }));
  await step(approval("Ship it?"));
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "2"); }));
}
"#;

#[test]
fn an_approval_nobody_can_give_pauses_the_run_until_resumed() {
    let store = Arc::new(MemoryStore::default());
    let h = Harness::with(
        store.clone(),
        Arc::new(NonInteractive { approve_all: false }),
    );
    h.write("a.txt", "a");

    let first = h.start(NEEDS_APPROVAL, Value::Null);
    assert_eq!(first.ending, Ending::AwaitingApproval("Ship it?".into()));
    assert_eq!(first.run.status, RunStatus::AwaitingApproval);
    assert_eq!(first.run.awaiting.as_deref(), Some("Ship it?"));
    assert_eq!(h.read("a.txt"), "a1");
    assert_eq!(first.steps.len(), 1);

    // Resume with approval, sharing the same store.
    let approver = Harness::with(store, Arc::new(NonInteractive { approve_all: true }));
    let second = approver.engine.resume(&first.run.id).unwrap();
    assert!(output(&second).is_ok(), "{:?}", second.ending);
    assert_eq!(h.read("a.txt"), "a12");
}

#[test]
fn canceling_a_run_that_waits_for_approval_marks_it_canceled() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let first = h.start(NEEDS_APPROVAL, Value::Null);
    h.engine.cancel(&first.run.id).unwrap();
    let run = h.store.get_run(&first.run.id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Canceled);
}

#[test]
fn jssg_steps_apply_changes_renames_and_state() {
    let h = Harness::new();
    h.write("src/a.ts", "const x = 1;\n");
    h.write("src/b.ts", "const y = 2;\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
import { setState } from "codemod:workflow";
export default function transform(root: any, options: any) {
  const text = root.root().text();
  if (!text.includes("x")) return null;
  setState("touched", root.filename());
  root.rename(root.filename().replace("a.ts", "renamed.ts"));
  return text.replace("1", options.params.value);
}
"#,
    );
    let report = h.start(
        r#"
import { step, jssg, getState } from "codemod:workflow";
export default async function main() {
  const result = await step(jssg("./scripts/transform.ts", { include: "src/**/*.ts" }));
  return { result, touched: getState("touched") };
}
"#,
        json!({ "value": "42" }),
    );
    let result = output(&report).unwrap();
    assert_eq!(h.read("src/renamed.ts"), "const x = 42;\n");
    assert!(!h.exists("src/a.ts"));
    assert_eq!(h.read("src/b.ts"), "const y = 2;\n");
    assert_eq!(
        result["result"]["changes"]["renamed"],
        json!([{ "from": "src/a.ts", "to": "src/renamed.ts" }])
    );
    assert_eq!(result["result"]["unmodified"], json!(1));
    assert!(
        result["touched"].as_str().unwrap().ends_with("a.ts"),
        "{result}"
    );
    assert_eq!(h.engine_summary_diffs(), 1);
}

#[test]
fn jssg_steps_have_file_scope_semantic_analysis_unless_turned_off() {
    let h = Harness::new();
    h.write("src/a.ts", "const value = 1;\nconsole.log(value);\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
export default function transform(root: any) {
  const use = root.root().find({ rule: { pattern: "console.log($A)" } });
  const definition = use.getMatch("A").definition();
  return `// defined: ${definition ? definition.node.text() : "unknown"}\n` + root.root().text();
}
"#,
    );
    let workflow = |options: &str| {
        format!(
            r#"
import {{ step, jssg }} from "codemod:workflow";
export default async function main() {{
  await step(jssg("./scripts/transform.ts", {{ include: "src/**/*.ts"{options} }}));
}}
"#
        )
    };

    h.start(&workflow(""), Value::Null);
    assert!(
        h.read("src/a.ts").starts_with("// defined: value"),
        "{}",
        h.read("src/a.ts")
    );

    h.write("src/a.ts", "const value = 1;\nconsole.log(value);\n");
    h.start(&workflow(r#", semanticAnalysis: "none""#), Value::Null);
    assert!(
        h.read("src/a.ts").starts_with("// defined: unknown"),
        "{}",
        h.read("src/a.ts")
    );
}

#[test]
fn ast_grep_steps_apply_rule_fixes() {
    let h = Harness::new();
    h.write("a.js", "var x = 1;\n");
    h.bundle_file(
        "rules.yaml",
        "id: no-var\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n",
    );
    let report = h.start(
        r#"
import { step, astGrep } from "codemod:workflow";
export default async function main() { return step(astGrep("rules.yaml")); }
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(h.read("a.js"), "let x = 1;\n");
    assert_eq!(result["changes"]["written"], json!(["a.js"]));
}

#[test]
fn shard_steps_group_files_and_feed_later_steps() {
    let h = Harness::new();
    for dir in ["one", "two"] {
        for n in 0..3 {
            h.write(&format!("src/{dir}/f{n}.ts"), "export {}\n");
        }
    }
    let report = h.start(
        r#"
import { step, shard } from "codemod:workflow";
export default async function main() {
  const shards = await step(shard({
    method: { type: "directory", maxFilesPerShard: 10 },
    target: "src",
    filePattern: "**/*.ts",
  }));
  return shards.map((s: any) => [s.name, s._meta_files.length]);
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 2, "{result}");
    assert!(result.to_string().contains("one"), "{result}");
}

#[test]
fn codemod_steps_run_a_local_package_and_detect_cycles() {
    let h = Harness::new();
    h.write("a.txt", "a");
    h.bundle_file(
        "child/workflow.ts",
        r#"
import { step, forEachFile } from "codemod:workflow";
export default async function main({ params }: any) {
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "!"); }));
  return params.from;
}
"#,
    );
    let report = h.start(
        r#"
import { step, codemod } from "codemod:workflow";
export default async function main() {
  return step(codemod("./child", { params: { from: "parent" } }));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!({ "output": "parent" }));
    assert_eq!(h.read("a.txt"), "a!");

    h.bundle_file(
        "loop/workflow.ts",
        r#"
import { step, codemod } from "codemod:workflow";
export default async function main() { return step(codemod("./")); }
"#,
    );
    let report = h.start(
        r#"
import { step, codemod } from "codemod:workflow";
export default async function main() { return step(codemod("./loop")); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("codemod dependency cycle"), "{error}");
}

#[test]
fn git_steps_are_skipped_unless_managed_git_is_on() {
    let h = approving();
    let report = h.start(
        r#"
import { step, branch, commit, pullRequest } from "codemod:workflow";
export default async function main() {
  return [
    await step(branch("codemod/test")),
    await step(commit("chore: test")),
    await step(pullRequest({ title: "test" })),
  ];
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([{ "skipped": true }, { "skipped": true }, { "skipped": true }])
    );
}

#[test]
fn shard_steps_can_be_disabled() {
    let h = Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            skip_shard_steps: true,
            ..Services::default()
        },
    );
    let report = h.start(
        r#"
import { step, shard } from "codemod:workflow";
export default async function main() {
  return step(shard({ method: { type: "directory", maxFilesPerShard: 1 }, filePattern: "**/*" }));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([]));
}
