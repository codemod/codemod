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

/// The shard's transform runs as the YAML engine's prescan ran it: as a dry
/// run, so a transform that changes files only then is what picks them.
#[test]
fn a_shard_transform_sees_a_dry_run() {
    let h = Harness::new();
    h.write("src/a/a.ts", "y = 1\n");
    h.write("src/b/b.ts", "y = 2\n");
    h.bundle_file(
        "scripts/codemod.ts",
        r#"
export default function transform(root: any, options: any) {
  const text = root.root().text();
  if (!options.dryRun || !text.includes("1")) return null;
  return text.replace("1", "one");
}
"#,
    );
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
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let shards = report.steps[0].result.as_ref().unwrap();
    assert!(shards.to_string().contains("a/a.ts"), "{shards}");
    assert!(!shards.to_string().contains("b/b.ts"), "{shards}");
    assert_eq!(h.read("src/a/a.ts"), "y = 1\n");
}

/// The YAML names its rule file by path; the translation imports it.
#[test]
fn a_yaml_ast_grep_step_applies_its_imported_rule_file() {
    let h = Harness::new();
    h.write("src/a.js", "var x = 1;\nold();\n");
    h.bundle_file(
        "rules/config.yml",
        "id: no-var\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n---\nid: rename\nlanguage: javascript\nrule:\n  pattern: old()\nfix: fresh()\n",
    );
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Rules
        ast-grep:
          config_file: rules/config.yml
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("src/a.js"), "let x = 1;\nfresh();\n");
}

/// `base_path` and the globs under it select the same files as in the YAML
/// engine, also when the base comes from the matrix.
#[test]
fn a_yaml_ast_grep_step_keeps_its_base_path() {
    let h = Harness::new();
    for dir in ["one", "two", "three"] {
        h.write(&format!("{dir}/src/a.js"), "var x = 1;\n");
        h.write(&format!("{dir}/a.js"), "var y = 2;\n");
        h.write(&format!("{dir}/README.md"), "var z = 3;\n");
    }
    h.bundle_file(
        "rules/config.yml",
        "id: no-var\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n",
    );
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: fixed
    name: Fixed base
    steps:
      - name: Only src
        ast-grep:
          config_file: rules/config.yml
          base_path: one
          include: ["src/**"]
  - id: each
    name: Each directory
    strategy:
      type: matrix
      values:
        - dir: two
    steps:
      - name: Whole base
        ast-grep:
          config_file: rules/config.yml
          base_path: "${{ matrix.dir }}"
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("one/src/a.js"), "let x = 1;\n");
    assert_eq!(h.read("one/a.js"), "var y = 2;\n");
    assert_eq!(h.read("two/src/a.js"), "let x = 1;\n");
    assert_eq!(h.read("two/a.js"), "let y = 2;\n");
    assert_eq!(h.read("two/README.md"), "var z = 3;\n");
    assert_eq!(h.read("three/a.js"), "var y = 2;\n");
}

/// The YAML names its shard function by path; the translation imports it.
#[test]
fn a_yaml_shard_function_runs_from_its_import() {
    let h = Harness::new();
    for name in ["a", "b"] {
        h.write(&format!("src/{name}.ts"), "y = 'OLD'\n");
    }
    h.bundle_file(
        "scripts/shard.js",
        r#"
export default function shard(input) {
  return [{ name: "all", _meta_shard: 0, _meta_files: input.files }];
}
"#,
    );
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
            function: ./scripts/shard.js
          target: src
          file_pattern: "**/*.ts"
          output_state: shards
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let shards = report.steps[0].result.as_ref().unwrap();
    assert_eq!(
        shards[0]["_meta_files"].as_array().unwrap().len(),
        2,
        "{shards}"
    );
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
          #!/bin/sh
          echo "picked=yes" >> "$STATE_OUTPUTS"
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
          #!/bin/sh
          choice=yes
          cat > /dev/null <<'CODEMOD_SCRIPT_END'
          CODEMOD_SCRIPT_END
          echo "picked=$choice" >> "$STATE_OUTPUTS"
      - name: Read
        run: echo "$CODEMOD_STATE_PICKED"
"#,
    );
    assert_eq!(printed, "\"yes\"");
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

fn js_step(h: &Harness, transform: &str, step: &str) -> codemod_workflow::RunReport {
    h.bundle_file("scripts/codemod.ts", transform);
    let workflow = h.workflow_file(
        "workflow.yaml",
        &format!(
            r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Transform
        js-ast-grep:
          js_file: scripts/codemod.ts
{step}
"#
        ),
    );
    h.start_loaded(workflow, json!({ "replacement": "NEW" }), false)
}

