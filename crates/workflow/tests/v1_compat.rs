//! Codemod 1.x compatibility: 1.x packages (a workflow.yaml) run unchanged,
//! with the 1.x transform APIs 2.0 removed (`codemod:metrics`, the state
//! functions of `codemod:workflow`); TypeScript workflows still don't get
//! them.

mod common;

use codemod_workflow::load_workflow;
use common::{output, Harness};
use serde_json::{json, Value};

const COUNT: &str = r#"
import { useMetricAtom } from "codemod:metrics";

const kinds = useMetricAtom("kinds");

export default function transform(root: any) {
  const text = root.root().text();
  kinds.increment({ kind: text.includes("OLD") ? "old" : "other", dropped: undefined });
  kinds.increment();
  if (!text.includes("OLD")) return null;
  return text.replace("OLD", "NEW");
}
"#;

#[test]
fn codemod_metrics_counts_add_up_into_the_run_output() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 'OLD';\n");
    h.write("src/b.ts", "const b = 'OLD';\n");
    h.write("src/c.ts", "const c = 'keep';\n");
    h.bundle_file("scripts/codemod.ts", COUNT);
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Count
        js-ast-grep:
          js_file: scripts/codemod.ts
          include: ["src/**/*.ts"]
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    let out = output(&report).unwrap();
    assert_eq!(h.read("src/a.ts"), "const a = 'NEW';\n");
    let mut kinds = out["metrics"]["kinds"].as_array().unwrap().clone();
    kinds.sort_by_key(|entry| entry["cardinality"].to_string());
    assert_eq!(
        kinds,
        [
            json!({ "cardinality": { "kind": "old" }, "count": 2 }),
            json!({ "cardinality": { "kind": "other" }, "count": 1 }),
            json!({ "cardinality": {}, "count": 3 }),
        ]
    );
    // The step's result carries them too (`workflow status --format json`).
    let step = report.steps[0].result.as_ref().unwrap();
    assert!(step["v1"]["metrics"]["kinds"].is_array(), "{step}");
}

#[test]
fn get_entries_sees_what_every_file_of_the_step_counted() {
    let h = Harness::new();
    for name in ["a", "b", "c", "d"] {
        h.write(&format!("src/{name}.ts"), "x\n");
    }
    h.write("src/zz.ts", "last\n");
    h.bundle_file(
        "scripts/codemod.ts",
        r#"
import { useMetricAtom } from "codemod:metrics";
const seen = useMetricAtom("seen");
export default function transform(root: any) {
  seen.increment({ file: root.filename().split("/").pop() });
  return null;
}
"#,
    );
    h.bundle_file(
        "scripts/report.ts",
        r#"
import { useMetricAtom } from "codemod:metrics";
export default function transform(root: any) {
  const total = useMetricAtom("seen").getEntries().length;
  return `files seen earlier in this step: ${total}\n`;
}
"#,
    );
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: See
        js-ast-grep:
          js_file: scripts/codemod.ts
          include: ["src/**/*.ts"]
      - name: Report
        js-ast-grep:
          js_file: scripts/report.ts
          include: ["src/zz.ts"]
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    let out = output(&report).unwrap();
    assert_eq!(out["metrics"]["seen"].as_array().unwrap().len(), 5);
    // Each step counts on its own, as each run of a 1.x transform did.
    assert_eq!(h.read("src/zz.ts"), "files seen earlier in this step: 0\n");
}

