//! `selectFiles(selector).filter(cb).map(cb)`: the prototype alternative to
//! `forEachFile(selector, cb)`.

mod common;

use codemod_workflow::{build, RunStatus};
use common::{output, Harness};
use serde_json::{json, Value};

fn build_error(source: &str) -> String {
    build("workflow.ts", source)
        .expect_err("expected a build error")
        .to_string()
}

fn moment_files(h: &Harness) {
    h.write("src/a.ts", "import moment from \"moment\";\nmoment();\n");
    h.write("src/b.ts", "export const b = 1;\n");
    h.write("src/c.ts", "import moment from \"moment\";\n");
}

// ------------------------------------------------------------------ run ----

#[test]
fn filter_then_map_inline_runs_the_map_only_on_kept_files() {
    let h = Harness::new();
    moment_files(&h);
    let report = h.start(
        r#"
import { step, selectFiles } from "codemod/workflow";
export default async function main() {
  const from = "moment";
  const result = await step("Migrate", selectFiles("src/**/*.ts")
    .filter((file) => file.text.includes(from))
    .map((file) => {
      file.write(file.text.replaceAll(`"${from}"`, '"dayjs"'));
      return file.path;
    }));
  return result;
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result["outputs"], json!(["src/a.ts", "src/c.ts"]));
    assert_eq!(result["changes"]["written"], json!(["src/a.ts", "src/c.ts"]));
    assert_eq!(result["failures"], json!([]));
    assert_eq!(h.read("src/a.ts"), "import moment from \"dayjs\";\nmoment();\n");
    assert_eq!(h.read("src/b.ts"), "export const b = 1;\n");
    assert_eq!(report.run.status, RunStatus::Succeeded);
}

#[test]
fn imported_filter_and_map_callbacks_work() {
    let h = Harness::new();
    moment_files(&h);
    h.bundle_file(
        "scripts/uses-moment.ts",
        "export default (file: any) => file.text.includes(\"moment\");\n",
    );
    h.bundle_file(
        "scripts/migrate.ts",
        "export default (file: any) => { file.write(file.text.replaceAll('\"moment\"', '\"dayjs\"')); return { path: file.path }; };\n",
    );
    let report = h.start(
        r#"
import { step, selectFiles } from "codemod/workflow";
import usesMoment from "./scripts/uses-moment.ts";
import migrate from "./scripts/migrate.ts";
export default async function main() {
  return step(selectFiles({ include: "src/**/*.ts" }).filter(usesMoment).map(migrate));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(
        result["outputs"],
        json!([{ "path": "src/a.ts" }, { "path": "src/c.ts" }])
    );
    assert_eq!(h.read("src/c.ts"), "import moment from \"dayjs\";\n");
}

#[test]
fn a_const_bound_selection_is_followed_and_array_map_is_left_alone() {
    let h = Harness::new();
    moment_files(&h);
    let report = h.start(
        r#"
import { step, selectFiles } from "codemod/workflow";
const all = selectFiles("src/**/*.ts");
export default async function main() {
  const withMoment = all.filter((file) => file.text.includes("moment"));
  const found = await step("Find", withMoment.map((file) => file.path));
  const everything = await step("All", all.map((file) => file.path.length));
  // Array#map on a step's result, and on a literal: plain JavaScript.
  const upper = found.outputs.map((path) => path.toUpperCase());
  const doubled = [1, 2].map((n) => n * 2);
  return { upper, doubled, count: everything.outputs.length };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "upper": ["SRC/A.TS", "SRC/C.TS"], "doubled": [2, 4], "count": 3 })
    );
}

#[test]
fn map_takes_ast_grep_rules() {
    let h = Harness::new();
    h.write("src/a.ts", "var x = 1;\n");
    let report = h.start(
        r#"
import { step, selectFiles, astGrep } from "codemod/workflow";
export default async function main() {
  const result = await step(selectFiles("src/**/*.ts").map(astGrep({
    id: "var", language: "typescript", rule: { pattern: "var $A = $B" }, fix: "let $A = $B",
  })));
  return result.matches;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(1));
    assert!(h.read("src/a.ts").starts_with("let x = 1"));
}

#[test]
fn a_filter_that_changes_its_file_fails_that_file() {
    let h = Harness::new();
    moment_files(&h);
    let report = h.start(
        r#"
import { step, selectFiles } from "codemod/workflow";
export default async function main() {
  return step(selectFiles("src/a.ts").filter((file) => { file.write("x"); return true; }).map((file) => file.path));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("filter callback cannot change files"), "{error}");
    assert_eq!(h.read("src/a.ts"), "import moment from \"moment\";\nmoment();\n");
}

/// The same task as forEachFile: the same step key, the same result.
#[test]
fn select_files_map_is_the_same_step_as_for_each_file() {
    let source = |call: &str| {
        format!(
            "import {{ step, forEachFile, selectFiles }} from \"codemod/workflow\";\nexport default async function main() {{\n  return step({call});\n}}\n"
        )
    };
    let h = Harness::new();
    moment_files(&h);
    let a = h.dry_run(&source("forEachFile(\"src/**\", (file) => file.path)"));
    let b = h.dry_run(&source("selectFiles(\"src/**\").map((file) => file.path)"));
    assert_eq!(output(&a).unwrap(), output(&b).unwrap());
    assert_eq!(a.steps[0].key, b.steps[0].key);
}

#[test]
fn on_resume_rerun_lists_the_files_again() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let source = |check: &str| {
        format!(
            r#"
import {{ step, selectFiles }} from "codemod/workflow";
export default async function main() {{
  const seen = await step("List", selectFiles("*.txt").map((file) => file.path), {{ onResume: "rerun" }});
  const replayed = await step("Replayed", selectFiles("*.txt").map((file) => file.path));
  await step("Check", () => {check});
  return {{ seen: seen.outputs, replayed: replayed.outputs }};
}}
"#
        )
    };
    let first = h.start(&source("{ throw new Error(\"not yet\"); }"), Value::Null);
    assert!(output(&first).is_err());
    h.write("b.txt", "b");
    let second = h.resume(&first, Some(&source("\"ok\"")));
    assert_eq!(
        output(&second).unwrap(),
        json!({ "seen": ["a.txt", "b.txt"], "replayed": ["a.txt"] })
    );
}

/// Nothing in the body lists files: the step's content is the selector and
/// the callbacks' ids, whatever the number of files.
#[test]
fn the_step_records_the_selector_not_the_files() {
    let h = Harness::new();
    for i in 0..200 {
        h.write(&format!("src/f{i}.ts"), "export {};\n");
    }
    let report = h.start(
        r#"
import { step, selectFiles } from "codemod/workflow";
export default async function main() {
  const r = await step(selectFiles("src/**").filter((file) => file.path.endsWith("7.ts")).map((file) => file.path));
  return r.outputs.length;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(20));
    let key = &report.steps[0].key;
    assert!(!key.contains("f17.ts"), "{key}");
    assert!(key.len() < 400, "{key}");
}

// -------------------------------------------------------- opaque value ----

/// The build refuses these shapes; reaching the runtime through a dynamic
/// import, the value still says it is not a list.
#[test]
fn the_selection_is_opaque_at_runtime() {
    let h = Harness::new();
    let report = h.start(
        r#"
export default async function main() {
  const wf: any = await import("codemod/workflow");
  const s = wf.selectFiles({ include: "src/**/*.ts", exclude: "dist/**" });
  const attempt = (f: () => unknown) => { try { f(); return "no error"; } catch (e: any) { return e.message; } };
  let looped = "no error";
  try { for (const _ of s) {} } catch (e: any) { looped = e.message; }
  let stepped = "no error";
  try { await wf.step(s); } catch (e: any) { stepped = e.message; }
  console.log(s);
  return {
    string: String(s),
    tag: Object.prototype.toString.call(s),
    json: attempt(() => JSON.stringify(s)),
    spread: attempt(() => [...s]),
    from: attempt(() => Array.from(s)),
    length: attempt(() => s.length),
    forEach: attempt(() => s.forEach(() => {})),
    looped,
    stepped,
    unextracted: attempt(() => s.map((file: any) => file.path)),
  };
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(
        result["string"],
        json!("[FileSelection src/**/*.ts (exclude dist/**)]")
    );
    assert_eq!(result["tag"], json!("[object FileSelection]"));
    for field in ["spread", "from", "length", "forEach", "looped"] {
        let message = result[field].as_str().unwrap();
        assert!(
            message.starts_with("selectFiles(...) is not a list of files"),
            "{field}: {message}"
        );
    }
    assert!(
        result["json"].as_str().unwrap().contains("is not data"),
        "{result}"
    );
    assert!(
        result["stepped"]
            .as_str()
            .unwrap()
            .contains("is not a task on its own"),
        "{result}"
    );
    assert!(
        result["unextracted"]
            .as_str()
            .unwrap()
            .contains("was not extracted by the build"),
        "{result}"
    );
}

// --------------------------------------------------------------- build ----

#[test]
fn shapes_the_build_cannot_follow_are_build_errors() {
    let cases: &[(&str, &str)] = &[
        // Passed to a helper.
        ("helper(selectFiles(\"**/*.ts\"));", "cannot be passed to helpers"),
        // Logged, spread, looped over in the body.
        ("console.log(selectFiles(\"**/*.ts\"));", "cannot be passed to helpers, logged"),
        ("const all = [...selectFiles(\"**/*.ts\")];", "use selectFiles(...) as the start of a chain"),
        ("for (const f of selectFiles(\"**/*.ts\")) {}", "use selectFiles(...) as the start of a chain"),
        // Not mapped.
        ("await step(selectFiles(\"**/*.ts\"));", "use selectFiles(...) as the start of a chain"),
        ("await step(selectFiles(\"**/*.ts\").filter((f) => true));", "use selectFiles(...) as the start of a chain"),
        // A const bound to one, used otherwise.
        ("const s = selectFiles(\"**/*.ts\"); helper(s);", "use selectFiles(...) as the start of a chain"),
        // let, and a conditional.
        ("let s = selectFiles(\"**/*.ts\"); await step(s.map((f) => f.path));", "bind selectFiles(...) with const, not let"),
        ("await step((flag ? selectFiles(\"a\") : selectFiles(\"b\")).map((f) => f.path));", "choose the selector, not the selection"),
        // A callback the build cannot extract.
        ("await step(selectFiles(\"**/*.ts\").map(local));", "must be written inline as a function, or imported"),
        ("await step(selectFiles(\"**/*.ts\").filter(local).map((f) => f.path));", "must be written inline as a function, or imported"),
        // Optional and computed calls.
        ("await step(selectFiles(\"**/*.ts\")?.map((f) => f.path));", "use selectFiles(...) as the start of a chain"),
        ("await step(selectFiles(\"**/*.ts\")[\"map\"]((f) => f.path));", "use selectFiles(...) as the start of a chain"),
        // Inside a callback.
        ("await step(() => selectFiles(\"x\").map((f) => f.path));", "uses 'selectFiles' inside a function step"),
    ];
    for (body, expected) in cases {
        let source = format!(
            "import {{ step, selectFiles }} from \"codemod/workflow\";\nconst local = (f: any) => f.path;\nfunction helper(x: unknown) {{}}\nexport default async function main(flag: boolean) {{\n  {body}\n}}\n"
        );
        let error = build_error(&source);
        assert!(error.contains(expected), "{body}\n  -> {error}");
        assert!(error.starts_with("workflow.ts:5:"), "{body}\n  -> {error}");
    }
}

#[test]
fn a_local_function_named_select_files_is_not_the_selection() {
    let built = build(
        "workflow.ts",
        r#"
import { step, forEachFile } from "codemod/workflow";
const selectFiles = (x: string) => [x];
export default async function main() {
  return selectFiles("a").map((s) => s.length);
}
"#,
    )
    .unwrap();
    assert!(built.callbacks.is_empty());
}

#[test]
fn a_renamed_import_is_followed() {
    let built = build(
        "workflow.ts",
        r#"
import { step, selectFiles as files } from "codemod/workflow";
export default async function main() {
  await step(files("a").filter((f) => true).map((f) => f.path));
}
"#,
    )
    .unwrap();
    assert_eq!(built.callbacks.len(), 2, "{}", built.main);
    assert_eq!(built.main.matches("__flowCallback(\"cb_").count(), 2, "{}", built.main);
}