/// What workflow.yaml's js-ast-grep step did with a transform's result, it
/// still does: a string replaces the file, `null` leaves it, `{ edit, data }`
/// edits it, and `root.rename()` moves it.
#[test]
fn a_yaml_transform_edits_and_renames_files_as_before() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 'OLD';\n");
    h.write("src/b.ts", "const b = 'OLD';\n");
    h.write("src/keep.ts", "const keep = 1;\n");
    let report = js_step(
        &h,
        r#"
export default function transform(root: any, options: any) {
  const text = root.root().text();
  if (!text.includes("OLD")) return null;
  const edit = text.replace("OLD", options.params.replacement);
  if (root.filename().endsWith("b.ts")) {
    root.rename("renamed.ts");
    return { edit, data: { renamed: true } };
  }
  return edit;
}
"#,
        "          include: [\"src/**/*.ts\"]",
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("src/a.ts"), "const a = 'NEW';\n");
    assert_eq!(h.read("src/renamed.ts"), "const b = 'NEW';\n");
    assert!(!h.exists("src/b.ts"));
    assert_eq!(h.read("src/keep.ts"), "const keep = 1;\n");
}

/// Files the transform fails on fail on their own; the step is partial, and
/// says why.
#[test]
fn a_yaml_transform_that_fails_on_some_files_makes_its_step_partial() {
    let h = Harness::new();
    h.write("src/a.ts", "a\n");
    h.write("src/b.ts", "b\n");
    let report = js_step(
        &h,
        r#"
export default function transform(root: any) {
  if (root.filename().endsWith("a.ts")) throw new Error("cannot handle a");
  return "done\n";
}
"#,
        "          include: [\"src/**/*.ts\"]",
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(report.steps[0].status, StepStatus::Partial);
    assert_eq!(h.read("src/b.ts"), "done\n");
    let events = h.events.take();
    let logged = events.iter().any(|event| {
        matches!(event, codemod_workflow::events::Event::StepLog { line, .. } if line.contains("cannot handle a"))
    });
    assert!(logged, "{events:#?}");
}

/// `getSelector` picks the files the transform runs on, and gives it the
/// matches; a selector that cannot be read fails the files it runs on.
#[test]
fn a_yaml_transform_s_selector_picks_its_files() {
    let h = Harness::new();
    h.write("src/a.ts", "console.log(1);\n");
    h.write("src/b.ts", "const b = 1;\n");
    let report = js_step(
        &h,
        r#"
export function getSelector() {
  return { rule: { pattern: "console.log($A)" } };
}
export default function transform(root: any, options: any) {
  return `// ${options.matches.length} match\n` + root.root().text();
}
"#,
        "          include: [\"src/**/*.ts\"]",
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("src/a.ts"), "// 1 match\nconsole.log(1);\n");
    assert_eq!(h.read("src/b.ts"), "const b = 1;\n");

    let h = Harness::new();
    h.write("src/a.ts", "a\n");
    let report = js_step(
        &h,
        r#"
export function getSelector() { throw new Error("no selector today"); }
export default function transform() { return null; }
"#,
        "          include: [\"src/**/*.ts\"]",
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("no selector today"), "{error}");
}

/// File-scope semantic analysis is on by default, as it was; a step
/// `dry_run` runs the transform (which sees `options.dryRun`) but changes
/// nothing.
#[test]
fn a_yaml_transform_gets_semantic_analysis_and_dry_run_as_before() {
    let transform = r#"
export default function transform(root: any, options: any) {
  const use = root.root().find({ rule: { pattern: "console.log($A)" } });
  const definition = use.getMatch("A").definition();
  const edit = `// defined: ${definition ? definition.node.text() : "unknown"}\n` + root.root().text();
  return { edit, data: options.dryRun ? "dry run" : "run" };
}
"#;
    for (options, expected, ran) in [
        ("", "// defined: value\n", "run"),
        (
            "          semantic_analysis: workspace",
            "// defined: value\n",
            "run",
        ),
        ("          dry_run: true", "", "dry run"),
    ] {
        let h = Harness::new();
        h.write("src/a.ts", "const value = 1;\nconsole.log(value);\n");
        let step = format!("          include: [\"src/**/*.ts\"]\n{options}");
        let report = js_step(&h, transform, &step);
        assert!(output(&report).is_ok(), "{options}: {:?}", report.ending);
        assert_eq!(
            h.read("src/a.ts"),
            format!("{expected}const value = 1;\nconsole.log(value);\n"),
            "{options}"
        );
        // The transform ran, and saw whether it was a dry run.
        let result = report.steps[0].result.as_ref().unwrap();
        assert_eq!(result["outputs"], json!([ran]), "{options}: {result}");
    }
}

/// A transform that changes other files with `jssgTransform`, driven by
/// comments in the file it runs on:
/// `// touch: <path>` appends `// touched by <file>` to `<path>` (relative
/// to the target), `// rename-less: <path>` renames that `.less` file to
/// `.css` and fixes the import, `// throw` fails after the touches, and
/// `OLD` becomes `NEW` in the file itself.
const CROSS_FILE: &str = r#"
import { jssgTransform } from "codemod:ast-grep";

export default async function transform(root: any, options: any) {
  const text: string = root.root().text();
  const name = root.filename().split("/").pop();
  for (const match of text.matchAll(/\/\/ touch: (\S+)/g)) {
    await jssgTransform(
      (r: any) => r.root().text() + `// touched by ${name}\n`,
      `${options.targetDir}/${match[1]}`,
      "typescript",
    );
  }
  let edit = text;
  for (const match of text.matchAll(/\/\/ rename-less: (\S+)/g)) {
    await jssgTransform(
      (r: any) => {
        r.rename(r.filename().replace(/\.less$/, ".css"));
        return r.root().text().replace("@color", "--color");
      },
      `${options.targetDir}/${match[1]}`,
      "css",
    );
    edit = edit.replace(/'([^']*)\.less'/, "'$1.css'");
  }
  if (text.includes("// throw")) throw new Error(`${name} fails on purpose`);
  edit = edit.replace("OLD", "NEW");
  return edit === text ? null : edit;
}
"#;