/// 1.x state: files of one step share it (with `acquireLock`), later steps
/// and shell steps read it.
#[cfg(not(windows))]
#[test]
fn transforms_share_state_with_the_step_later_steps_and_shell_steps() {
    let h = Harness::new();
    for index in 0..12 {
        h.write(
            &format!("src/f{index:02}.ts"),
            &format!("const f = {index};\n"),
        );
    }
    h.write("summary.txt", "TODO\n");
    h.bundle_file(
        "scripts/collect.ts",
        r#"
import { acquireLock, getState, setState } from "codemod:workflow";
export default function transform(root: any) {
  const release = acquireLock("files");
  try {
    const files = getState<string[]>("files") ?? [];
    files.push(root.filename().split("/").pop());
    setState("files", files);
  } finally {
    release();
  }
  setState("scratch", 1, false);
  return null;
}
"#,
    );
    h.bundle_file(
        "scripts/summary.ts",
        r#"
import { getState, unsetState } from "codemod:workflow";
export default function transform(root: any) {
  const files = getState<string[]>("files") ?? [];
  unsetState("files");
  if (getState("files") !== undefined) throw new Error("unset did not take");
  return `files: ${files.length}, scratch: ${getState("scratch")}\n`;
}
"#,
    );
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: collect
    name: Collect
    steps:
      - name: Collect
        js-ast-grep:
          js_file: scripts/collect.ts
          include: ["src/**/*.ts"]
      - name: Shell sees it
        run: echo "$CODEMOD_STATE_FILES"
  - id: summary
    name: Summary
    depends_on: [collect]
    steps:
      - name: Summary
        js-ast-grep:
          js_file: scripts/summary.ts
          include: ["summary.txt"]
      - name: Gone
        run: echo "files=[$CODEMOD_STATE_FILES]"
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("summary.txt"), "files: 12, scratch: undefined\n");
    let collected = &report.steps[0].result.as_ref().unwrap()["v1"]["state"]["set"]["files"];
    assert_eq!(collected.as_array().unwrap().len(), 12, "{collected}");
    let shell = report.steps[1].result.as_ref().unwrap()["stdout"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        shell.contains("f00.ts") && shell.contains("f11.ts"),
        "{shell}"
    );
    let gone = report.steps[3].result.as_ref().unwrap()["stdout"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(gone.contains("files=[]"), "{gone}");
}

#[test]
fn a_removed_step_output_function_says_so() {
    let h = Harness::new();
    h.write("a.ts", "x\n");
    h.bundle_file(
        "scripts/codemod.ts",
        r#"
import { getState, setStepOutput } from "codemod:workflow";
export default function transform() {
  getState("x");
  setStepOutput("x", "1");
  return null;
}
"#,
    );
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Apply
        js-ast-grep:
          js_file: scripts/codemod.ts
          include: ["a.ts"]
"#,
    );
    let error = output(&h.start_loaded(workflow, Value::Null, false)).unwrap_err();
    assert!(error.contains("setStepOutput was removed"), "{error}");
}

/// TypeScript workflows keep 2.0's rules: the 1.x modules are not there.
#[test]
fn typescript_workflows_do_not_get_the_1x_modules() {
    let h = Harness::new();
    h.write("a.ts", "x\n");
    h.bundle_file("scripts/codemod.ts", COUNT);
    let report = h.start(
        r#"
import { forEachFile, step } from "codemod/workflow";
import { jssgFile } from "codemod-internal:jssg-compat";
import * as count from "./scripts/codemod.ts";

export default async function main() {
  await step("Count", forEachFile({ include: "a.ts" }, (file) => jssgFile(count, file)));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("codemod:metrics"), "{error}");
    assert!(error.contains("was removed"), "{error}");

    let h = Harness::new();
    h.write("a.ts", "x\n");
    h.bundle_file(
        "scripts/state.ts",
        "import { getState } from \"codemod:workflow\";\nexport default function f() { return getState(\"x\") ?? null; }\n",
    );
    let report = h.start(
        r#"
import { forEachFile, step } from "codemod/workflow";
import read from "./scripts/state.ts";

export default async function main() {
  await step("Read", forEachFile({ include: "a.ts" }, () => read()));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("getState"), "{error}");
}

#[test]
fn container_runtimes_are_refused() {
    let h = Harness::new();
    for (kind, at) in [("docker", "node"), ("podman", "template")] {
        let source = if at == "node" {
            format!(
                "version: \"1\"\nnodes:\n  - id: boxed\n    name: Boxed\n    runtime:\n      type: {kind}\n      image: alpine\n    steps:\n      - name: Where\n        run: uname -s\n"
            )
        } else {
            format!(
                "version: \"1\"\ntemplates:\n  - id: boxed\n    name: Boxed\n    runtime:\n      type: {kind}\n      image: alpine\n    steps:\n      - name: Where\n        run: uname -s\nnodes:\n  - id: use\n    name: Use\n    steps:\n      - name: Use\n        use:\n          template: boxed\n"
            )
        };
        let path = h.bundle.path().join("workflow.yaml");
        std::fs::write(&path, source).unwrap();
        let error = load_workflow(&path).unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "{at} 'boxed' runs its steps in a {kind} container"
            )),
            "{error}"
        );
        assert!(error.contains("codemod 1.x"), "{error}");
    }
}

