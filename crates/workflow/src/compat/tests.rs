use super::*;
use crate::build::build;

fn parse(yaml: &str) -> Workflow {
    serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("bad test YAML: {e}"))
}

/// The text without whitespace, and without commas before a closing
/// bracket, so a check does not depend on where lines break.
fn squashed(text: &str) -> String {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    compact
        .replace(",)", ")")
        .replace(",]", "]")
        .replace(",}", "}")
}

/// Whether `source` has `needle`, however the lines break.
fn has(source: &str, needle: &str) -> bool {
    squashed(source).contains(&squashed(needle))
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

#[test]
fn shell_state_guard_uses_the_platform_passed_to_main() {
    use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
    use rquickjs::{CatchResultExt, Context, Function, Module, Promise, Runtime};

    let source = translated(
        r#"
version: "1"
nodes:
  - id: stateful
    name: Stateful
    steps:
      - name: First
        run: echo first
      - name: Record
        run: echo seen=true >> "$STATE_OUTPUTS"
"#,
    )
    .source;
    let built = build("workflow.ts", &source).unwrap();
    let runtime = Runtime::new().unwrap();
    let workflow = crate::build::FLOW_MODULE;
    runtime.set_loader(
        BuiltinResolver::default().with_module(workflow),
        BuiltinLoader::default().with_module(
            workflow,
            "export const shell = command => command;\n\
             export const step = label => { throw new Error(`started ${label}`); };",
        ),
    );
    Context::full(&runtime).unwrap().with(|ctx| {
        let (module, ready) = Module::declare(ctx.clone(), "workflow.js", built.main.as_str())
            .unwrap()
            .eval()
            .unwrap();
        ready.finish::<()>().unwrap();
        let main: Function = module.get("default").unwrap();
        for (platform, expected) in [
            (
                "win32",
                "$STATE_OUTPUTS), which is not supported on Windows",
            ),
            ("linux", "started First"),
        ] {
            let input = ctx
                .json_parse(format!(r#"{{"platform":"{platform}"}}"#))
                .unwrap();
            let result: Promise = main.call((input,)).unwrap();
            let error = result.finish::<()>().catch(&ctx).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    });
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
        has(&source,
            "  const state: State = {};\n  await setupNode(state);\n  await transformNode(state);\n  await cleanupNode(state);\n}"
        ),
        "{source}"
    );
    assert!(
        has(
            &source,
            r#"applyStateOutputs(state, await step("Initialize", shell("CODEMOD_STATE_DIR="#
        ) && has(&source, r#"(\n$CODEMOD_PATH/scripts/setup.sh\n)"#),
        "{source}"
    );
    for unused in [
        "after(",
        "Params",
        "Matrix",
        "DEFAULT_PARAMS",
        "const matrix",
    ] {
        assert!(!has(&source, unused), "{unused} in:\n{source}");
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
import { forEachFile, step } from "codemod/workflow";
import type { WorkflowContext } from "codemod/workflow";
import * as workflowTransform from "./src/workflow.ts";
import { jssgFile } from "codemod-internal:jssg-compat";

type Params = Record<string, unknown>;

export default async function main({ params }: WorkflowContext<Params>) {
  // Apply AST Transformations
  await step(
    "Rewrite rmdir",
    forEachFile(
      {
        include: ["**/*.js", "**/*.ts"],
        exclude: ["**/node_modules/**"],
        language: "typescript",
      },
      (file) => jssgFile(workflowTransform, file, { params }),
    ),
  );
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
        r#"import * as codemodTransform from "./scripts/codemod.ts";"#,
        "await step(\"Transform\", forEachFile({ language: \"typescript\" }, (file) => jssgFile(codemodTransform, file, { params })));",
        r#"import configRules from "./rules/config.yml";"#,
        r#"await step("Rules", forEachFile(astGrep(configRules)));"#,
        r#"forEachFile({ include: ["src/**"] }, astGrep(configRules))"#,
    ] {
        assert!(
            has(&source, expected),
            "missing {expected} in:\n{source}"
        );
    }
}

#[test]
fn ast_grep_base_path_is_joined_to_the_globs() {
    let result = translated(
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Under base
        ast-grep:
          config_file: rules/config.yml
          base_path: ./packages/app/
          include: ["*.ts", "src/**/*.ts", "**/*.js"]
          exclude: ["node_modules"]
          max_threads: 2
      - name: Whole base
        ast-grep:
          config_file: rules/config.yml
          base_path: packages/app
"#,
    );
    for expected in [
        r#"include: ["packages/app/**/*.ts", "packages/app/src/**/*.ts", "packages/app/**/*.js"]"#,
        r#"exclude: ["packages/app/**/node_modules"]"#,
        r#"forEachFile({ include: ["packages/app/**"] }, astGrep(configRules))"#,
        "step 'Under base': max_threads is dropped",
    ] {
        assert!(
            has(&result.source, expected),
            "missing {expected} in:\n{}",
            result.source
        );
    }
    assert!(!has(&result.source, "basePath"), "{}", result.source);
    assert_eq!(rebase_glob("a", "!b/*.ts"), "!a/b/*.ts");
    assert_eq!(rebase_glob("a/", "/top.ts"), "a/top.ts");
    assert_eq!(rebase_glob("./a", "dist/"), "a/**/dist/");
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
        has(&source, "await Promise.all([aNode(), bNode()]);"),
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
    let expect = |needle: &str| assert!(has(source, needle), "missing {needle:?} in:\n{source}");

    expect("const DEFAULT_PARAMS: Params = { mode: \"safe\", pr_size: 20 };");
    expect("import { branch, commit, pullRequest } from \"codemod/git\";");
    let workflow_import = source
        .split("import ")
        .find(|part| part.starts_with('{') && part.contains("} from \"codemod/workflow\";"))
        .unwrap();
    assert!(
        !workflow_import.contains("commit"),
        "git steps imported from codemod/workflow: {workflow_import}"
    );
    expect("state[\"shards\"] = await step(\"Build shards\", shard({ method: { type: \"directory\", maxFilesPerShard: Number(`${asText(params.pr_size, \"params.pr_size\")}`) }, target: \"./src\", files: { language: \"tsx\" }, filter: (file) => jssgWouldChange(codemodTransform, file, { language: \"tsx\", params }), previous: state[\"shards\"] }));");
    // Git steps share one checkout: the matrix with a pull request runs one
    // value at a time; the one without runs its values together.
    expect("const values: Matrix[] = (state[\"shards\"] ?? []);\n  if (!Array.isArray(values)) {\n    throw new Error(\"matrix values must be a list\");\n  }\n  for (let index = 0; index < values.length; index++) {\n    await applyNodeFor(params, state, values[index], index);\n  }");
    expect(".map((matrix: Matrix, index: number) => reportNodeFor(params, matrix, index))");
    expect("await step(approval(\"Run 'Apply transforms'?\"));");
    expect("const branchName = `codemod-apply-${index}`;");
    expect("import * as codemodTransform from \"./scripts/codemod.ts\";");
    expect("import { jssgFile, jssgWouldChange } from \"codemod-internal:jssg-compat\";");
    // The adapter parses with the selector's language.
    expect("forEachFile({ include: matrix?._meta_files, language: \"tsx\" }, (file) => jssgFile(codemodTransform, file, { params, matrix: matrix ?? undefined }))");
    expect("if (evaluate(\"condition\", \"${{ params.mode == \\\"fast\\\" }}\", { params, matrix, state })) {");
    expect("await step(commit(`chore: ${asText(matrix?.name, \"matrix.name\")}`, { allowEmpty: true }));");
    expect("if (evaluate(\"condition\", \"contains(params.mode, \\\"x\\\")\", { params, matrix, state })) {");
    expect("shell(\"echo fast\", { env: { ...matrixEnv(matrix), CI: \"true\" } })");
    expect("await lintFixTemplate({ ...params, ...{ target_glob: \"src/**\" } }, matrix, state);");
    expect("await step(pullRequest({ title: `refactor: migrate ${asText(matrix?.name, \"matrix.name\")}`, draft: true, branch: branchName }));");
    expect("agent(`Summarize ${asText(matrix?.team, \"matrix.team\")}`)");
    expect("codemod(\"@codemod/other\", { params })");
    expect("const reportNodeDone = after([evaluateShardsNodeDone], () => reportNode(params));");

    let warnings = result.warnings.join("\n");
    assert!(warnings.contains("state.schema is not used"), "{warnings}");
    assert!(warnings.contains("runtime"), "{warnings}");
    assert!(warnings.contains("timeout_ms"), "{warnings}");
    assert!(warnings.contains("args, env and working_dir"), "{warnings}");
}

#[test]
fn a_matrix_with_any_git_step_runs_one_value_at_a_time() {
    let matrix = |git: &str, steps: &str| {
        format!(
            r#"
version: "1"
templates:
  - id: save
    name: Save
    steps:
      - name: Save
        run: echo save
        commit:
          message: save
nodes:
  - id: apply
    name: Apply
    strategy:
      type: matrix
      values:
        - team: a
        - team: b
{git}    steps:
{steps}"#
        )
    };
    let plain_step = "      - name: Run\n        run: echo hi\n";
    let cases = [
        (
            "branch",
            matrix(
                "    branch_name: \"codemod-${{ matrix.team }}\"\n",
                plain_step,
            ),
        ),
        (
            "commit",
            matrix(
                "",
                "      - name: Run\n        run: echo hi\n        commit:\n          message: hi\n",
            ),
        ),
        (
            "template commit",
            matrix(
                "",
                "      - name: Save\n        use:\n          template: save\n",
            ),
        ),
    ];
    for (name, yaml) in cases {
        let source = source_of(&yaml);
        assert!(
            has(
                &source,
                "for (let index = 0; index < values.length; index++)"
            ),
            "{name}: not sequential:\n{source}"
        );
        assert!(!has(&source, ".map((matrix: Matrix"), "{name}:\n{source}");
    }
    let source = source_of(&matrix("", plain_step));
    assert!(has(&source, ".map((matrix: Matrix"), "{source}");
}

#[test]
fn only_the_git_steps_used_are_imported_from_codemod_git() {
    let yaml = |commit: &str| {
        format!(
            r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Run
        run: echo hi
{commit}"#
        )
    };
    let source = translated(&yaml("        commit:\n          message: hi\n")).source;
    assert!(
        has(&source, "import { commit } from \"codemod/git\";\n"),
        "{source}"
    );
    let source = translated(&yaml("")).source;
    assert!(!has(&source, "codemod/git"), "{source}");
    let source = translated(
        r#"
version: "1"
nodes:
  - id: apply
    name: Apply
    branch_name: "codemod-apply"
    steps:
      - name: Run
        run: echo hi
"#,
    )
    .source;
    assert!(
        has(&source, "import { branch } from \"codemod/git\";\n"),
        "{source}"
    );
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
        has(
            &translated.source,
            "Rewrite app/api/**\\/*.ts routes: Two lines, and a *\\/ too"
        ),
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
        has(
            &source,
            r#"applyStateOutputs(state, await step("Version", shell("#
        ),
        "{source}"
    );
    assert!(
        has(
            &source,
            "(\\necho ${asText(state.version, \"state.version\")}\\n)"
        ),
        "{source}"
    );
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
        has(
            &source,
            r#"applyStateOutputs(state, await step("Count", shell("#
        ),
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
        has(
            &source,
            r#"applyStateOutputs(state, await step("Count", shell("#
        ),
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
        has(&source, r#"await step("Run", shell("echo hi"));"#),
        "{source}"
    );
    assert!(!has(&source, "State"), "{source}");
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
        has(
            &source,
            r#"applyStateOutputs(state, await step("Run script", shell("#
        ),
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
        has(&source, "applyStateOutputs(state, await step(\"Count\""),
        "{source}"
    );
}

#[test]
fn a_container_runtime_is_refused_by_run_and_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workflow.yaml");
    std::fs::write(
        &path,
        r#"
version: "1"
nodes:
  - id: where
    name: Where
    runtime:
      type: docker
      image: alpine
    steps:
      - name: uname
        run: uname -s
"#,
    )
    .unwrap();
    for error in [
        translate_file(&path).unwrap_err(),
        upgrade_file(&path).unwrap_err(),
    ] {
        assert!(
            error.contains("node 'where' runs its steps in a docker container (`runtime: docker`)"),
            "{error}"
        );
        assert!(error.contains("npx codemod@1"), "{error}");
    }
}

// ---- `codemod workflow upgrade`: plain callbacks ----

/// `upgrade_file` on a package of `files`, checking the result builds.
fn upgraded(yaml: &str, files: &[(&str, &str)]) -> Translated {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let path = dir.path().join("workflow.yaml");
    std::fs::write(&path, yaml).unwrap();
    let result = upgrade_file(&path).unwrap();
    if let Err(error) = build("workflow.ts", &result.source) {
        panic!(
            "upgraded workflow does not build: {error}\n\n{}",
            result.source
        );
    }
    result
}

const RMDIR_TRANSFORM: &str = "import type { SgRoot } from '@codemod.com/jssg-types/main';\nexport default function transform(root: SgRoot): string | null {\n  return null;\n}\n";

const TS_GLOBS: &str = r#"          include:
            - "**/*.cjs"
            - "**/*.cts"
            - "**/*.js"
            - "**/*.jsx"
            - "**/*.mjs"
            - "**/*.mts"
            - "**/*.ts"
            - "**/*.tsx"
          exclude:
            - "**/node_modules/**"
          language: typescript
"#;

#[test]
fn an_upgraded_transform_is_called_from_a_plain_callback() {
    let yaml = format!(
        r#"version: "1"
nodes:
  - id: apply-transforms
    name: Apply AST Transformations
    type: automatic
    steps:
      - name: Handle DEP0147 via transforming `fs.rmdir` to `fs.rm`.
        js-ast-grep:
          js_file: src/workflow.ts
          base_path: .
{TS_GLOBS}"#
    );
    let result = upgraded(&yaml, &[("src/workflow.ts", RMDIR_TRANSFORM)]);
    assert_eq!(
        result.source,
        r#"// Generated from workflow.yaml by `codemod workflow upgrade`.
import { parse } from "codemod/ast-grep";
import { forEachFile, step } from "codemod/workflow";
import transform from "./src/workflow.ts";

export default async function main() {
  // Apply AST Transformations
  await step(
    "Handle DEP0147 via transforming `fs.rmdir` to `fs.rm`.",
    forEachFile(
      {
        include: [
          "**/*.cjs",
          "**/*.cts",
          "**/*.js",
          "**/*.jsx",
          "**/*.mjs",
          "**/*.mts",
          "**/*.ts",
          "**/*.tsx",
        ],
        exclude: ["**/node_modules/**"],
        language: "typescript",
      },
      (file) => {
        const out = transform(parse(file));
        if (typeof out === "string") file.write(out);
      },
    ),
  );
}
"#
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

#[test]
fn transforms_keep_their_options_but_no_longer_get_capabilities() {
    let yaml = format!(
        r#"version: "1"
nodes:
  - id: apply-transforms
    name: Apply AST Transformations
    type: automatic
    steps:
      - name: Migrate from chalk
        js-ast-grep:
          js_file: src/workflow.ts
          base_path: .
{TS_GLOBS}
  - id: remove-dependencies
    name: Remove chalk dependency
    type: automatic
    steps:
      - name: Remove the chalk dependency
        js-ast-grep:
          js_file: src/remove-dependencies.ts
          base_path: .
          include:
            - "**/package.json"
          exclude:
            - "**/node_modules/**"
          language: typescript
          capabilities:
            - child_process
            - fs
"#
    );
    let result = upgraded(
        &yaml,
        &[
            ("src/workflow.ts", RMDIR_TRANSFORM),
            (
                "src/remove-dependencies.ts",
                "import type { Transform } from '@codemod.com/jssg-types/main';\nimport type Json from '@codemod.com/jssg-types/langs/json';\nconst transform: Transform<Json> = async (root) => {\n  return null;\n};\nexport default transform;\n",
            ),
        ],
    );
    assert_eq!(
        result.source,
        r#"// Generated from workflow.yaml by `codemod workflow upgrade`.
import { parse } from "codemod/ast-grep";
import { forEachFile, step } from "codemod/workflow";
import type { WorkflowContext } from "codemod/workflow";
import workflowTransform from "./src/workflow.ts";
import removeDependenciesTransform from "./src/remove-dependencies.ts";

type Params = Record<string, string>;

export default async function main({ params, target }: WorkflowContext<Params>) {
  await Promise.all([
    applyTransformsNode(),
    removeDependenciesNode(params, target),
  ]);
}

/** Apply AST Transformations */
async function applyTransformsNode() {
  await step(
    "Migrate from chalk",
    forEachFile(
      {
        include: [
          "**/*.cjs",
          "**/*.cts",
          "**/*.js",
          "**/*.jsx",
          "**/*.mjs",
          "**/*.mts",
          "**/*.ts",
          "**/*.tsx",
        ],
        exclude: ["**/node_modules/**"],
        language: "typescript",
      },
      (file) => {
        const out = workflowTransform(parse(file));
        if (typeof out === "string") file.write(out);
      },
    ),
  );
}

/** Remove chalk dependency */
async function removeDependenciesNode(params: Params, target: string) {
  await step(
    "Remove the chalk dependency",
    forEachFile(
      {
        include: ["**/package.json"],
        exclude: ["**/node_modules/**"],
        language: "typescript",
      },
      async (file) => {
        const out = await removeDependenciesTransform(parse(file), {
          params,
          language: "typescript",
          targetDir: target,
        });
        if (typeof out === "string") file.write(out);
      },
    ),
  );
}
"#
    );
}

#[test]
fn a_selector_several_steps_share_is_one_const() {
    let step = |name: &str| {
        format!("      - name: {name}\n        js-ast-grep:\n          js_file: src/{name}.ts\n{TS_GLOBS}")
    };
    let yaml = format!(
        "version: \"1\"\nnodes:\n  - id: apply\n    name: Apply\n    steps:\n{}{}{}",
        step("enroll"),
        step("unenroll"),
        step("cleanup")
    );
    let files: Vec<(String, &str)> = ["enroll", "unenroll", "cleanup"]
        .iter()
        .map(|name| (format!("src/{name}.ts"), RMDIR_TRANSFORM))
        .collect();
    let files: Vec<(&str, &str)> = files.iter().map(|(n, t)| (n.as_str(), *t)).collect();
    let source = upgraded(&yaml, &files).source;
    assert_eq!(source.matches("\"**/*.tsx\"").count(), 1, "{source}");
    assert!(
        source.contains("const files = {\n  include: [\n    \"**/*.cjs\","),
        "{source}"
    );
    assert!(
        source.contains("  await step(\n    \"enroll\",\n    forEachFile(files, (file) => {\n      const out = enrollTransform(parse(file));\n"),
        "{source}"
    );
    assert_eq!(
        source.matches("language: \"typescript\"").count(),
        1,
        "{source}"
    );
}

#[test]
fn selectors_results_matrices_and_bases_are_kept_in_the_plain_callback() {
    let result = upgraded(
        r#"version: "1"
nodes:
  - id: report
    name: Report
    strategy:
      type: matrix
      values:
        - team: a
    steps:
      - name: Report
        js-ast-grep:
          js_file: src/report.ts
          base_path: ./packages/app/
          language: tsx
"#,
        &[(
            "src/report.ts",
            "export const getSelector = () => ({ rule: { pattern: 'console.log($$$)' } });\nexport default async function transform(root, options) {\n  return { edit: null, data: options.matches.length };\n}\n",
        )],
    );
    let source = &result.source;
    for needle in [
        "import transform, { getSelector } from \"./src/report.ts\";",
        "if (!file.path.startsWith(\"packages/app/\")) return;",
        "const root: Parameters<typeof transform>[0] = parse(file);",
        "const matches = root.root().findAll(getSelector({ params }));",
        "if (matches.length === 0) return;",
        "const result = await transform(root, { params, language: \"tsx\", targetDir: target, matches, matrixValues: matrix ?? undefined });",
        "if (typeof result === \"string\") return file.write(result);",
        "if (typeof result?.edit === \"string\") file.write(result.edit);",
        "return result?.data;",
        "forEachFile({ language: \"tsx\" }, async (file) => {",
    ] {
        assert!(has(source, needle), "missing {needle} in:\n{source}");
    }
    assert!(!source.contains("jssg-compat"), "{source}");
}

#[test]
fn a_transform_that_renames_its_file_keeps_the_adapter_with_a_reason() {
    let result = upgraded(
        r#"version: "1"
nodes:
  - id: styles
    name: Styles
    steps:
      - name: Move styles
        js-ast-grep:
          js_file: src/move.ts
          include: ["**/*.less"]
          language: css
"#,
        &[(
            "src/move.ts",
            "export default function transform(root) {\n  root.rename(root.filename().replace('.less', '.css'));\n  return null;\n}\n",
        )],
    );
    assert!(
        has(
            &result.source,
            "(file) => jssgFile(moveTransform, file, { params })"
        ),
        "{}",
        result.source
    );
    assert!(
        result
            .source
            .contains("import { jssgFile } from \"codemod-internal:jssg-compat\";"),
        "{}",
        result.source
    );
    assert_eq!(
        result.warnings,
        ["src/move.ts runs through jssgFile, which runs a jssg transform as workflow.yaml did: move.ts calls root.rename()"]
    );
}

#[test]
fn templates_pass_the_target_to_transforms_that_take_options() {
    let result = upgraded(
        r#"version: "1"
templates:
  - id: fix
    name: Fix
    steps:
      - name: Fix
        js-ast-grep:
          js_file: src/fix.ts
nodes:
  - id: apply
    name: Apply
    steps:
      - name: Fix it
        use:
          template: fix
"#,
        &[(
            "src/fix.ts",
            "export default function transform(root, options) {\n  return options.targetDir;\n}\n",
        )],
    );
    let source = &result.source;
    for needle in [
        "async function fixTemplate(params: Params, matrix: Matrix, state: State, target: string) {",
        "await fixTemplate(params, matrix, state, target);",
        "export default async function main({ params, target }: WorkflowContext<Params>) {",
        "targetDir: target",
    ] {
        assert!(has(source, needle), "missing {needle} in:\n{source}");
    }
}

#[test]
fn what_a_selector_uses_is_imported() {
    let source = source_of(
        r#"
version: "1"
nodes:
  - id: each
    name: Each
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
    assert!(has(&source, "__evaluate as evaluate"), "{source}");
    // The node gets what the selector's expression reads.
    assert!(has(&source, "async function eachNodeFor(params: Params, state: State, matrix: Matrix, index: number)"), "{source}");
    assert!(
        has(&source, "forEachFile({ include: evaluate(\"list\""),
        "{source}"
    );
}

#[test]
fn transforms_with_the_same_file_name_get_their_own_names() {
    let transform = "export const getSelector = () => ({ rule: { pattern: 'x' } });\nexport default function transform(root): string | null {\n  return null;\n}\n";
    let source = upgraded(
        r#"version: "1"
nodes:
  - id: apply
    name: Apply
    steps:
      - name: A
        js-ast-grep:
          js_file: a/codemod.ts
      - name: B
        js-ast-grep:
          js_file: b/codemod.ts
"#,
        &[("a/codemod.ts", transform), ("b/codemod.ts", transform)],
    )
    .source;
    assert!(
        source.contains(
            "import codemodTransform, { getSelector as codemodSelector } from \"./a/codemod.ts\";"
        ),
        "{source}"
    );
    assert!(
        source.contains("import codemodTransform2, { getSelector as codemodSelector2 } from \"./b/codemod.ts\";"),
        "{source}"
    );
}
