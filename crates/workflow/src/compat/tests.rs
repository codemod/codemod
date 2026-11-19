use super::*;
use crate::build::build;

fn parse(yaml: &str) -> Workflow {
    serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("bad test YAML: {e}"))
}

/// Translate and check the result builds.
fn translated(yaml: &str) -> Translated {
    let result = translate(&parse(yaml));
    if let Err(error) = build("workflow.ts", &result.source) {
        panic!(
            "translated workflow does not build: {error}\n\n{}",
            result.source
        );
    }
    result
}

fn fill_template(text: &str) -> String {
    text.replace("{language}", "typescript")
        .replace("{include_patterns}", "            - \"**/*.ts\"")
        .replace("{name}", "demo")
        .replace("{skill_path}", "skills/demo")
}

#[test]
fn every_workflow_yaml_in_the_repo_translates_and_builds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = [
        "crates/cli/src/templates/shell/workflow.yaml",
        "crates/cli/src/templates/astgrep-yaml/workflow.yaml",
        "crates/cli/src/templates/skill/workflow.yaml",
        "crates/codemod-sandbox/tests/integration/semantic/false-positive-workflow.yaml",
    ];
    for file in files {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        let result = translated(&fill_template(&text));
        assert!(result.warnings.is_empty(), "{file}: {:?}", result.warnings);
    }
}

#[test]
fn a_linear_shell_workflow_becomes_a_sequence_of_awaits() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../cli/src/templates/shell/workflow.yaml"),
    )
    .unwrap();
    let source = translated(&text).source;
    assert!(
        source.contains(
            "  const state: State = {};\n  await setupNode(state);\n  await transformNode(state);\n  await cleanupNode(state);\n}"
        ),
        "{source}"
    );
    assert!(
        source.contains(
            r#"applyStateOutputs(state, await step("Initialize", shell("CODEMOD_STATE_DIR="#
        ) && source.contains(r#"(\n$CODEMOD_PATH/scripts/setup.sh\n)"#),
        "{source}"
    );
    for unused in [
        "after(",
        "Params",
        "Matrix",
        "DEFAULT_PARAMS",
        "const matrix",
    ] {
        assert!(!source.contains(unused), "{unused} in:\n{source}");
    }
}

#[test]
fn a_single_node_workflow_runs_its_steps_from_main() {
    let source = translated(
        r#"
version: "1"
nodes:
  - id: apply-transforms
    name: Apply AST Transformations
    type: automatic
    runtime:
      type: direct
    steps:
      - name: Rewrite rmdir
        js-ast-grep:
          js_file: src/workflow.ts
          base_path: .
          include: ["**/*.js", "**/*.ts"]
          exclude: ["**/node_modules/**"]
          language: typescript
"#,
    )
    .source;
    assert_eq!(
        source,
        r#"// Generated from workflow.yaml by `codemod workflow upgrade`.
import { forEachFile, jssg, step } from "codemod/workflow";

export default async function main() {
  // Apply AST Transformations
  await step("Rewrite rmdir", forEachFile({
    include: ["**/*.js", "**/*.ts"],
    exclude: ["**/node_modules/**"],
    language: "typescript",
  }, jssg("src/workflow.ts")));
}
"#
    );
}

#[test]
fn transforms_run_through_for_each_file_with_the_yaml_defaults() {
    let source = translated(
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Transform
        js-ast-grep:
          js_file: scripts/codemod.ts
      - name: Rules
        ast-grep:
          config_file: rules/config.yml
      - name: Some rules
        ast-grep:
          config_file: rules/config.yml
          include: ["src/**"]
"#,
    )
    .source;
    for expected in [
        // workflow.yaml parses as TypeScript unless told otherwise.
        r#"await step("Transform", forEachFile({ language: "typescript" }, jssg("scripts/codemod.ts")));"#,
        r#"await step("Rules", forEachFile(astGrep("rules/config.yml")));"#,
        r#"forEachFile({ include: ["src/**"] }, astGrep("rules/config.yml"))"#,
    ] {
        assert!(
            source.contains(expected),
            "missing {expected} in:\n{source}"
        );
    }
}

#[test]
fn independent_nodes_run_together() {
    let source = translated(
        r#"
version: "1"
nodes:
  - id: a
    name: A
    steps:
      - name: One
        run: echo one
  - id: b
    name: B
    steps:
      - name: Two
        run: echo two
"#,
    )
    .source;
    assert!(
        source.contains("await Promise.all([aNode(), bNode()]);"),
        "{source}"
    );
}

const FEATURES: &str = r#"
version: "1"
params:
  schema:
    pr_size:
      type: number
      default: 20
    mode:
      type: string
      default: safe
state:
  schema:
    shards:
      type: array
      items:
        type: object
