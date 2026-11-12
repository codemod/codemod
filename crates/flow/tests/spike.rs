use std::fs;
use std::path::Path;

use codemod_flow::{build, run, RunOptions, RunOutcome};
use serde_json::{json, Value};
use tempfile::TempDir;

fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

fn read(dir: &Path, path: &str) -> String {
    fs::read_to_string(dir.join(path)).unwrap()
}

fn execute(
    dir: &Path,
    source: &str,
    input: Value,
    history: Vec<Value>,
    dry_run: bool,
) -> RunOutcome {
    let built = build("workflow.ts", source).unwrap_or_else(|e| panic!("build failed: {e}"));
    run(
        &built,
        RunOptions {
            target: dir.to_path_buf(),
            input,
            history,
            history_path: None,
            dry_run,
        },
    )
    .unwrap()
}

fn build_error(source: &str) -> String {
    build("workflow.ts", source)
        .expect_err("expected a build error")
        .to_string()
}

// ---------------------------------------------------------------- build ----

#[test]
fn assigning_to_an_outside_variable_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  let count = 0;
  await step(forEachFile("**/*.ts", (file) => { count++; }));
}
"#,
    );
    assert!(error.contains("assigns to 'count'"), "{error}");
    assert!(error.starts_with("workflow.ts:5:"), "{error}");
}

#[test]
fn capturing_a_reassigned_local_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  let prefix = "a";
  prefix = "b";
  await step(forEachFile("**/*.ts", (file) => file.path.startsWith(prefix)));
}
"#,
    );
    assert!(
        error.contains("captures 'prefix', which is reassigned"),
        "{error}"
    );
}

#[test]
fn capturing_a_local_function_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  const shout = (s: string) => s.toUpperCase();
  await step(forEachFile("**/*.ts", (file) => { file.write(shout(file.text)); }));
}
"#,
    );
    assert!(error.contains("captures 'shout'"), "{error}");
}

#[test]
fn using_a_reassigned_module_binding_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod:flow";
let suffix = ".js";
suffix = ".mjs";
export default async function main() {
  await step(forEachFile("**/*.ts", (file) => { file.rename(file.path + suffix); }));
}
"#,
    );
    assert!(
        error.contains("'suffix', a module-level binding that is reassigned"),
        "{error}"
    );
}

#[test]
fn starting_a_step_inside_a_callback_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  await step(forEachFile("**/*.ts", (file) => { step(() => 1); }));
}
"#,
    );
    assert!(
        error.contains("uses 'step' inside a forEachFile callback"),
        "{error}"
    );
}

#[test]
fn a_callback_passed_by_name_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod:flow";
const touch = (file: any) => file.write(file.text);
export default async function main() {
  await step(forEachFile("**/*.ts", touch));
}
"#,
    );
    assert!(error.contains("must be written inline"), "{error}");
}

#[test]
fn module_level_helpers_are_copied_into_the_callback_with_their_dependencies() {
    let built = build(
        "workflow.ts",
        r#"
import { step, forEachFile } from "codemod:flow";
const MARK = "// migrated";
const unused = 42;
function mark(text: string): string { return `${MARK}\n${text}`; }
export default async function main() {
  const extra = "!";
  await step(forEachFile("**/*.ts", (file) => { file.write(mark(file.text) + extra); }));
}
"#,
    )
    .unwrap();
    assert_eq!(built.callbacks.len(), 1);
    let callback = built.callbacks.values().next().unwrap();
    assert!(callback.contains("const MARK"), "{callback}");
    assert!(callback.contains("function mark"), "{callback}");
    assert!(!callback.contains("unused"), "{callback}");
    assert!(callback.contains("extra"), "{callback}");
    assert!(built.main.contains("() => ({ extra })"), "{}", built.main);
}

// ------------------------------------------------------------------ run ----

#[test]
fn renames_files_without_parsing_them() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.less", ".a {}");
    write(dir.path(), "src/nested/b.less", ".b {}");
    write(dir.path(), "src/c.ts", "export {}");

    let outcome = execute(
        dir.path(),
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  const moved = await step("Rename LESS files", forEachFile("src/**/*.less", (file) => {
    file.rename(file.path.replace(/\.less$/, ".css"));
  }));
  return moved.changes;
}
"#,
        json!(null),
        vec![],
        false,
    );

    let changes = outcome.result.unwrap();
    assert_eq!(
        changes["renamed"],
        json!([
            { "from": "src/a.less", "to": "src/a.css" },
            { "from": "src/nested/b.less", "to": "src/nested/b.css" },
        ])
    );
    assert_eq!(read(dir.path(), "src/a.css"), ".a {}");
    assert!(!dir.path().join("src/a.less").exists());
    assert_eq!(outcome.history[0]["label"], "Rename LESS files");
}

