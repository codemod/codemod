//! Steps and callbacks in their own isolates, and what the workflow body may use.

mod common;

use codemod_workflow::events::Event;
use codemod_workflow::{load_workflow, StepStatus};
use common::{output, Harness};
use serde_json::{json, Value};

/// Load a workflow that should fail to build; returns the message.
fn load_error(h: &Harness, source: &str) -> String {
    let path = h.bundle.path().join("workflow.ts");
    std::fs::write(&path, source).unwrap();
    match load_workflow(&path) {
        Ok(_) => panic!("expected the workflow not to load"),
        Err(error) => error.to_string(),
    }
}

// ------------------------------------------------------- forEachFile ----

#[test]
fn callbacks_can_be_async_and_run_concurrently() {
    let h = Harness::new();
    for name in ["c", "a", "b", "d"] {
        h.write(&format!("{name}.txt"), name);
    }
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
export default async function main() {
  return step(forEachFile({ include: "*.txt", concurrency: 4 }, async (file) => {
    await sleep(5);
    file.write(file.text.toUpperCase());
    return file.path;
  }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(
        result["outputs"],
        json!(["a.txt", "b.txt", "c.txt", "d.txt"])
    );
    assert_eq!(h.read("b.txt"), "B");
}

#[test]
fn parse_from_codemod_ast_grep_takes_a_file_or_text() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  return step(forEachFile({ language: "typescript" }, (file) => {
    const fromFile = parse(file);
    const fromText = parse("let b = 2;");
    const legacy = parse("typescript", "var c = 3;");
    return {
      filename: fromFile.filename().endsWith("src/a.ts"),
      language: file.language,
      kinds: [fromFile, fromText, legacy].map((root) => root.root().child(0)?.kind()),
    };
  }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(
        result["outputs"][0],
        json!({
            "filename": true,
            "language": "typescript",
            "kinds": ["lexical_declaration", "lexical_declaration", "variable_declaration"],
        })
    );
}

#[test]
fn parse_text_without_a_language_fails_the_file_with_a_hint() {
    let h = Harness::new();
    h.write("a.txt", "x");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  return step(forEachFile("*.txt", (file) => { parse(file.text); }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    let error = result["failures"][0]["error"].as_str().unwrap();
    assert!(error.contains("parse(text) needs a language"), "{error}");
}

#[test]
fn metrics_declared_at_module_level_are_recorded_by_callbacks() {
    let h = Harness::new();
    h.write("a.ts", "foo(); foo();");
    h.write("b.ts", "foo();");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
import { parse } from "codemod:ast-grep";
import { useMetricAtom } from "codemod:metrics";

const calls = useMetricAtom("calls");

export default async function main() {
  const result = await step(forEachFile({ language: "typescript" }, (file) => {
    const found = parse(file).root().findAll({ rule: { pattern: "foo()" } });
    calls.increment({ file: file.path }, found.length);
  }));
  return result.metrics;
}
"#,
        Value::Null,
    );
    let mut entries = output(&report).unwrap()["calls"]
        .as_array()
        .unwrap()
        .clone();
    entries.sort_by_key(|entry| entry["cardinality"]["file"].as_str().unwrap().to_string());
    assert_eq!(
        Value::Array(entries),
        json!([
            { "cardinality": { "file": "a.ts" }, "count": 2 },
            { "cardinality": { "file": "b.ts" }, "count": 1 },
        ])
    );
    assert!(h.summary.metrics().contains_key("calls"));
}

#[test]
fn recording_a_metric_in_the_body_is_an_error() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { useMetricAtom } from "codemod:metrics";
const calls = useMetricAtom("calls");
export default async function main() {
  calls.increment();
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("metrics can only be recorded inside a step"),
        "{error}"
    );
}

#[test]
fn callbacks_can_set_state_with_the_jssg_api() {
    let h = Harness::new();
    h.write("a.ts", "");
    h.write("b.ts", "");
    let report = h.start(
        r#"
import { step, forEachFile, getState, setState, acquireLock } from "codemod:workflow";
export default async function main() {
  await step(forEachFile({ language: "typescript" }, (file) => {
    const release = acquireLock("seen");
    try {
      setState("seen", [...(getState("seen") ?? []), file.path].sort());
    } finally {
      release();
    }
  }));
  return getState("seen");
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["a.ts", "b.ts"]));
}

#[test]
fn callbacks_can_import_files_next_to_the_workflow() {
    let h = Harness::new();
    h.write("a.txt", "hello");
    h.bundle_file(
        "lib/shout.ts",
        "export function shout(text: string): string { return text.toUpperCase() + \"!\"; }\n",
    );
    h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
import { shout } from "./lib/shout.ts";
export default async function main() {
  await step(forEachFile("*.txt", (file) => { file.write(shout(file.text)); }));
}
"#,
        Value::Null,
    );
    assert_eq!(h.read("a.txt"), "HELLO!");
}

#[test]
fn without_a_selector_files_come_from_the_package_languages() {
    let h = Harness::new();
    h.write("a.ts", "");
    h.write("b.py", "");
    h.bundle_file("codemod.yaml", "targets:\n  languages: [\"python\"]\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
export default async function main() {
  return step(forEachFile((file) => file.path));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap()["outputs"], json!(["b.py"]));
}

#[test]
fn without_a_selector_or_package_languages_the_step_fails() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
export default async function main() {
  return step(forEachFile((file) => file.path));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("targets.languages"), "{error}");
}

// ---------------------------------------------------- function steps ----

#[test]
fn a_function_step_runs_in_an_isolate_with_captures_and_real_modules() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step } from "codemod:workflow";
import path from "path";
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
export default async function main({ params }: { params: { name: string } }) {
  const name = params.name;
  return step("Compute", async () => {
    await sleep(1);
    return { joined: path.join("a", name), realClock: Date.now() > 1_600_000_000_000 };
  });
}
"#,
        json!({ "name": "b" }),
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "joined": "a/b", "realClock": true })
    );
}

