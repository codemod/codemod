//! `forEachFile(selector, callback, reducer, initial)`: each file's output is
//! folded into one value as it arrives, in path order, and the step's result
//! carries that value instead of every output.

mod common;

use common::{output, Harness};
use serde_json::{json, Value};

#[test]
fn a_reducer_folds_outputs_in_path_order_instead_of_listing_them() {
    let h = Harness::new();
    for name in ["d", "b", "a", "c", "e"] {
        h.write(&format!("src/{name}.ts"), &format!("var {name} = 1;\n"));
    }
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const result = await step("Count", forEachFile(
    { include: "src/**/*.ts", concurrency: 3 },
    (file) => {
      file.write(file.text.replace("var", "let"));
      return { path: file.path, size: file.text.length };
    },
    (acc, output, path) => ({ paths: [...acc.paths, path], bytes: acc.bytes + output.size, seen: [...acc.seen, output.path] }),
    { paths: [], bytes: 0, seen: [] },
  ));
  return { keys: Object.keys(result).sort(), reduced: result.reduced, written: result.changes.written };
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    let paths = json!(["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts", "src/e.ts"]);
    assert_eq!(
        result["reduced"],
        json!({ "paths": paths, "bytes": 55, "seen": paths })
    );
    assert_eq!(result["keys"], json!(["changes", "failures", "reduced"]));
    assert_eq!(result["written"], paths);
    assert_eq!(h.read("src/a.ts"), "let a = 1;\n");
}

/// Many files over several workers, each in flight with others: the fold
/// still sees them in path order.
#[test]
fn the_fold_order_is_the_path_order_with_many_files_in_flight() {
    let h = Harness::new();
    let mut names: Vec<String> = (0..400).map(|i| format!("f{i:04}.txt")).collect();
    for name in &names {
        h.write(name, "x\n");
    }
    names.sort();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const { reduced } = await step(forEachFile(
    { include: "*.txt", concurrency: 4 },
    async (file) => {
      // Later files finish first.
      await new Promise((resolve) => setTimeout(resolve, 400 - Number(file.path.slice(1, 5))));
      return file.path;
    },
    (acc, path) => [...acc, path],
    [],
  ));
  return reduced;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(names));
}

#[test]
fn files_without_output_or_that_fail_are_not_folded() {
    let h = Harness::new();
    h.write("a.txt", "keep\n");
    h.write("b.txt", "skip\n");
    h.write("c.txt", "fail\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const result = await step(forEachFile("*.txt", (file) => {
    if (file.text.startsWith("fail")) throw new Error("no");
    if (file.text.startsWith("skip")) return undefined;
    return 1;
  }, (count, one) => count + one, 0));
  return result;
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result["reduced"], 1, "{result}");
    assert_eq!(result["failures"][0]["path"], "c.txt", "{result}");
}

/// A reducer that throws fails the step, and nothing its callbacks did is
/// written.
#[test]
fn a_failed_reducer_fails_the_step_and_leaves_no_edits() {
    let h = Harness::new();
    h.write("a.ts", "var a = 1;\n");
    h.write("b.ts", "var b = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Fold", forEachFile("*.ts", (file) => {
    file.write(file.text.replace("var", "let"));
    return file.path;
  }, (acc, path) => {
    if (path === "b.ts") throw new Error("cannot fold b");
    return acc + 1;
  }, 0));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("cannot fold b"), "{error}");
    assert_eq!(h.read("a.ts"), "var a = 1;\n");
    assert_eq!(h.read("b.ts"), "var b = 1;\n");
}

#[test]
fn a_reducer_result_that_is_not_json_fails_the_step() {
    let h = Harness::new();
    h.write("a.ts", "var a = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("*.ts", (file) => {
    file.write("let a = 1;\n");
    return 1;
  }, (acc, one) => () => one, 0));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("reducer"), "{error}");
    assert_eq!(h.read("a.ts"), "var a = 1;\n");
}

/// Reducers use outside values like callbacks do, can be async, and can be
/// imported from a file of the package.
#[test]
fn reducers_capture_values_await_and_can_be_imported() {
    let h = Harness::new();
    h.write("pkg-a/x.ts", "var x = 1;\nvar y = 2;\n");
    h.write("pkg-b/z.ts", "var z = 1;\n");
    h.bundle_file(
        "scripts/by-package.ts",
        r#"
import { packageOf } from "./helpers.ts";
export default async function byPackage(acc: Record<string, number>, count: number, path: string) {
  await Promise.resolve();
  const name = packageOf(path);
  return { ...acc, [name]: (acc[name] ?? 0) + count };
}
"#,
    );
    h.bundle_file(
        "scripts/helpers.ts",
        "export const packageOf = (path: string) => path.split(\"/\")[0];\n",
    );
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import byPackage from "./scripts/by-package.ts";
export default async function main() {
  const weight = 10;
  const imported = await step(forEachFile("**/*.ts", (file) => file.text.split("var").length - 1, byPackage, {}));
  const inline = await step(forEachFile("**/*.ts", (file) => file.text.split("var").length - 1, (acc, n) => acc + n * weight, 0));
  return { imported: imported.reduced, inline: inline.reduced };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "imported": { "pkg-a": 2, "pkg-b": 1 }, "inline": 30 })
    );
}