const MIGRATE_MOMENT: &str = r#"
import { step, forEachFile, parse } from "codemod:flow";

const LEGACY = "moment";

function importsOf(root: any, name: string): any[] {
  return root.root().findAll({ rule: { pattern: `import $X from "${name}"` } });
}

export default async function main() {
  const usages = await step("Find moment imports",
    forEachFile({ language: "typescript", containing: LEGACY }, (file) => {
      const found = importsOf(parse(file), LEGACY);
      return found.length ? { path: file.path, count: found.length } : undefined;
    }),
  );

  const paths = usages.outputs.map((usage: { path: string }) => usage.path);

  await step(forEachFile({ language: "typescript" }, (file) => {
    if (!paths.includes(file.path)) return;
    const root = parse(file);
    const edits = importsOf(root, LEGACY).map((node: any) =>
      node.replace(`import ${node.getMatch("X").text()} from "dayjs";`),
    );
    file.write(root.root().commitEdits(edits));
  }));

  return { migrated: paths };
}
"#;

#[test]
fn a_later_step_captures_an_earlier_step_result_and_parses_files() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "src/a.ts",
        "import moment from \"moment\";\nmoment();\n",
    );
    write(dir.path(), "src/b.ts", "import other from \"other\";\n");
    write(
        dir.path(),
        "src/c.ts",
        "// mentions moment only in a comment\n",
    );

    let outcome = execute(dir.path(), MIGRATE_MOMENT, json!(null), vec![], false);

    assert_eq!(outcome.result.unwrap(), json!({ "migrated": ["src/a.ts"] }));
    assert_eq!(
        read(dir.path(), "src/a.ts"),
        "import moment from \"dayjs\";\nmoment();\n"
    );
    assert_eq!(
        read(dir.path(), "src/b.ts"),
        "import other from \"other\";\n"
    );
    // The second step's captures hold the first step's result.
    assert!(outcome.history[1]["key"]
        .as_str()
        .unwrap()
        .contains("src/a.ts"));
}

#[test]
fn a_dry_run_reports_changes_without_writing() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "import moment from \"moment\";\n");

    let outcome = execute(dir.path(), MIGRATE_MOMENT, json!(null), vec![], true);

    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(
        outcome.history[1]["result"]["changes"]["written"],
        json!(["src/a.ts"])
    );
    assert_eq!(
        read(dir.path(), "src/a.ts"),
        "import moment from \"moment\";\n"
    );
}

const TOUCH_THEN_CHECK: &str = r#"
import { step, forEachFile } from "codemod:flow";
export default async function main(input: { fail: boolean }) {
  await step("Touch", forEachFile("*.txt", (file) => { file.write(file.text + "+"); }));
  await step("Check", () => {
    if (input.fail) throw new Error("not yet");
    return "ok";
  });
}
"#;

#[test]
fn resuming_replays_finished_steps_and_retries_the_failed_one() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a");

    let first = execute(
        dir.path(),
        TOUCH_THEN_CHECK,
        json!({ "fail": true }),
        vec![],
        false,
    );
    let error = first.result.unwrap_err();
    assert!(error.contains("step 'Check' failed: not yet"), "{error}");
    assert_eq!(first.history[1]["status"], "failed");

    let second = execute(
        dir.path(),
        TOUCH_THEN_CHECK,
        json!({ "fail": false }),
        first.history,
        false,
    );
    assert!(second.result.is_ok(), "{:?}", second.result);
    // "Touch" was replayed, not run again.
    assert_eq!(read(dir.path(), "a.txt"), "a+");
    assert_eq!(second.history[1]["status"], "succeeded");
}

#[test]
fn fixing_the_step_that_failed_and_resuming_works() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a");
    let broken = TOUCH_THEN_CHECK.replace("if (input.fail) throw", "throw");

    let first = execute(dir.path(), &broken, json!({ "fail": false }), vec![], false);
    assert!(first.result.is_err());

    let second = execute(
        dir.path(),
        TOUCH_THEN_CHECK,
        json!({ "fail": false }),
        first.history,
        false,
    );
    assert!(second.result.is_ok(), "{:?}", second.result);
    assert_eq!(read(dir.path(), "a.txt"), "a+");
}