/// Two files that change a third (outside the step's files), one that
/// changes another of the step's files, and one that renames a stylesheet.
fn cross_file_target(h: &Harness) {
    h.write(
        "src/a.ts",
        "// touch: shared/index.ts\n// touch: src/b.ts\n// rename-less: src/styles.less\nimport './styles.less';\nconst a = 'OLD';\n",
    );
    h.write("src/b.ts", "// touch: shared/index.ts\nconst b = 'OLD';\n");
    h.write("src/styles.less", "@color: red;\n");
    h.write("shared/index.ts", "export {};\n");
}

/// A workflow.yaml transform may still change other files with
/// `jssgTransform`: every change to a file adds up, whichever file's
/// transform made it, and none is lost to another.
#[test]
fn a_yaml_transform_changes_other_files_with_jssg_transform() {
    let h = Harness::new();
    cross_file_target(&h);
    let report = js_step(&h, CROSS_FILE, "          include: [\"src/**/*.ts\"]");
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(report.steps[0].status, StepStatus::Succeeded);
    assert_eq!(
        h.read("src/a.ts"),
        "// touch: shared/index.ts\n// touch: src/b.ts\n// rename-less: src/styles.less\nimport './styles.css';\nconst a = 'NEW';\n"
    );
    // b.ts's own change and a.ts's change to it.
    assert_eq!(
        h.read("src/b.ts"),
        "// touch: shared/index.ts\nconst b = 'NEW';\n// touched by a.ts\n"
    );
    // Both files' changes to a file outside the step's files, in either order.
    let index = h.read("shared/index.ts");
    let mut lines: Vec<&str> = index.lines().collect();
    lines.sort();
    assert_eq!(
        lines,
        ["// touched by a.ts", "// touched by b.ts", "export {};"],
        "{index}"
    );
    assert!(!h.exists("src/styles.less"));
    assert_eq!(h.read("src/styles.css"), "--color: red;\n");
    // The step's changes and the run's diffs name every file.
    let changes = &report.steps[0].result.as_ref().unwrap()["changes"];
    let mut written: Vec<&str> = changes["written"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path.as_str().unwrap())
        .collect();
    written.sort();
    assert_eq!(
        written,
        ["shared/index.ts", "src/a.ts", "src/b.ts", "src/styles.css"],
        "{changes}"
    );
    assert_eq!(
        changes["renamed"],
        json!([{ "from": "src/styles.less", "to": "src/styles.css" }])
    );
    assert_eq!(h.engine_summary_diffs(), 4);
}