templates:
  - id: lint-fix
    name: Lint fix
    steps:
      - name: Lint
        run: npx eslint --fix ${{ params.target_glob }}
nodes:
  - id: evaluate-shards
    name: Evaluate shards
    steps:
      - name: Build shards
        shard:
          method:
            type: directory
            max_files_per_shard: ${{ params.pr_size }}
          target: ./src
          output_state: shards
          js-ast-grep:
            js_file: scripts/codemod.ts
            language: tsx
  - id: apply
    name: Apply transforms
    trigger:
      type: manual
    depends_on: [evaluate-shards]
    strategy:
      type: matrix
      from_state: shards
    env:
      CI: "true"
    pull_request:
      title: "refactor: migrate ${{ matrix.name }}"
      draft: true
    steps:
      - name: Run transform
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: tsx
      - name: Only when fast
        if: ${{ params.mode == "fast" }}
        run: echo fast
        commit:
          message: "chore: ${{ matrix.name }}"
      - name: Odd condition
        if: contains(params.mode, "x")
        run: echo odd
      - name: Lint
        use:
          template: lint-fix
          inputs:
            target_glob: "src/**"
  - id: report
    name: Report
    runtime:
      type: docker
    depends_on: [evaluate-shards]
    strategy:
      type: matrix
      values:
        - team: a
        - team: b
    steps:
      - name: Ask the agent
        ai:
          prompt: "Summarize ${{ matrix.team }}"
          timeout_ms: 1000
      - name: Nested
        codemod:
          source: "@codemod/other"
          args: ["--x"]
"#;

#[test]
fn the_documented_features_translate_and_build() {
    let result = translated(FEATURES);
    let source = &result.source;
    let expect =
        |needle: &str| assert!(source.contains(needle), "missing {needle:?} in:\n{source}");

    expect("\"pr_size\": 20");
    expect("\"mode\": \"safe\"");
    expect("state[\"shards\"] = await step(\"Build shards\", shard({ method: { type: \"directory\", maxFilesPerShard: Number(`${params.pr_size}`) }, target: \"./src\", jssg: { path: \"scripts/codemod.ts\", language: \"tsx\", params }, previous: state[\"shards\"] }));");
    expect("(state[\"shards\"] ?? []).map((matrix: Matrix, index: number) => applyNodeFor(params, state, matrix, index))");
    expect("await step(approval(\"Run 'Apply transforms'?\"));");
    expect("const branchName = `codemod-apply-${index}`;");
    expect("forEachFile({ language: \"tsx\" }, jssg(\"scripts/codemod.ts\", {\n    files: matrix?._meta_files,\n    params,\n    matrix: matrix ?? undefined,\n  }))");
    expect("if (evaluate(\"condition\", \"${{ params.mode == \\\"fast\\\" }}\", { params, matrix, state })) {");
    expect("await step(commit(`chore: ${matrix?.name}`, { allowEmpty: true }));");
    expect("if (evaluate(\"condition\", \"contains(params.mode, \\\"x\\\")\", { params, matrix, state })) {");
    expect("shell(\"echo fast\", { env: { ...matrixEnv(matrix), CI: \"true\" } })");
    expect("await lintFixTemplate({ ...params, ...{\"target_glob\":\"src/**\"} }, matrix, state);");
    expect("await step(pullRequest({ title: `refactor: migrate ${matrix?.name}`, draft: true, branch: branchName }));");
    expect("agent(`Summarize ${matrix?.team}`)");
    expect("codemod(\"@codemod/other\", { params })");
    expect("const reportNodeDone = after([evaluateShardsNodeDone], () => reportNode(params));");

    let warnings = result.warnings.join("\n");
    assert!(warnings.contains("state.schema is not used"), "{warnings}");
    assert!(warnings.contains("runtime"), "{warnings}");
    assert!(warnings.contains("timeout_ms"), "{warnings}");
    assert!(warnings.contains("args, env and working_dir"), "{warnings}");
}

#[test]
fn names_and_descriptions_cannot_break_out_of_comments() {
    let translated = translated(
        r#"
version: "1"
nodes:
  - id: api
    name: "Rewrite app/api/**/*.ts routes"
    description: |
      Two lines,
      and a */ too
    type: automatic
    steps:
      - name: Hoist handlers
        run: echo hoisted
  - id: pages
    name: "Then pages/**/*.tsx"
    type: automatic
    depends_on: [api]
    steps:
      - name: Wrap pages
        run: echo wrapped
"#,
    );
    assert!(
        translated
            .source
            .contains("Rewrite app/api/**\\/*.ts routes: Two lines, and a *\\/ too"),
        "{}",
        translated.source
    );
}

// ---- workflow state is a local object, not an engine API ----

