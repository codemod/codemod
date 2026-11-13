mod common;

use std::sync::Arc;

use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{
    cancel_run, Canceled, Ending, MemoryStore, RunStatus, Services, StepStatus,
};
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
import { step, shell, getState, setState } from "codemod/workflow";
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
import { step, shell } from "codemod/workflow";
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
import { step, shell } from "codemod/workflow";
export default async function main() { return step(shell("touch should-not-exist")); }
"#,
    );
    assert_eq!(output(&report).unwrap(), json!({ "skipped": true }));
    assert!(!h.exists("should-not-exist"));
}

const NEEDS_APPROVAL: &str = r#"
import { step, approval, forEachFile } from "codemod/workflow";
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
    // The step that waits is on record, so `status` can name it.
    assert_eq!(first.steps.len(), 2);
    assert_eq!(first.steps[1].status, StepStatus::AwaitingApproval);

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
    assert_eq!(
        cancel_run(h.store.as_ref(), &first.run.id).unwrap(),
        Canceled::Marked
    );
    let run = h.store.get_run(&first.run.id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Canceled);
    assert!(run.awaiting.is_none());
    assert_eq!(
        cancel_run(h.store.as_ref(), &first.run.id).unwrap(),
        Canceled::AlreadyFinished(RunStatus::Canceled)
    );
}

#[test]
fn jssg_steps_apply_changes_renames_and_state() {
    let h = Harness::new();
    h.write("src/a.ts", "const x = 1;\n");
    h.write("src/b.ts", "const y = 2;\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
import { setState } from "codemod/workflow";
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
import { step, jssg, getState } from "codemod/workflow";
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
fn jssg_transform_logs_become_step_logs_grouped_by_file() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 1;\n");
    h.write("src/b.ts", "const b = 2;\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
import { warn, setCurrentUnit } from "codemod/runtime";
export default function transform(root: any) {
  const name = root.relativeFilename();
  console.log(`visiting ${name}`);
  setCurrentUnit("declarations");
  warn(`found a const in ${name}`, { file: name });
  return null;
}
"#,
    );
    h.start(
        r#"
import { step, jssg } from "codemod/workflow";
export default async function main() {
  await step("Scan", jssg("./scripts/transform.ts", { include: "src/**/*.ts" }));
}
"#,
        Value::Null,
    );
    let lines: Vec<String> = h
        .events
        .take()
        .into_iter()
        .filter_map(|event| match event {
            codemod_workflow::events::Event::StepLog { line, .. } => Some(line),
            _ => None,
        })
        .collect();
    let a = lines
        .iter()
        .position(|l| l == "src/a.ts")
        .expect("src/a.ts header");
    assert_eq!(
        lines[a..a + 4],
        [
            "src/a.ts",
            "  visiting src/a.ts",
            "src/a.ts :: declarations",
            "  [warn] found a const in src/a.ts {\"file\":\"src/a.ts\"}",
        ],
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l == "src/b.ts :: declarations"),
        "{lines:#?}"
    );
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
import {{ step, jssg }} from "codemod/workflow";
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
import { step, astGrep } from "codemod/workflow";
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
import { step, shard } from "codemod/workflow";
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
import { step, forEachFile } from "codemod/workflow";
export default async function main({ params }: any) {
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "!"); }));
  return params.from;
}
"#,
    );
    let report = h.start(
        r#"
import { step, codemod } from "codemod/workflow";
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
import { step, codemod } from "codemod/workflow";
export default async function main() { return step(codemod("./")); }
"#,
    );
    let report = h.start(
        r#"
import { step, codemod } from "codemod/workflow";
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
import { step, branch, commit, pullRequest } from "codemod/workflow";
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
import { step, shard } from "codemod/workflow";
export default async function main() {
  return step(shard({ method: { type: "directory", maxFilesPerShard: 1 }, filePattern: "**/*" }));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([]));
}

