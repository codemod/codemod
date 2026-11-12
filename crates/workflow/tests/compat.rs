//! workflow.yaml files run through the compatibility layer.

mod common;

use std::sync::Arc;

use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{Ending, MemoryStore, StepStatus};
use common::{output, Harness};
use serde_json::{json, Value};

const TRANSFORM: &str = r#"
import { setState, getState } from "codemod:workflow";
export default function transform(root: any, options: any) {
  const text = root.root().text();
  if (!text.includes("OLD")) return null;
  setState("seen", (getState("seen") ?? 0) + 1);
  const suffix = options.matrixValues?.suffix ?? "";
  return text.replace("OLD", options.params.replacement + suffix);
}
"#;

#[test]
fn a_yaml_workflow_runs_with_params_state_and_conditions() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 'OLD';\n");
    h.write("src/b.ts", "const b = 'OLD';\n");
    h.write("src/c.ts", "const c = 'keep';\n");
    h.bundle_file("scripts/codemod.ts", TRANSFORM);
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
params:
  schema:
    replacement:
      type: string
      default: NEW
nodes:
  - id: transform
    name: Transform
    steps:
      - name: Apply
        js-ast-grep:
          js_file: scripts/codemod.ts
          include: ["src/**/*.ts"]
  - id: follow-up
    name: Follow up
    depends_on: [transform]
    steps:
      - name: Runs when two files changed
        if: ${{ state.seen == 2 }}
        run: echo "done=true" >> "$STATE_OUTPUTS"
      - name: Never runs
        if: params.replacement == "other"
        run: exit 1
"#,
    );

    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("src/a.ts"), "const a = 'NEW';\n");
    assert_eq!(h.read("src/b.ts"), "const b = 'NEW';\n");
    let labels: Vec<&str> = report.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Apply", "Runs when two files changed"]);
    assert_eq!(report.steps[1].state.as_ref().unwrap()["done"], json!(true));
}

#[test]
fn matrix_values_run_once_each() {
    let h = Harness::new();
    h.write("one/a.ts", "x = 'OLD'\n");
    h.write("two/a.ts", "x = 'OLD'\n");
    h.bundle_file("scripts/codemod.ts", TRANSFORM);
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: each
    name: Each directory
    strategy:
      type: matrix
      values:
        - dir: one
          suffix: "-1"
        - dir: two
          suffix: "-2"
    steps:
      - name: Apply
        js-ast-grep:
          js_file: scripts/codemod.ts
          base_path: "${{ matrix.dir }}"
"#,
    );
    let report = h.start_loaded(workflow, json!({ "replacement": "NEW" }), false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("one/a.ts"), "x = 'NEW-1'\n");
    assert_eq!(h.read("two/a.ts"), "x = 'NEW-2'\n");
    assert_eq!(report.steps.len(), 2);
}

#[test]
fn shards_feed_a_matrix_from_state() {
    let h = Harness::new();
    for dir in ["alpha", "beta"] {
        h.write(&format!("src/{dir}/a.ts"), "y = 'OLD'\n");
        h.write(&format!("src/{dir}/b.ts"), "y = 'clean'\n");
    }
    h.bundle_file("scripts/codemod.ts", TRANSFORM);
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: evaluate
    name: Evaluate shards
    steps:
      - name: Build shards
        shard:
          method:
            type: directory
            max_files_per_shard: 10
          target: src
          output_state: shards
          js-ast-grep:
            js_file: scripts/codemod.ts
  - id: apply
    name: Apply
    depends_on: [evaluate]
    strategy:
      type: matrix
      from_state: shards
    steps:
      - name: Apply shard
        js-ast-grep:
          js_file: scripts/codemod.ts
"#,
    );
    let report = h.start_loaded(workflow, json!({ "replacement": "NEW" }), false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("src/alpha/a.ts"), "y = 'NEW'\n");
    assert_eq!(h.read("src/beta/a.ts"), "y = 'NEW'\n");
    let shards = report.steps[0].result.as_ref().unwrap().as_array().unwrap();
    assert_eq!(shards.len(), 2, "{shards:?}");
    // One step per shard, each limited to its shard's files.
    assert_eq!(report.steps.len(), 3);
    assert!(report
        .steps
        .iter()
        .all(|s| s.status == StepStatus::Succeeded));
}

#[test]
fn a_manual_node_waits_for_approval() {
    let store = Arc::new(MemoryStore::default());
    let h = Harness::with(
        store.clone(),
        Arc::new(NonInteractive { approve_all: false }),
    );
    h.write("a.txt", "a");
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: first
    name: First
    steps:
      - name: One
        run: echo one
  - id: gated
    name: Gated
    type: manual
    depends_on: [first]
    steps:
      - name: Two
        run: echo two
"#,
    );
    let first = h.start_loaded(workflow, Value::Null, false);
    assert_eq!(
        first.ending,
        Ending::AwaitingApproval("Run 'Gated'?".into())
    );

    let approver = Harness::with(store, Arc::new(NonInteractive { approve_all: true }));
    let second = approver.engine.resume(&first.run.id).unwrap();
    assert!(output(&second).is_ok(), "{:?}", second.ending);
    let labels: Vec<&str> = second.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["One", "approval: Run 'Gated'?", "Two"]);
}

#[test]
fn invalid_yaml_reports_the_validation_error() {
    let h = Harness::new();
    let path = h.bundle.path().join("workflow.yaml");
    std::fs::write(
        &path,
        "version: \"1\"\nnodes:\n  - id: a\n    name: A\n    depends_on: [missing]\n    steps: []\n",
    )
    .unwrap();
    let error = codemod_workflow::load_workflow(&path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing"), "{error}");
}