#[test]
fn assigning_to_an_outside_variable_in_a_function_step_is_a_build_error() {
    let h = Harness::new();
    let error = load_error(
        &h,
        r#"
import { step } from "codemod:workflow";
export default async function main() {
  let total = 0;
  await step(() => { total = 1; });
}
"#,
    );
    assert!(error.contains("assigns to 'total'"), "{error}");
    assert!(error.contains("function step"), "{error}");
}

#[test]
fn a_function_step_passed_by_name_fails_with_a_hint() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step } from "codemod:workflow";
const compute = () => 1;
export default async function main() {
  return step(compute);
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("write the function inline"), "{error}");
}

#[test]
fn changing_a_finished_function_step_is_nondeterministic() {
    let h = Harness::new();
    let first = h.start(
        r#"
import { step } from "codemod:workflow";
export default async function main() {
  await step("One", () => 1);
  await step("Two", () => { throw new Error("stop"); });
}
"#,
        Value::Null,
    );
    assert!(output(&first).is_err());
    let resumed = h.resume(
        &first,
        Some(
            r#"
import { step } from "codemod:workflow";
export default async function main() {
  await step("One", () => 2);
  await step("Two", () => "fixed");
}
"#,
        ),
    );
    let error = output(&resumed).unwrap_err();
    assert!(error.contains("differs from the recorded run"), "{error}");
}

// ------------------------------------------------------------- body ----

#[test]
fn the_body_clock_and_random_numbers_replay_identically() {
    let h = Harness::new();
    let source = |fixed: bool| {
        format!(
            r#"
import {{ step }} from "codemod:workflow";
export default async function main() {{
  const seed = Math.random();
  const id = crypto.randomUUID();
  const at = Date.now();
  await step("Remember", () => ({{ seed, id, at }}));
  await step("Then", () => {{ if (!{fixed}) throw new Error("stop"); return Date.now(); }});
  return {{ seed, id, at }};
}}
"#
        )
    };
    let first = h.start(&source(false), Value::Null);
    assert!(output(&first).is_err());
    let recorded = first.steps[0].result.clone().unwrap();

    let resumed = h.resume(&first, Some(&source(true)));
    assert_eq!(output(&resumed).unwrap(), recorded);
    assert_eq!(resumed.steps[1].status, StepStatus::Succeeded);
}