#[test]
fn a_reducer_runs_in_a_dry_run_and_nothing_is_written() {
    let h = Harness::new();
    h.write("a.ts", "var a = 1;\n");
    let report = h.dry_run(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const { reduced } = await step(forEachFile("*.ts", (file) => {
    file.write("let a = 1;\n");
    return 1;
  }, (acc, n) => acc + n, 41));
  return reduced;
}
"#,
    );
    assert_eq!(output(&report).unwrap(), json!(42));
    assert_eq!(h.read("a.ts"), "var a = 1;\n");
}

/// The reduced value is recorded with the step: a resumed run replays it
/// and does not fold again. A changed reducer is another step.
#[test]
fn a_resumed_run_replays_the_reduced_value() {
    let h = Harness::new();
    h.write("a.txt", "a\n");
    h.write("b.txt", "b\n");
    let workflow = |start: u32| {
        format!(
            r#"
import {{ step, forEachFile }} from "codemod/workflow";
export default async function main() {{
  const {{ reduced }} = await step("Fold", forEachFile("*.txt", (file) => {{
    file.write(file.text + "!");
    return file.path;
  }}, (acc, path) => [...acc, path], [{start}]));
  await step("Check", () => {{ throw new Error("not yet"); }});
  return reduced;
}}
"#
        )
    };
    let report = h.start(&workflow(0), Value::Null);
    assert!(output(&report).is_err());
    assert_eq!(h.read("a.txt"), "a\n!");

    // Replayed: the files are not changed again.
    let resumed = h.resume(&report, None);
    assert!(output(&resumed).is_err());
    assert_eq!(h.read("a.txt"), "a\n!");
    let record = resumed
        .steps
        .iter()
        .find(|step| step.label == "Fold")
        .and_then(|step| step.result.clone())
        .unwrap();
    assert_eq!(record["reduced"], json!([0, "a.txt", "b.txt"]));

    // Another initial value is another step: it runs again.
    let resumed = h.resume(&report, Some(&workflow(1)));
    assert!(output(&resumed).is_err());
    assert_eq!(h.read("a.txt"), "a\n!!");
}

#[test]
fn a_reducer_needs_an_initial_value_and_a_callback() {
    let h = Harness::new();
    h.write("a.ts", "var a = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("*.ts", (file) => 1, (acc, n) => acc + n));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("initial"), "{error}");
}

#[test]
fn ast_grep_rules_take_no_reducer() {
    let h = Harness::new();
    h.write("a.ts", "var a = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
const rules = astGrep({ id: "let", language: "typescript", rule: { pattern: "var $A = $B" }, fix: "let $A = $B" });
export default async function main() {
  await step(forEachFile("*.ts", rules, (acc, n) => acc + n, 0));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("astGrep(...) returns nothing per file"),
        "{error}"
    );
    assert_eq!(h.read("a.ts"), "var a = 1;\n");
}

#[test]
fn ast_grep_with_a_reducer_is_a_build_error() {
    let h = Harness::new();
    let path = h.bundle.path().join("workflow.ts");
    std::fs::write(
        &path,
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("*.ts", astGrep({ id: "x", language: "typescript", rule: { pattern: "a" } }), (acc, n) => acc + n, 0));
}
"#,
    )
    .unwrap();
    let error = codemod_workflow::load_workflow(&path)
        .expect_err("a build error")
        .to_string();
    assert!(
        error.contains("astGrep(...) returns nothing per file"),
        "{error}"
    );
}

#[test]
fn a_reducer_that_never_settles_does_not_keep_a_timed_out_step_running() {
    let h = Harness::new();
    h.write("a.txt", "x\n");
    let started = std::time::Instant::now();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Fold", forEachFile(
    "*.txt",
    (file) => { file.write("y\n"); return 1; },
    () => new Promise(() => {}),
    0,
  ), { timeout: 300 });
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("timed out") || error.contains("time"),
        "{error}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert_eq!(h.read("a.txt"), "x\n");
}

/// The step's key keeps an own `__proto__` key of the initial value, so a
/// changed one is another step.
#[test]
fn an_initial_value_that_differs_only_in_a_proto_key_is_another_step() {
    let h = Harness::new();
    h.write("a.txt", "a\n");
    let workflow = |start: u32| {
        format!(
            r#"
import {{ step, forEachFile }} from "codemod/workflow";
export default async function main() {{
  const {{ reduced }} = await step("Fold", forEachFile("*.txt", (file) => {{
    file.write(file.text + "!");
    return 1;
  }}, (acc, n) => acc, JSON.parse('{{"__proto__": {start}}}')));
  await step("Check", () => {{ throw new Error("not yet"); }});
  return reduced;
}}
"#
        )
    };
    let report = h.start(&workflow(0), Value::Null);
    assert!(output(&report).is_err());
    assert_eq!(h.read("a.txt"), "a\n!");
    let resumed = h.resume(&report, Some(&workflow(1)));
    assert!(output(&resumed).is_err());
    assert_eq!(h.read("a.txt"), "a\n!!");
}
