//! Semantic answers describe the workspace as it was when the step started:
//! a step sees the edits of the steps before it, the same in a real run and
//! in a dry run, and never its own.

mod common;

use common::{output, Harness};
use serde_json::{json, Value};

fn harness() -> Harness {
    let h = Harness::new();
    h.write("src/lib.ts", "export function helper() {}\n");
    h.write("src/a.ts", "import { helper } from \"./lib\";\nhelper();\n");
    h
}

/// Step 1 adds a second call to `helper` in a.ts; step 2 asks for its callers.
const CALLERS_AFTER_AN_EDIT: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
export default async function main() {
  await step("Add a call", forEachFile({ include: "src/a.ts" }, (file) => {
    file.write(file.text + "helper();\n");
  }));
  const found = await step("Callers", forEachFile({ include: "src/lib.ts", semanticAnalysis: "workspace" }, (file) => {
    const name = parse(file).root().find({ rule: { kind: "identifier", inside: { kind: "function_declaration" } } });
    return name.callers().length;
  }));
  return found.outputs;
}
"#;

/// Step 1 puts a line above the code in a.ts; step 2 asks a file-scope
/// question whose answer depends on byte offsets.
const DEFINITION_AFTER_AN_EDIT: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
export default async function main() {
  await step("Add a header", forEachFile({ include: "src/b.ts" }, (file) => {
    file.write("// a header that moves every offset\n" + file.text);
  }));
  const found = await step("Definition", forEachFile({ include: "src/b.ts", semanticAnalysis: "file" }, (file) => {
    const use = parse(file).root().find({ rule: { pattern: "console.log($A)" } }).getMatch("A");
    return use.definition()?.node.text() ?? null;
  }));
  return found.outputs;
}
"#;

#[test]
fn a_step_sees_the_previous_steps_edits() {
    let h = harness();
    assert_eq!(
        output(&h.start(CALLERS_AFTER_AN_EDIT, Value::Null)).unwrap(),
        json!([2])
    );
}

#[test]
fn a_dry_run_sees_the_previous_steps_edits() {
    let h = harness();
    assert_eq!(
        output(&h.dry_run(CALLERS_AFTER_AN_EDIT)).unwrap(),
        json!([2])
    );
}

#[test]
fn a_dry_run_file_scope_answer_uses_the_previous_steps_text() {
    let h = harness();
    h.write("src/b.ts", "const value = 1;\nconsole.log(value);\n");
    assert_eq!(
        output(&h.dry_run(DEFINITION_AFTER_AN_EDIT)).unwrap(),
        json!(["value"])
    );
}

#[test]
fn a_node_from_text_this_step_produced_throws() {
    let h = harness();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "src/lib.ts", semanticAnalysis: "workspace" }, (file) => {
    const edited = parse({ path: file.path, text: "// moved\n" + file.text, language: "typescript" });
    try {
      edited.root().find({ rule: { kind: "identifier", inside: { kind: "function_declaration" } } }).callers();
      return "answered";
    } catch (error) {
      return String(error.message ?? error);
    }
  }));
  return found.outputs;
}
"#,
        Value::Null,
    );
    let message = output(&report).unwrap()[0].as_str().unwrap().to_string();
    assert!(
        message.contains("changed in this step"),
        "unexpected answer: {message}"
    );
}

/// Step 1 changes a declared type; step 2 asks the checker for it. In a dry
/// run step 1's edit is only in memory, so the checker has to get it from
/// the run, not the disk. Needs a checker (`CODEMOD_TS_API`, set in CI).
#[test]
fn types_see_the_previous_steps_edits_in_a_dry_run() {
    if !std::env::var("CODEMOD_TS_API").is_ok_and(|v| !v.is_empty() && v != "off") {
        return;
    }
    let h = harness();
    h.write(
        "src/c.ts",
        "const value: number = 1;\nconsole.log(value);\n",
    );
    let workflow = r#"
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
export default async function main() {
  await step("Retype", forEachFile({ include: "src/c.ts" }, (file) => {
    file.write(file.text.replace("const value: number = 1;", "const value: string = \"1\";"));
  }));
  const found = await step("Type", forEachFile({ include: "src/c.ts", semanticAnalysis: { mode: "workspace", types: "ts7" } }, (file) => {
    return parse(file).root().find({ rule: { pattern: "console.log($A)" } }).getMatch("A").typeInfo();
  }));
  return found.outputs;
}
"#;
    assert_eq!(
        output(&h.start(workflow, Value::Null)).unwrap(),
        json!(["string"])
    );
    let h = harness();
    h.write(
        "src/c.ts",
        "const value: number = 1;\nconsole.log(value);\n",
    );
    assert_eq!(output(&h.dry_run(workflow)).unwrap(), json!(["string"]));
}