fn source_of(yaml: &str) -> String {
    translated(yaml).source
}

#[test]
fn a_shell_step_gets_state_as_variables_and_its_values_are_copied_into_it() {
    let source = source_of(
        r#"
version: "1"
nodes:
  - id: detect
    name: Detect
    steps:
      - name: Version
        run: echo "version=19" >> "$STATE_OUTPUTS"
      - name: Use it
        run: echo ${{ state.version }}
"#,
    );
    assert!(
        source.contains(r#"applyStateOutputs(state, await step("Version", shell("#),
        "{source}"
    );
    assert!(source.contains("(\\necho ${state.version}\\n)"), "{source}");
}

#[test]
fn a_template_that_can_write_state_carries_it_when_used_twice() {
    let source = source_of(
        r#"
version: "1"
templates:
  - id: bump
    name: Bump
    steps:
      - name: Count
        run: echo "count=1" >> "$STATE_OUTPUTS"
nodes:
  - id: twice
    name: Twice
    steps:
      - name: First
        use:
          template: bump
      - name: Second
        use:
          template: bump
"#,
    );
    assert!(
        source.contains(r#"applyStateOutputs(state, await step("Count", shell("#),
        "{source}"
    );
}

#[test]
fn a_template_nested_ten_deep_still_carries_state() {
    let mut yaml = String::from(
        r#"
version: "1"
templates:
  - id: t0
    name: T0
    steps:
      - name: Count
        run: echo "count=1" >> "$STATE_OUTPUTS"
      - name: Again
        run: echo "count=2" >> "$STATE_OUTPUTS"
"#,
    );
    for level in 1..=10 {
        yaml.push_str(&format!(
            "  - id: t{level}\n    name: T{level}\n    steps:\n      - name: Use{level}\n        use:\n          template: t{}\n",
            level - 1
        ));
    }
    yaml.push_str(
        r#"nodes:
  - id: deep
    name: Deep
    steps:
      - name: Top
        use:
          template: t10
"#,
    );
    let source = source_of(&yaml);
    assert!(
        source.contains(r#"applyStateOutputs(state, await step("Count", shell("#),
        "{source}"
    );
}

#[test]
fn a_transform_in_a_workflow_that_reads_state_is_flagged() {
    let result = translated(
        r#"
version: "1"
nodes:
  - id: scan
    name: Scan
    steps:
      - name: Collect
        js-ast-grep:
          js_file: scripts/collect.ts
      - name: Report
        run: echo ${{ state.found }}
"#,
    );
    assert!(
        result.warnings.iter().any(|w| w
            .starts_with("step 'Collect': a transform can no longer read or set workflow state")),
        "{:?}",
        result.warnings
    );
}

#[test]
fn a_workflow_with_one_step_and_no_state_reads_stays_plain() {
    let source = source_of(
        r#"
version: "1"
nodes:
  - id: only
    name: Only
    steps:
      - name: Run
        run: echo hi
"#,
    );
    assert!(
        source.contains(r#"await step("Run", shell("echo hi"));"#),
        "{source}"
    );
    assert!(!source.contains("State"), "{source}");
}

#[test]
fn a_potentially_stateful_script_with_a_plain_step_still_gets_the_wrapper() {
    let source = source_of(
        r#"
version: "1"
nodes:
  - id: only
    name: Only
    steps:
      - name: Announce
        run: echo starting
      - name: Run script
        run: ./record-version.sh
"#,
    );
    assert!(
        source.contains(r#"applyStateOutputs(state, await step("Run script", shell("#),
        "{source}"
    );
    // A script can use STATE_OUTPUTS even if the YAML does not mention it.
    let guard = source.find("not supported on Windows").unwrap();
    let first_step = source.find(r#"await step("Announce","#).unwrap();
    assert!(guard < first_step, "{source}");
}

#[test]
fn templates_used_many_times_over_are_walked_once() {
    let mut yaml = String::from(
        "version: \"1\"\ntemplates:\n  - id: t0\n    name: T0\n    steps:\n      - name: Count\n        run: echo \"count=1\" >> \"$STATE_OUTPUTS\"\n",
    );
    // Each template uses the one before it twice: 2^40 uses, walked once each.
    for level in 1..=40 {
        yaml.push_str(&format!(
            "  - id: t{level}\n    name: T{level}\n    steps:\n      - name: A{level}\n        use:\n          template: t{prev}\n      - name: B{level}\n        use:\n          template: t{prev}\n",
            prev = level - 1
        ));
    }
    yaml.push_str("nodes:\n  - id: top\n    name: Top\n    steps:\n      - name: Top\n        use:\n          template: t40\n");
    let source = source_of(&yaml);
    assert!(
        source.contains("applyStateOutputs(state, await step(\"Count\""),
        "{source}"
    );
}
