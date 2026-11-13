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
        "crates/cli/src/templates/js-astgrep/workflow.yaml",
        "crates/cli/src/templates/astgrep-yaml/workflow.yaml",
        "crates/cli/src/templates/hybrid/workflow.yaml",
        "crates/cli/src/templates/hybrid/workflow.toml.yaml",
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
            "export default async function main() {\n  await setupNode();\n  await transformNode();\n  await cleanupNode();\n}"
        ),
        "{source}"
    );
    assert!(
        source.contains("await step(\"Initialize\", shell(\"$CODEMOD_PATH/scripts/setup.sh\"));"),
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
/// <reference types="codemod/workflow" />
import { jssg, step } from "codemod:workflow";

export default async function main() {
  // Apply AST Transformations
  await step("Rewrite rmdir", jssg("src/workflow.ts", {
    include: ["**/*.js", "**/*.ts"],
    exclude: ["**/node_modules/**"],
    language: "typescript",
  }));
}
"#
    );
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
    expect("setState(\"shards\", await step(\"Build shards\", shard({ method: { type: \"directory\", maxFilesPerShard: Number(`${params.pr_size}`) }, target: \"./src\", jssg: { path: \"scripts/codemod.ts\", language: \"tsx\", params }, previous: getState<any>(\"shards\") })));");
    expect("(getState<any>(\"shards\") ?? []).map((matrix: Matrix, index: number) => applyNodeFor(params, matrix, index))");
    expect("await step(approval(\"Run 'Apply transforms'?\"));");
    expect("const branchName = `codemod-apply-${index}`;");
    expect("jssg(\"scripts/codemod.ts\", {\n    files: matrix?._meta_files,\n    language: \"tsx\",\n    params,\n    matrix: matrix ?? undefined,\n  })");
    expect("if (params.mode == \"fast\") {");
    expect("await step(commit(`chore: ${matrix?.name}`, { allowEmpty: true }));");
    expect("if (evaluate(\"condition\", \"contains(params.mode, \\\"x\\\")\", { params, matrix, state: getState() })) {");
    expect("shell(\"echo fast\", { env: { ...matrixEnv(matrix), CI: \"true\" } })");
    expect("await lintFixTemplate({ ...params, ...{\"target_glob\":\"src/**\"} }, matrix);");
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
