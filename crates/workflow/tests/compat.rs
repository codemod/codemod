//! workflow.yaml files run through the compatibility layer.

mod common;

use std::sync::Arc;

use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{Ending, MemoryStore, StepStatus};
use common::{output, Harness};
use serde_json::{json, Value};

const TRANSFORM: &str = r#"
export default function transform(root: any, options: any) {
  const text = root.root().text();
  if (!text.includes("OLD")) return null;
  const suffix = options.matrixValues?.suffix ?? "";
  return text.replace("OLD", options.params.replacement + suffix);
}
"#;

#[test]
fn two_plain_yaml_shell_steps_use_the_native_shell_without_a_state_wrapper() {
    let h = Harness::new();
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: echo
    name: Echo
    steps:
      - name: First
        run: echo first
      - name: Second
        run: echo second
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(report.steps.len(), 2);
    for (step, expected) in report.steps.iter().zip(["first", "second"]) {
        let stdout = &step.result.as_ref().unwrap()["stdout"];
        assert_eq!(stdout.as_str().unwrap().trim(), expected);
    }
}

#[cfg(windows)]
#[test]
fn yaml_shell_state_is_rejected_before_any_step_runs_on_windows() {
    let h = Harness::new();
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: stateful
    name: Stateful
    steps:
      - name: First
        run: echo first
      - name: Record
        run: echo seen=true >> "%STATE_OUTPUTS%"
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    let error = output(&report).unwrap_err();
    assert!(error.contains("not supported on Windows"), "{error}");
    assert!(error.contains("$STATE_OUTPUTS"), "{error}");
    assert!(report.steps.is_empty(), "{:?}", report.steps);
}

#[cfg(not(windows))]
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
      - name: Count
        run: echo "seen=2" >> "$STATE_OUTPUTS"
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
    assert_eq!(labels, ["Apply", "Count", "Runs when two files changed"]);
}

/// The YAML engine skipped a step whose condition could not be evaluated,
/// such as one reading a parameter left unset (its default is null). Packages
/// rely on that, so the condition must not be read as true.
#[test]
fn a_condition_that_cannot_be_evaluated_skips_its_step() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
params:
  schema:
    customName:
      type: string
      default: null
nodes:
  - id: only
    name: Only
    steps:
      - name: Needs a custom name
        if: params.customName != "null" && params.customName != ""
        run: echo unexpected
      - name: Always
        run: echo always
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let labels: Vec<&str> = report.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Always"]);
}

/// A node whose only use of the params is handing them to a template.
#[cfg(not(windows))]
#[test]
fn a_node_that_only_uses_a_template_still_gets_the_params() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
templates:
  - id: greet
    name: Greet
    steps:
      - name: Say it
        run: echo "hello ${{ params.who }}" >> "$STATE_OUTPUTS.log"; echo "said=${{ params.who }}" >> "$STATE_OUTPUTS"
nodes:
  - id: only
    name: Only
    steps:
      - name: Use the template
        use:
          template: greet
          inputs:
            who: world
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let stdout = report.steps[0].result.as_ref().unwrap()["stdout"]
        .as_str()
        .unwrap();
    assert!(stdout.ends_with("said=world\n"), "{stdout}");
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
      - name: Apply in ${{ matrix.dir }}
        js-ast-grep:
          js_file: scripts/codemod.ts
          base_path: "${{ matrix.dir }}"
"#,
    );
    let report = h.start_loaded(workflow, json!({ "replacement": "NEW" }), false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("one/a.ts"), "x = 'NEW-1'\n");
    assert_eq!(h.read("two/a.ts"), "x = 'NEW-2'\n");
    let mut labels: Vec<&str> = report.steps.iter().map(|s| s.label.as_str()).collect();
    labels.sort();
    assert_eq!(labels, ["Apply in one", "Apply in two"]);
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

/// What a shell step writes to `$STATE_OUTPUTS` reaches a shell step in a
/// later node as `CODEMOD_STATE_<KEY>`, JSON-encoded, as in the YAML engine.
#[cfg(not(windows))]
#[test]
fn state_a_shell_step_writes_reaches_a_later_shell_step() {
    let h = Harness::new();
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: detect
    name: Detect
    steps:
      - name: Pick
        run: echo 'replacement=NEW' >> "$STATE_OUTPUTS"
  - id: apply
    name: Apply
    depends_on: [detect]
    steps:
      - name: Read
        run: echo "$CODEMOD_STATE_REPLACEMENT"
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let read = &report.steps[1].result.as_ref().unwrap()["stdout"];
    assert_eq!(printed(read), "\"NEW\"");
}