#[test]
fn changing_a_finished_step_before_resuming_is_a_nondeterminism_error() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a");

    let first = execute(
        dir.path(),
        TOUCH_THEN_CHECK,
        json!({ "fail": true }),
        vec![],
        false,
    );
    let changed = TOUCH_THEN_CHECK.replace("\"*.txt\"", "\"**/*.txt\"");
    let second = execute(
        dir.path(),
        &changed,
        json!({ "fail": false }),
        first.history,
        false,
    );

    let error = second.result.unwrap_err();
    assert!(error.contains("NondeterminismError"), "{error}");
    assert!(error.contains("step #0 ('Touch')"), "{error}");
}

#[test]
fn a_capture_that_is_not_json_fails_when_the_step_starts() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.ts", "");

    let outcome = execute(
        dir.path(),
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  const seen = new Set(["a.ts"]);
  await step(forEachFile("*.ts", (file) => seen.has(file.path)));
}
"#,
        json!(null),
        vec![],
        false,
    );

    let error = outcome.result.unwrap_err();
    assert!(
        error.contains("captures.seen is a Set, which is not JSON"),
        "{error}"
    );
    assert!(outcome.history.is_empty());
}

#[test]
fn mutating_a_capture_inside_a_callback_fails_that_file() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.ts", "");

    let outcome = execute(
        dir.path(),
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  const seen: string[] = [];
  return step(forEachFile("*.ts", (file) => { seen.push(file.path); }));
}
"#,
        json!(null),
        vec![],
        false,
    );

    let result = outcome.result.unwrap();
    assert_eq!(result["failures"][0]["path"], "a.ts");
    let error = result["failures"][0]["error"].as_str().unwrap();
    assert!(error.contains("TypeError"), "{error}");
    assert!(error.contains("are frozen"), "{error}");
}

#[test]
fn a_step_result_is_frozen_in_the_workflow() {
    let dir = TempDir::new().unwrap();
    let outcome = execute(
        dir.path(),
        r#"
import { step } from "codemod:flow";
export default async function main() {
  const result = await step(() => ({ items: [1, 2] }));
  try { result.items.push(3); } catch (e) { return "frozen"; }
  return "mutable";
}
"#,
        json!(null),
        vec![],
        false,
    );
    assert_eq!(outcome.result.unwrap(), json!("frozen"));
}

#[test]
fn steps_started_together_both_run() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a");
    write(dir.path(), "b.md", "b");

    let outcome = execute(
        dir.path(),
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  const [txt, md] = await Promise.all([
    step(forEachFile("*.txt", (file) => file.path)),
    step(forEachFile("*.md", (file) => file.path)),
  ]);
  return [...txt.outputs, ...md.outputs];
}
"#,
        json!(null),
        vec![],
        false,
    );
    assert_eq!(outcome.result.unwrap(), json!(["a.txt", "b.md"]));
    assert_eq!(outcome.history.len(), 2);
}

#[test]
fn conflicting_renames_fail_the_step_and_write_nothing() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a");
    write(dir.path(), "b.txt", "b");

    let outcome = execute(
        dir.path(),
        r#"
import { step, forEachFile } from "codemod:flow";
export default async function main() {
  await step(forEachFile("*.txt", (file) => { file.rename("same.txt"); }));
}
"#,
        json!(null),
        vec![],
        false,
    );
    let error = outcome.result.unwrap_err();
    assert!(error.contains("are both renamed to 'same.txt'"), "{error}");
    assert!(dir.path().join("a.txt").exists());
    assert!(!dir.path().join("same.txt").exists());
}

#[test]
fn the_history_file_is_written_after_each_step() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a");
    let history_file = dir.path().join("history.json");
    let built = build("workflow.ts", TOUCH_THEN_CHECK).unwrap();

    let outcome = run(
        &built,
        RunOptions {
            target: dir.path().to_path_buf(),
            input: json!({ "fail": true }),
            history: vec![],
            history_path: Some(history_file.clone()),
            dry_run: false,
        },
    )
    .unwrap();

    assert!(outcome.result.is_err());
    let saved: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(history_file).unwrap()).unwrap();
    assert_eq!(saved, outcome.history);
    assert_eq!(saved[0]["status"], "succeeded");
    assert_eq!(saved[1]["status"], "failed");
}