/// A dry run writes nothing, and reports each changed file once, with
/// every change made to it.
#[test]
fn a_dry_run_reports_what_yaml_transforms_would_change_in_other_files() {
    let h = Harness::new();
    cross_file_target(&h);
    h.bundle_file("scripts/codemod.ts", CROSS_FILE);
    let workflow = h.workflow_file(
        "workflow.yaml",
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Transform
        js-ast-grep:
          js_file: scripts/codemod.ts
          include: ["src/**/*.ts"]
"#,
    );
    let report = h.start_loaded(workflow, json!({}), true);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("shared/index.ts"), "export {};\n");
    assert_eq!(
        h.read("src/b.ts"),
        "// touch: shared/index.ts\nconst b = 'OLD';\n"
    );
    assert!(h.exists("src/styles.less") && !h.exists("src/styles.css"));
    let diffs = h.summary.diffs();
    assert_eq!(diffs.len(), 4, "{diffs:#?}");
    let index = diffs
        .iter()
        .map(|diff| format!("{diff:?}"))
        .find(|diff| diff.contains("shared/index.ts"))
        .expect("a diff of shared/index.ts");
    assert!(
        index.contains("+// touched by a.ts") && index.contains("+// touched by b.ts"),
        "{index}"
    );
}

/// A transform fails on its file, and none of its changes to other files
/// are kept, when it throws, changes its own file with `jssgTransform`
/// (it returns its file's new text instead), or reaches outside the target,
/// through `..` or a symbolic link. The other files' changes are kept.
#[test]
fn a_yaml_transform_s_changes_to_other_files_are_kept_only_when_it_succeeds() {
    let h = Harness::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("escape.ts"), "export {};\n").unwrap();
    // Next to the target: `../escape.ts` from it.
    std::fs::write(h.target.path().join("../escape.ts"), "export {};\n").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), h.target.path().join("link")).unwrap();
        h.write(
            "src/symlink.ts",
            "// touch: shared/index.ts\n// touch: link/escape.ts\nconst l = 'OLD';\n",
        );
    }
    h.write("shared/index.ts", "export {};\n");
    h.write(
        "src/ok.ts",
        "// touch: shared/index.ts\nconst ok = 'OLD';\n",
    );
    h.write(
        "src/throws.ts",
        "// touch: shared/index.ts\n// throw\nconst t = 'OLD';\n",
    );
    h.write(
        "src/self.ts",
        "// touch: shared/index.ts\n// touch: src/self.ts\nconst s = 'OLD';\n",
    );
    h.write(
        "src/dots.ts",
        "// touch: shared/index.ts\n// touch: ../escape.ts\nconst d = 'OLD';\n",
    );
    let report = js_step(&h, CROSS_FILE, "          include: [\"src/**/*.ts\"]");
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(report.steps[0].status, StepStatus::Partial);
    assert_eq!(
        h.read("src/ok.ts"),
        "// touch: shared/index.ts\nconst ok = 'NEW';\n"
    );
    assert_eq!(
        h.read("shared/index.ts"),
        "export {};\n// touched by ok.ts\n"
    );
    for (path, text) in [
        (
            "src/throws.ts",
            "// touch: shared/index.ts\n// throw\nconst t = 'OLD';\n",
        ),
        (
            "src/self.ts",
            "// touch: shared/index.ts\n// touch: src/self.ts\nconst s = 'OLD';\n",
        ),
        (
            "src/dots.ts",
            "// touch: shared/index.ts\n// touch: ../escape.ts\nconst d = 'OLD';\n",
        ),
    ] {
        assert_eq!(h.read(path), text, "{path}");
    }
    for escape in [
        outside.path().join("escape.ts"),
        h.target.path().join("../escape.ts"),
    ] {
        assert_eq!(std::fs::read_to_string(&escape).unwrap(), "export {};\n");
    }
    let failures = &report.steps[0].result.as_ref().unwrap()["failures"];
    let error = |path: &str| {
        failures
            .as_array()
            .unwrap()
            .iter()
            .find(|failure| failure["path"] == path)
            .unwrap_or_else(|| panic!("{path} did not fail: {failures}"))["error"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(error("src/throws.ts").contains("throws.ts fails on purpose"));
    assert!(error("src/self.ts").contains("is being transformed already"));
    assert!(error("src/dots.ts").contains("outside the target directory"));
    #[cfg(unix)]
    assert!(error("src/symlink.ts").contains("outside the target directory"));
    let _ = std::fs::remove_file(h.target.path().join("../escape.ts"));
}

/// A transform that does not wait for its `jssgTransform` call fails on
/// its file: the call would change the other file after the transform's
/// changes were kept or dropped. Nothing of it is kept, and the other
/// files' transforms are not affected by the late call.
#[test]
fn a_yaml_transform_that_does_not_await_jssg_transform_fails() {
    let h = Harness::new();
    h.write("shared/index.ts", "export {};\n");
    h.write("src/late.ts", "// late\nconst l = 'OLD';\n");
    h.write(
        "src/ok.ts",
        "// touch: shared/index.ts\nconst ok = 'OLD';\n",
    );
    let transform = CROSS_FILE.replace(
        "  let edit = text;",
        r#"  if (text.includes("// late")) {
    jssgTransform(async (r: any) => {
      await new Promise((resolve) => setTimeout(resolve, 50));
      return r.root().text() + "// late\n";
    }, `${options.targetDir}/shared/index.ts`, "typescript");
  }
  let edit = text;"#,
    );
    let report = js_step(&h, &transform, "          include: [\"src/**/*.ts\"]");
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("src/late.ts"), "// late\nconst l = 'OLD';\n");
    assert_eq!(
        h.read("src/ok.ts"),
        "// touch: shared/index.ts\nconst ok = 'NEW';\n"
    );
    assert_eq!(
        h.read("shared/index.ts"),
        "export {};\n// touched by ok.ts\n"
    );
    let failures = report.steps[0].result.as_ref().unwrap()["failures"].to_string();
    assert!(
        failures.contains("src/late.ts") && failures.contains("await it"),
        "{failures}"
    );
}