/// Runs a node of these YAML steps; what the last one printed, trimmed.
#[cfg(not(windows))]
fn last_printed(steps: &str) -> String {
    let h = Harness::new();
    let workflow = h.workflow_file(
        "workflow.yaml",
        &format!("version: \"1\"\nnodes:\n  - id: only\n    name: Only\n    steps:\n{steps}"),
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let last = report.steps.last().unwrap().result.as_ref().unwrap();
    printed(&last["stdout"])
}

/// What a translated step printed, without the state lines added after it.
#[cfg(not(windows))]
fn printed(stdout: &Value) -> String {
    let stdout = stdout.as_str().unwrap();
    let own = stdout.split("::codemod-state-outputs::").next().unwrap();
    own.trim().to_string()
}

#[cfg(not(windows))]
#[test]
fn a_script_with_its_own_interpreter_can_write_state() {
    let printed = last_printed(
        r#"      - name: Pick
        run: |
          #!/usr/bin/env python3
          import os
          open(os.environ["STATE_OUTPUTS"], "a").write("picked=yes\n")
      - name: Read
        run: echo "$CODEMOD_STATE_PICKED"
"#,
    );
    assert_eq!(printed, "\"yes\"");
}

#[cfg(not(windows))]
#[test]
fn a_script_with_its_own_interpreter_may_contain_any_line() {
    let printed = last_printed(
        r#"      - name: Pick
        run: |
          #!/usr/bin/env python3
          import os
          end = """
          CODEMOD_SCRIPT_END
          """
          open(os.environ["STATE_OUTPUTS"], "a").write("picked=yes\n")
      - name: Read
        run: echo "$CODEMOD_STATE_PICKED"
"#,
    );
    assert_eq!(printed, "\"yes\"");
}

#[cfg(not(windows))]
#[test]
fn a_param_cannot_end_a_script_with_its_own_interpreter_early() {
    let h = Harness::new();
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: only
    name: Only
    steps:
      - name: Pick
        run: |
          #!/usr/bin/env python3
          import os
          text = """${{ params.text }}"""
          open(os.environ["STATE_OUTPUTS"], "a").write("lines=%d\n" % len(text.splitlines()))
"#,
    );
    let params = json!({ "text": "hello\nCODEMOD_SCRIPT_END\nworld" });
    let report = h.start_loaded(workflow, params, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let stdout = report.steps[0].result.as_ref().unwrap()["stdout"]
        .as_str()
        .unwrap();
    assert!(stdout.ends_with("lines=3\n"), "{stdout}");
}

#[cfg(not(windows))]
#[test]
fn a_state_line_ending_in_crlf_keeps_its_value_without_the_cr() {
    let printed = last_printed(
        r#"      - name: Pick
        run: printf 'who=world\r\n' >> "$STATE_OUTPUTS"
      - name: Read
        run: echo "$CODEMOD_STATE_WHO"
"#,
    );
    assert_eq!(printed, "\"world\"");
}

#[cfg(not(windows))]
#[test]
fn a_state_line_is_split_at_its_first_equals_sign() {
    let printed = last_printed(
        r#"      - name: Note
        run: echo 'note=a@=b' >> "$STATE_OUTPUTS"
      - name: Read
        run: echo "$CODEMOD_STATE_NOTE"
"#,
    );
    assert_eq!(printed, "\"a@=b\"");
}

#[cfg(not(windows))]
#[test]
fn setting_state_to_null_removes_the_key() {
    let printed = last_printed(
        r#"      - name: Set
        run: echo 'flag=1' >> "$STATE_OUTPUTS"
      - name: Clear
        run: echo 'flag=null' >> "$STATE_OUTPUTS"
      - name: Read
        run: echo "[${CODEMOD_STATE_FLAG-unset}]"
"#,
    );
    assert_eq!(printed, "[unset]");
}

#[cfg(not(windows))]
#[test]
fn state_lines_longer_than_a_step_keeps_fail_the_run() {
    let h = Harness::new();
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: only
    name: Only
    steps:
      - name: Write
        run: yes 'key@=value' | head -n 200000 >> "$STATE_OUTPUTS"
      - name: Read
        run: echo "${CODEMOD_STATE_KEY-unset}" | head -c 20
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("more than 1 MiB to $STATE_OUTPUTS"),
        "{error}"
    );
}

#[cfg(not(windows))]
#[test]
fn a_multi_line_command_with_an_expression_keeps_its_lines_as_written() {
    let printed = last_printed(
        r#"      - name: Pick
        run: echo 'who=world' >> "$STATE_OUTPUTS"
      - name: Heredoc
        run: |
          cat <<END
          hello ${{ state.who }}
          END
"#,
    );
    assert!(printed.starts_with("hello world"), "{printed}");
}
