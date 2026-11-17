mod common;

use std::sync::Arc;

use codemod_workflow::events::Event;
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
fn a_shell_steps_output_lines_come_back_as_values() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main() {
  const run = await step(shell(
    'echo "count=2" >> "$CODEMOD_OUTPUT" && echo "items@=\"a\"" >> "$CODEMOD_OUTPUT" && echo "$GREETING"',
    { env: { GREETING: "hi" } },
  ));
  return run;
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result["output"].as_str().unwrap().trim(), "hi");
    assert_eq!(result["values"], json!({ "count": 2, "items": ["a"] }));
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
fn jssg_steps_apply_changes_and_renames() {
    let h = Harness::new();
    h.write("src/a.ts", "const x = 1;\n");
    h.write("src/b.ts", "const y = 2;\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
export default function transform(root: any, options: any) {
  const text = root.root().text();
  if (!text.includes("x")) return null;
  root.rename(root.filename().replace("a.ts", "renamed.ts"));
  return text.replace("1", options.params.value);
}
"#,
    );
    let report = h.start(
        r#"
import { step, jssg } from "codemod/workflow";
export default async function main() {
  const result = await step(jssg("./scripts/transform.ts", { include: "src/**/*.ts" }));
  return { result };
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
    assert_eq!(h.engine_summary_diffs(), 1);
}

#[test]
fn a_jssg_steps_returned_data_is_added_up_on_its_result_and_the_run() {
    let h = Harness::new();
    h.write("src/a.ts", "foo(); foo();\n");
    h.write("src/b.ts", "foo();\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
export default function transform(root: any) {
  const calls = root.root().findAll({ rule: { pattern: "foo()" } });
  return { data: { calls: calls.map(() => ({ name: "foo" })) } };
}
"#,
    );
    let report = h.start(
        r#"
import { step, jssg } from "codemod/workflow";
export default async function main() {
  return (await step(jssg("./scripts/transform.ts", { include: "src/**/*.ts" }))).data;
}
"#,
        Value::Null,
    );
    let calls = json!([{ "cardinality": { "name": "foo" }, "count": 3 }]);
    assert_eq!(output(&report).unwrap()["calls"], calls);
    assert_eq!(json!(h.summary.metrics()["calls"]), calls);
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
    let lines = step_logs(&h);
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

fn step_logs(h: &Harness) -> Vec<String> {
    h.events
        .take()
        .into_iter()
        .filter_map(|event| match event {
            codemod_workflow::events::Event::StepLog { line, .. } => Some(line),
            _ => None,
        })
        .collect()
}

#[test]
fn files_a_jssg_transform_fails_on_are_logged_with_their_errors() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 1;\n");
    h.write("src/b.ts", "const b = 2;\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
export default function transform(root: any) {
  if (root.relativeFilename().endsWith("a.ts")) throw new Error("cannot handle a");
  return null;
}
"#,
    );
    let report = h.start(
        r#"
import { step, jssg } from "codemod/workflow";
export default async function main() {
  return await step("Scan", jssg("./scripts/transform.ts", { include: "src/**/*.ts" }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result["failures"][0]["path"], "src/a.ts");
    assert_eq!(result["unmodified"], json!(1));
    assert_eq!(report.steps[0].status, StepStatus::Partial);
    assert_eq!(
        report.steps[0].error.as_deref(),
        Some("1 of 2 files failed")
    );

    let lines = step_logs(&h);
    let a = lines
        .iter()
        .position(|l| l == "src/a.ts")
        .expect("src/a.ts header");
    assert!(
        lines[a + 1].starts_with("  ") && lines[a + 1].contains("cannot handle a"),
        "{lines:#?}"
    );
    assert!(!lines.iter().any(|l| l == "src/b.ts"), "{lines:#?}");
}

#[test]
fn a_selector_that_cannot_be_read_is_reported_only_when_the_transform_runs() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 1;\n");
    h.bundle_file(
        "scripts/transform.ts",
        r#"
export function getSelector() { throw new Error("no selector today"); }
export default function transform() { return null; }
"#,
    );
    let workflow = |include: &str| {
        format!(
            r#"
import {{ step, jssg }} from "codemod/workflow";
export default async function main() {{
  return await step("Scan", jssg("./scripts/transform.ts", {{ include: "{include}" }}));
}}
"#
        )
    };

    // No file to run on: nothing was lost.
    let report = h.start(&workflow("elsewhere/**/*.ts"), Value::Null);
    output(&report).unwrap();
    let lines = step_logs(&h);
    assert!(!lines.iter().any(|l| l.contains("selector")), "{lines:#?}");

    let report = h.start(&workflow("src/**/*.ts"), Value::Null);
    assert_eq!(output(&report).unwrap()["unmodified"], json!(1));
    let lines = step_logs(&h);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("cannot read the transform's selector")
                && l.contains("no selector today")),
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

/// A transform that writes how its file was parsed over the file.
const MARK: &str = r#"
export default function transform(root: any, options: any) {
  return `${options.language}: ${root.root().text()}`;
}
"#;

#[test]
fn for_each_file_runs_a_jssg_transform_on_the_files_its_selector_picks() {
    let h = Harness::new();
    h.write("src/a.ts", "a\n");
    h.write("src/skip.ts", "skip\n");
    h.write("lib/b.ts", "b\n");
    h.bundle_file("scripts/mark.ts", MARK);
    let report = h.start(
        r#"
import { step, forEachFile, jssg } from "codemod/workflow";
export default async function main() {
  const selector = { include: "src/**/*.ts", exclude: "**/skip.ts", language: "tsx" };
  const one = await step("Selected", forEachFile(selector, jssg("scripts/mark.ts")));
  const transform = jssg("scripts/mark.ts", { include: "lib/**", language: "typescript" });
  const two = await step("Its own", forEachFile(transform));
  return [one.changes.written, two.changes.written];
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([["src/a.ts"], ["lib/b.ts"]])
    );
    assert_eq!(h.read("src/a.ts"), "Tsx: a\n");
    assert_eq!(h.read("src/skip.ts"), "skip\n");
    assert_eq!(h.read("lib/b.ts"), "TypeScript: b\n");
}

#[test]
fn for_each_file_gives_a_jssg_transform_the_package_language() {
    let h = Harness::new();
    h.write("a.py", "a\n");
    h.write("b.ts", "b\n");
    h.bundle_file("scripts/mark.ts", MARK);
    let workflow = r#"
import { step, forEachFile, jssg } from "codemod/workflow";
export default async function main() {
  return (await step(forEachFile(jssg("scripts/mark.ts")))).changes.written;
}
"#;
    // No manifest: TypeScript, as `jssg` on its own.
    assert_eq!(
        output(&h.start(workflow, Value::Null)).unwrap(),
        json!(["b.ts"])
    );

    h.bundle_file("codemod.yaml", "targets:\n  languages: [python]\n");
    assert_eq!(
        output(&h.start(workflow, Value::Null)).unwrap(),
        json!(["a.py"])
    );
    assert_eq!(h.read("a.py"), "Python: a\n");

    h.bundle_file(
        "codemod.yaml",
        "targets:\n  languages: [python, typescript]\n",
    );
    let error = output(&h.start(workflow, Value::Null)).unwrap_err();
    assert!(
        error.contains("codemod.yaml lists several (python, typescript)"),
        "{error}"
    );
}

#[test]
fn for_each_file_applies_ast_grep_rules_to_the_files_its_selector_picks() {
    let h = Harness::new();
    h.write("src/a.js", "var x = 1;\n");
    h.write("lib/b.js", "var y = 2;\n");
    h.bundle_file(
        "rules.yaml",
        "id: no-var\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n",
    );
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  const some = await step("Some", forEachFile("src/**", astGrep("rules.yaml")));
  const all = await step("All", forEachFile(astGrep("rules.yaml")));
  return [some.changes.written, all.changes.written];
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([["src/a.js"], ["lib/b.js"]])
    );
    assert_eq!(h.read("src/a.js"), "let x = 1;\n");
}

#[test]
fn for_each_file_refuses_what_a_transform_cannot_take() {
    let h = Harness::new();
    h.bundle_file(
        "rules.yaml",
        "id: r\nlanguage: javascript\nrule:\n  pattern: a\n",
    );
    let failed = |call: &str| {
        let source = format!(
            "import {{ step, forEachFile, jssg, astGrep, shell }} from \"codemod/workflow\";\nexport default async function main() {{ await step({call}); }}\n"
        );
        output(&h.start(&source, Value::Null)).unwrap_err()
    };
    for (call, message) in [
        (
            r#"forEachFile({ include: "a/**" }, jssg("t.ts", { include: "b/**" }))"#,
            "'include' is given twice, in the selector and in jssg(...)",
        ),
        (
            r#"forEachFile({ containing: "x" }, jssg("t.ts"))"#,
            "selector.containing is for callbacks, not for jssg(...)",
        ),
        (
            r#"forEachFile({ language: "tsx" }, astGrep("rules.yaml"))"#,
            "the rules of astGrep(...) name their languages",
        ),
        (
            r#"forEachFile(shell("true"))"#,
            "forEachFile takes a callback written inline as a function, jssg(...) or astGrep(...)",
        ),
    ] {
        let error = failed(call);
        assert!(error.contains(message), "{call}: {error}");
    }
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

const SOME_FILES_FAIL: &str = r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const some = await step("Some", forEachFile("*.ts", (file) => {
    if (file.path === "a.ts") throw new Error("cannot handle a");
    file.write(file.text.trim() + "x\n");
  }));
  await step("Next", () => { throw new Error("not yet"); });
  return some.failures.map((failure) => failure.path);
}
"#;

#[test]
fn a_for_each_file_step_is_partial_when_some_files_fail() {
    let h = Harness::new();
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    let first = h.start(SOME_FILES_FAIL, Value::Null);

    // The step finished with what the other files did, and the run went on.
    let some = &first.steps[0];
    assert_eq!(some.status, StepStatus::Partial);
    assert_eq!(some.error.as_deref(), Some("1 of 2 files failed"));
    assert_eq!(h.read("b.ts"), "bx\n");
    assert_eq!(first.steps[1].label, "Next");
    assert_eq!(first.steps.len(), 2);
    let events = h.events.take();
    assert!(events.iter().any(|event| matches!(
        event,
        Event::StepFinished { label, status: StepStatus::Partial, error: Some(error), .. }
            if label == "Some" && error == "1 of 2 files failed"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::StepLog { line, .. } if line == "a.ts"
    )));

    // A resume reuses the partial step instead of running it again.
    let second = h.resume(
        &first,
        Some(&SOME_FILES_FAIL.replace("throw new Error(\"not yet\");", "")),
    );
    assert_eq!(output(&second).unwrap(), json!(["a.ts"]));
    assert_eq!(h.read("b.ts"), "bx\n");
}

#[test]
fn a_for_each_file_step_fails_when_every_file_fails() {
    let h = Harness::new();
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("All", forEachFile("*.ts", (file) => { throw new Error(`cannot handle ${file.path}`); }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("the callback failed on every file; first error: a.ts:"),
        "{error}"
    );
    assert_eq!(report.steps[0].status, StepStatus::Failed);
}

#[test]
fn a_workflow_in_src_finds_its_files_and_its_package() {
    let h = Harness::new();
    h.bundle_file(
        "codemod.yaml",
        "schema_version: \"1.0\"\nname: p\nversion: 0.1.0\ntargets:\n  languages: [\"python\"]\n",
    );
    h.bundle_file("src/scripts/echo.sh", "printf '%s|' \"$@\"\n");
    h.write("a.py", "x = 1\n");
    h.write("b.ts", "x\n");
    let workflow = h.workflow_file(
        "src/workflow.ts",
        r#"
import { step, shell, forEachFile } from "codemod/workflow";
const here = import.meta.dirname;
export default async function main() {
  const echoed = await step(shell`sh ${import.meta.dirname}/scripts/echo.sh ${"two words"} ${["it's", 3]}`);
  // No selector: the package's languages, from the codemod.yaml above src/.
  const seen = await step(forEachFile((file) => ({
    path: file.path,
    dirname: import.meta.dirname,
    filename: import.meta.filename,
  })));
  return { here, url: import.meta.url, echoed: echoed.output, seen: seen.outputs };
}
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    let result = output(&report).unwrap();

    let src = std::fs::canonicalize(h.bundle.path().join("src")).unwrap();
    let src = src.to_string_lossy();
    assert_eq!(result["here"], json!(src));
    assert_eq!(result["url"], json!(format!("file://{src}/workflow.ts")));
    assert!(
        result["echoed"]
            .as_str()
            .unwrap()
            .contains("two words|it's|3|"),
        "{result}"
    );
    assert_eq!(
        result["seen"],
        json!([{ "path": "a.py", "dirname": src, "filename": format!("{src}/workflow.ts") }])
    );
}