#[test]
fn an_approval_the_body_swallows_still_pauses_the_run() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let report = h.start(
        r#"
import { step, approval, forEachFile } from "codemod/workflow";
export default async function main() {
  try {
    await step(approval("Ship it?"));
  } catch {}
  await step(forEachFile("*.txt", (file) => { file.write("shipped"); }));
  return "done";
}
"#,
        Value::Null,
    );
    assert_eq!(report.ending, Ending::AwaitingApproval("Ship it?".into()));
    assert_eq!(h.read("a.txt"), "a", "nothing runs past the gate");
}

#[test]
fn ast_grep_refuses_a_base_path_outside_the_target() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    h.bundle_file(
        "rules.yml",
        "id: r\nlanguage: typescript\nrule:\n  pattern: const a = 1\nfix: const a = 2\n",
    );
    let report = h.start(
        r#"
import { step, astGrep } from "codemod/workflow";
export default async function main() {
  await step("Scan", astGrep("rules.yml", { basePath: "../elsewhere" }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("must stay inside the target"), "{error}");
    assert_eq!(h.read("a.ts"), "const a = 1;\n");
}

#[test]
fn renames_are_compared_by_the_file_they_name() {
    let h = Harness::new();
    h.write("a.txt", "a");
    h.write("b.txt", "b");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  // Renaming a file to its own path, spelled differently, changes nothing.
  const same = await step("Same", forEachFile("a.txt", (file) => { file.rename("./a.txt"); }));
  return same.changes;
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "written": [], "renamed": [], "deleted": [] })
    );

    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Collide", forEachFile("*.txt", (file) => {
    file.rename(file.path === "a.txt" ? "./c.txt" : "c.txt");
  }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("both renamed to 'c.txt'"), "{error}");
    assert!(h.exists("a.txt") && h.exists("b.txt") && !h.exists("c.txt"));
}

#[test]
fn changes_several_files_make_to_one_file_add_up() {
    let h = Harness::new();
    for name in ["a", "b", "c", "d", "e", "f"] {
        h.write(
            &format!("src/parts/{name}.ts"),
            &format!("export const {name} = 1;\n"),
        );
    }
    h.write("src/index.ts", "// exports\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
import { jssgTransform } from "codemod/ast-grep";
export default async function transform(root: any) {
  const name = root.filename().split("/").pop().replace(".ts", "");
  const index = root.filename().replace(`parts/${name}.ts`, "index.ts");
  await jssgTransform(
    (other: any) => `${other.root().text()}export * from "./parts/${name}";\n`,
    index,
    "typescript",
  );
  return null;
}
"#,
    );
    let report = h.start(
        r#"
import { step, jssg } from "codemod/workflow";
export default async function main() {
  return step(jssg("./scripts/transform.ts", { include: "src/parts/*.ts" }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result["changes"]["written"], json!(["src/index.ts"]));
    let index = h.read("src/index.ts");
    for name in ["a", "b", "c", "d", "e", "f"] {
        assert!(index.contains(&format!("\"./parts/{name}\"")), "{index}");
    }
    assert_eq!(index.lines().count(), 7, "{index}");
}

#[test]
fn a_function_step_commits_what_it_changes_with_jssg_transform() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  await step("Edit", async () => {
    await jssgTransform((root) => root.root().text().replace("1", "2"), `${target}/a.ts`, "typescript");
    await jssgTransform((root) => root.root().text().replace("2", "3"), `${target}/a.ts`, "typescript");
  });
}
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("a.ts"), "const a = 3;\n");
    assert_eq!(h.engine_summary_diffs(), 1);
}

#[test]
fn for_each_file_selects_what_jssg_selects() {
    let h = Harness::new();
    // Hidden directories are searched, and a language means the same files
    // for both tasks.
    h.write(".github/workflows/ci.yml", "on: push\n");
    h.write("src/a.ts", "export {};\n");
    h.write("src/b.kt", "fun main() {}\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const hidden = await step("Hidden", forEachFile(".github/**/*.yml", (file) => file.path));
  const kotlin = await step("Kotlin", forEachFile({ language: "kotlin" }, (file) => file.path));
  return { hidden: hidden.outputs, kotlin: kotlin.outputs };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "hidden": [".github/workflows/ci.yml"], "kotlin": ["src/b.kt"] })
    );
}