/// Many files changing one file, and files changing each other, keep every
/// change: a transform waiting for a file another holds while that one
/// waits for its own gives way and runs again.
#[test]
fn yaml_transforms_that_change_the_same_files_keep_every_change() {
    let h = Harness::new();
    h.write("shared/index.ts", "");
    for n in 0..24 {
        // Pairs that change each other, and all of them the index.
        let pair = n ^ 1;
        h.write(
            &format!("src/f{n:02}.ts"),
            &format!("// touch: shared/index.ts\n// touch: src/f{pair:02}.ts\nconst f = 'OLD';\n"),
        );
    }
    let report = js_step(&h, CROSS_FILE, "          include: [\"src/**/*.ts\"]");
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(
        report.steps[0].status,
        StepStatus::Succeeded,
        "{:?}",
        report.steps[0].result
    );
    let index = h.read("shared/index.ts");
    let mut lines: Vec<&str> = index.lines().collect();
    lines.sort();
    let expected: Vec<String> = (0..24)
        .map(|n| format!("// touched by f{n:02}.ts"))
        .collect();
    assert_eq!(lines, expected);
    for n in 0..24 {
        let pair = n ^ 1;
        assert_eq!(
            h.read(&format!("src/f{n:02}.ts")),
            format!("// touch: shared/index.ts\n// touch: src/f{pair:02}.ts\nconst f = 'NEW';\n// touched by f{pair:02}.ts\n"),
        );
    }
}

/// A step `dry_run` changes nothing, in other files either.
#[test]
fn a_yaml_step_dry_run_changes_no_other_file() {
    let h = Harness::new();
    cross_file_target(&h);
    let report = js_step(
        &h,
        CROSS_FILE,
        "          include: [\"src/**/*.ts\"]\n          dry_run: true",
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("shared/index.ts"), "export {};\n");
    assert_eq!(
        h.read("src/b.ts"),
        "// touch: shared/index.ts\nconst b = 'OLD';\n"
    );
    assert!(h.exists("src/styles.less"));
    assert_eq!(h.engine_summary_diffs(), 0);
}

/// A shard's transform may change other files too; as a dry run, it
/// changes nothing, and picks the files it would change itself.
#[test]
fn a_shard_transform_may_change_other_files() {
    let h = Harness::new();
    h.write(
        "src/a/a.ts",
        "// touch: shared/index.ts\nconst a = 'OLD';\n",
    );
    h.write("src/b/b.ts", "// touch: shared/index.ts\nconst b = 1;\n");
    h.write("shared/index.ts", "export {};\n");
    h.bundle_file("scripts/codemod.ts", CROSS_FILE);
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
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let shards = report.steps[0].result.as_ref().unwrap();
    assert!(shards.to_string().contains("a/a.ts"), "{shards}");
    assert!(!shards.to_string().contains("b/b.ts"), "{shards}");
    assert_eq!(h.read("shared/index.ts"), "export {};\n");
    assert_eq!(
        h.read("src/a/a.ts"),
        "// touch: shared/index.ts\nconst a = 'OLD';\n"
    );
}