#[test]
fn install_skill_steps_are_skipped_in_unattended_runs() {
    let h = Harness::new();
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: skill
    name: Skill
    steps:
      - name: Install skill
        install-skill:
          package: some-package
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(
        report.steps[0].result.as_ref().unwrap()["skipped"],
        json!(true)
    );
}

/// 1.x gave transforms an `fs` over the target without the capability.
/// They keep reading and creating target files through the compatibility shim.
#[test]
fn transforms_read_and_create_target_files_with_legacy_fs() {
    let h = Harness::new();
    h.write("src/a.ts", "OLD\n");
    h.write("names.json", "{\"OLD\":\"NEW\"}");
    h.bundle_file(
        "scripts/read.ts",
        r#"
import fs from "fs";
import { readFile } from "node:fs/promises";
export default async function transform(root: any, options: any) {
  const names = JSON.parse(fs.readFileSync(options.targetDir + "/names.json", "utf8"));
  const again = JSON.parse(await readFile("names.json", "utf8"));
  if (again.OLD !== names.OLD) throw new Error("relative read differs");
  try { fs.existsSync("../outside"); throw new Error("escaped the target"); }
  catch (error) { if (!error.message.includes("inside the target")) throw error; }
  return root.root().text().replace("OLD", names.OLD);
}
"#,
    );
    h.bundle_file(
        "scripts/write.ts",
        r#"
import fs from "fs";
import { mkdir } from "node:fs/promises";
export default async function transform() {
  fs.mkdirSync("empty");
  if (fs.existsSync("empty")) throw new Error("mkdir changed the target");
  await new Promise((resolve, reject) => fs.mkdir("with/callback", (error) => error ? reject(error) : resolve()));
  await mkdir("with/promises", { recursive: true });
  await fs.promises.mkdir("with/default");
  try { await mkdir("../outside"); throw new Error("escaped the target"); }
  catch (error) { if (!error.message.includes("inside the target")) throw error; }
  fs.writeFileSync("with/promises/other.txt", "x");
  return null;
}
"#,
    );
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Read
        js-ast-grep:
          js_file: scripts/read.ts
          include: ["src/**/*.ts"]
      - name: Write
        js-ast-grep:
          js_file: scripts/write.ts
          include: ["src/**/*.ts"]
"#,
    );
    output(&h.start_loaded(workflow, Value::Null, false)).unwrap();
    assert_eq!(h.read("src/a.ts"), "NEW\n");
    assert_eq!(h.read("with/promises/other.txt"), "x");

    // TypeScript workflows do not get the compatibility shim.
    let h = Harness::new();
    h.write("a.ts", "x\n");
    h.bundle_file(
        "scripts/read.ts",
        "import fs from \"fs\";\nexport default function f() { return fs.existsSync(\"a.ts\"); }\n",
    );
    let report = h.start(
        r#"
import { forEachFile, step } from "codemod/workflow";
import read from "./scripts/read.ts";

export default async function main() {
  const result = await step("Read", forEachFile({ include: "a.ts" }, () => read()));
  return result.outputs;
}
"#,
        Value::Null,
    );
    assert!(output(&report).unwrap_err().contains("codemod/files"));
}