#[test]
fn the_body_can_use_pure_builtins_and_logs_to_the_run() {
    let h = Harness::new();
    let report = h.start(
        r#"
import path from "path";
import { format } from "util";
export default async function main() {
  console.log("hello", { from: "the body" });
  return format("%s", path.basename("/a/b.ts", ".ts"));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!("b"));
    let logged = h.events.take().into_iter().any(
        |event| matches!(event, Event::RunLog { line } if line == "hello {\"from\":\"the body\"}"),
    );
    assert!(logged);
}

#[test]
fn timers_in_the_body_fail_with_a_hint() {
    let h = Harness::new();
    let report = h.start(
        r#"
export default async function main() {
  await new Promise((resolve) => setTimeout(resolve, 1));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("setTimeout is not available in the workflow body"),
        "{error}"
    );
}

#[test]
fn using_an_effectful_builtin_in_the_body_is_a_build_error() {
    let h = Harness::new();
    let error = load_error(
        &h,
        r#"
import { readFileSync } from "fs";
export default async function main() {
  return readFileSync("x", "utf8");
}
"#,
    );
    assert!(
        error.contains("uses 'readFileSync' from 'fs' outside a step"),
        "{error}"
    );
}

#[test]
fn an_effectful_builtin_used_only_in_a_step_is_left_out_of_the_body() {
    let h = Harness::new();
    let built = codemod_workflow::build(
        "workflow.ts",
        r#"
import { step } from "codemod:workflow";
import os from "os";
export default async function main() {
  return step(() => os.platform().length > 0);
}
"#,
    )
    .unwrap();
    assert!(!built.main.contains("\"os\""), "{}", built.main);
    let report = h.start(
        r#"
import { step } from "codemod:workflow";
import os from "os";
export default async function main() {
  return step(() => os.platform().length > 0);
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(true));
}

#[test]
fn a_pure_package_works_in_the_body() {
    let h = Harness::new();
    h.bundle_file(
        "node_modules/slug/package.json",
        r#"{ "name": "slug", "type": "module", "main": "index.js" }"#,
    );
    h.bundle_file(
        "node_modules/slug/index.js",
        "import path from \"path\";\nexport default (text) => path.basename(text).toLowerCase().replace(/[^a-z]+/g, \"-\");\n",
    );
    let report = h.start(
        r#"
import slug from "slug";
export default async function main() {
  return slug("/x/Hello World");
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!("hello-world"));
}

#[test]
fn a_package_that_reaches_fs_cannot_be_used_in_the_body() {
    let h = Harness::new();
    h.bundle_file(
        "node_modules/reader/package.json",
        r#"{ "name": "reader", "type": "module", "main": "index.js" }"#,
    );
    h.bundle_file(
        "node_modules/reader/index.js",
        "export { read } from \"./read.js\";\n",
    );
    h.bundle_file(
        "node_modules/reader/read.js",
        "import { readFileSync } from \"node:fs\";\nexport const read = (p) => readFileSync(p, \"utf8\");\n",
    );
    let error = load_error(
        &h,
        r#"
import { read } from "reader";
export default async function main() {
  return read("x");
}
"#,
    );
    assert!(error.contains("the workflow body uses 'reader'"), "{error}");
    assert!(error.contains("read.js → node:fs"), "{error}");

    // Inside a step it is fine.
    let h = Harness::new();
    h.bundle_file(
        "node_modules/reader/package.json",
        r#"{ "name": "reader", "type": "module", "main": "index.js" }"#,
    );
    h.bundle_file(
        "node_modules/reader/index.js",
        "export const read = () => \"ok\";\nimport \"node:fs\";\n",
    );
    let workflow = h.workflow(
        r#"
import { step } from "codemod:workflow";
import { read } from "reader";
export default async function main() {
  return step(() => read());
}
"#,
    );
    assert!(workflow.built.body_imports.is_empty());
}

#[test]
fn importing_parse_from_codemod_workflow_points_to_codemod_ast_grep() {
    let h = Harness::new();
    let error = load_error(
        &h,
        r#"
import { parse } from "codemod:workflow";
export default async function main() {}
"#,
    );
    assert!(
        error.contains("import { parse } from \"codemod:ast-grep\""),
        "{error}"
    );
}
