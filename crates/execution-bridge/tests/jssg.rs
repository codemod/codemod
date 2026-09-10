//! JSSG adapter behavior: file selection, failure status, renames, and script
//! resolution. Each test uses `execute_in` with a temporary repository root so
//! no test touches the process working directory.

use std::path::Path;

use butterflow_execution_bridge::{execute_in, parse_request, CompletionStatus};
use butterflow_runners::direct_runner::DirectRunner;
use tempfile::TempDir;

const REPLACE_SCRIPT: &str = r#"export default async function transform(root) {
  return {
    content: root.root().text().replaceAll("oldApi", "newApi"),
    output: { file: root.relativeFilename().replaceAll("\\", "/") },
  };
}"#;

fn write(dir: &Path, relative: &str, content: &[u8]) {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

fn read(dir: &Path, relative: &str) -> String {
    std::fs::read_to_string(dir.join(relative)).expect("read")
}

/// A repository under test plus a separate workflow directory holding the
/// script, so the script itself is never part of the enumerated file set.
struct Fixture {
    repo: TempDir,
    workflow: TempDir,
}

impl Fixture {
    fn path(&self) -> &Path {
        self.repo.path()
    }
}

fn repo(script: &str) -> Fixture {
    let workflow = tempfile::tempdir().expect("workflow dir");
    write(workflow.path(), "transform.js", script.as_bytes());
    Fixture {
        repo: tempfile::tempdir().expect("repo dir"),
        workflow,
    }
}

async fn run(
    fixture: &Fixture,
    operation: serde_json::Value,
) -> butterflow_execution_bridge::OperationCompletion {
    run_with_context(
        fixture.path(),
        operation,
        serde_json::json!({ "scriptRoot": fixture.workflow.path() }),
    )
    .await
}

async fn run_with_context(
    dir: &Path,
    operation: serde_json::Value,
    context: serde_json::Value,
) -> butterflow_execution_bridge::OperationCompletion {
    let mut request = serde_json::json!({
        "protocolVersion": 2,
        "commandId": "migrate",
        "operation": operation,
    });
    if !context.is_null() {
        request["context"] = context;
    }
    let request = parse_request(&request.to_string()).expect("request parses");
    execute_in(&DirectRunner::with_quiet(true), &request, dir).await
}

fn jssg(extra: serde_json::Value) -> serde_json::Value {
    let mut operation = serde_json::json!({
        "kind": "jssg",
        "script": "transform.js",
        "language": "typescript",
    });
    for (key, value) in extra.as_object().expect("object") {
        operation[key] = value.clone();
    }
    operation
}

fn files_output(completion: &butterflow_execution_bridge::OperationCompletion) -> Vec<String> {
    completion
        .output
        .as_ref()
        .expect("output")
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item["file"].as_str().expect("file").to_string())
        .collect()
}

#[tokio::test]
async fn missing_include_derives_language_extensions_like_the_engine() {
    let dir = repo(REPLACE_SCRIPT);
    write(dir.path(), "src/b.ts", b"oldApi('b');\n");
    write(dir.path(), "src/a.js", b"oldApi('a');\n");
    write(dir.path(), "src/notes.md", b"oldApi('md');\n");
    write(dir.path(), "src/blob.bin", &[0xff, 0xfe, b'o', b'l', b'd']);

    let completion = run(&dir, jssg(serde_json::json!({}))).await;

    assert_eq!(
        completion.status,
        CompletionStatus::Succeeded,
        "{completion:?}"
    );
    assert_eq!(files_output(&completion), vec!["src/a.js", "src/b.ts"]);
    assert_eq!(read(dir.path(), "src/a.js"), "newApi('a');\n");
    assert_eq!(read(dir.path(), "src/b.ts"), "newApi('b');\n");
    assert_eq!(read(dir.path(), "src/notes.md"), "oldApi('md');\n");
    assert!(!dir.path().join("transform.js").exists());
}

#[tokio::test]
async fn invalid_utf8_language_files_are_skipped_not_fatal() {
    let dir = repo(REPLACE_SCRIPT);
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");
    write(dir.path(), "src/latin1.ts", &[b'o', b'l', b'd', 0xe9, b';']);

    let completion = run(&dir, jssg(serde_json::json!({}))).await;

    assert_eq!(
        completion.status,
        CompletionStatus::Succeeded,
        "{completion:?}"
    );
    assert_eq!(files_output(&completion), vec!["src/a.ts"]);
}

#[tokio::test]
async fn walker_visits_hidden_files_and_honors_gitignore_without_a_repository() {
    let dir = repo(REPLACE_SCRIPT);
    write(dir.path(), ".gitignore", b"ignored/\n");
    write(dir.path(), ".hidden/h.ts", b"oldApi('h');\n");
    write(dir.path(), "ignored/i.ts", b"oldApi('i');\n");
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");

    let completion = run(&dir, jssg(serde_json::json!({ "include": ["**/*.ts"] }))).await;

    assert_eq!(
        completion.status,
        CompletionStatus::Succeeded,
        "{completion:?}"
    );
    assert_eq!(files_output(&completion), vec![".hidden/h.ts", "src/a.ts"]);
    assert_eq!(read(dir.path(), ".hidden/h.ts"), "newApi('h');\n");
    assert_eq!(read(dir.path(), "ignored/i.ts"), "oldApi('i');\n");
}

#[tokio::test]
async fn configuration_and_pre_write_errors_are_failed() {
    let dir = repo(REPLACE_SCRIPT);
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");

    let cases = [
        (
            jssg(serde_json::json!({ "language": "klingon" })),
            "invalid JSSG language",
        ),
        (
            jssg(serde_json::json!({ "script": "missing.js" })),
            "failed to resolve JSSG script",
        ),
        (
            jssg(serde_json::json!({ "include": ["["] })),
            "invalid include glob",
        ),
        (
            jssg(serde_json::json!({ "target": { "exclude": ["["] } })),
            "invalid exclude glob",
        ),
        (
            jssg(serde_json::json!({ "target": { "root": "nope" } })),
            "failed to resolve JSSG target root",
        ),
        (
            jssg(
                serde_json::json!({ "semanticAnalysis": { "mode": "workspace", "root": "nope" } }),
            ),
            "failed to resolve semanticAnalysis.root",
        ),
    ];
    for (operation, expected) in cases {
        let completion = run(&dir, operation.clone()).await;
        assert_eq!(completion.status, CompletionStatus::Failed, "{operation}");
        let message = completion.error.expect("error").message;
        assert!(message.contains(expected), "{operation}: {message}");
        assert_eq!(
            read(dir.path(), "src/a.ts"),
            "oldApi('a');\n",
            "{operation}"
        );
    }
}

#[tokio::test]
async fn selector_errors_are_failed() {
    let dir = repo(
        r#"export function getSelector() { throw new Error("selector exploded"); }
export default async function transform(root) { return null; }"#,
    );
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");

    let completion = run(&dir, jssg(serde_json::json!({}))).await;

    assert_eq!(
        completion.status,
        CompletionStatus::Failed,
        "{completion:?}"
    );
    let message = completion.error.expect("error").message;
    assert!(
        message.contains("failed to load JSSG selector"),
        "{message}"
    );
    assert!(message.contains("selector exploded"), "{message}");
}

#[tokio::test]
async fn transform_error_before_any_write_is_failed() {
    let dir =
        repo(r#"export default async function transform() { throw new Error("first file"); }"#);
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");
    write(dir.path(), "src/b.ts", b"oldApi('b');\n");

    let completion = run(&dir, jssg(serde_json::json!({}))).await;

    assert_eq!(
        completion.status,
        CompletionStatus::Failed,
        "{completion:?}"
    );
    let message = completion.error.expect("error").message;
    assert!(message.contains("JSSG failed for"), "{message}");
    assert!(message.contains("first file"), "{message}");
    assert_eq!(read(dir.path(), "src/a.ts"), "oldApi('a');\n");
}

#[tokio::test]
async fn transform_error_after_a_write_is_unknown() {
    let dir = repo(
        r#"export default async function transform(root) {
  if (root.relativeFilename().endsWith("b.ts")) throw new Error("second file");
  return root.root().text().replaceAll("oldApi", "newApi");
}"#,
    );
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");
    write(dir.path(), "src/b.ts", b"oldApi('b');\n");

    let completion = run(&dir, jssg(serde_json::json!({}))).await;

    assert_eq!(
        completion.status,
        CompletionStatus::Unknown,
        "{completion:?}"
    );
    let message = completion.error.expect("error").message;
    assert!(message.contains("second file"), "{message}");
    assert_eq!(read(dir.path(), "src/a.ts"), "newApi('a');\n");
    assert_eq!(read(dir.path(), "src/b.ts"), "oldApi('b');\n");
}

#[tokio::test]
async fn legacy_string_results_yield_no_output_entries() {
    let dir = repo(
        r#"export default async function transform(root) {
  return root.root().text().replaceAll("oldApi", "newApi");
}"#,
    );
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");

    let completion = run(&dir, jssg(serde_json::json!({}))).await;

    assert_eq!(
        completion.status,
        CompletionStatus::Succeeded,
        "{completion:?}"
    );
    assert_eq!(completion.output, Some(serde_json::json!([])));
    assert_eq!(read(dir.path(), "src/a.ts"), "newApi('a');\n");
}

#[tokio::test]
async fn renamed_sources_are_removed_only_after_every_file_ran() {
    // While processing a.ts the transform renames b.ts (a later file) through
    // jssgTransform. b.ts must still be processed as enumerated and only then
    // removed, matching the workflow engine's deferred deletion.
    let dir = repo(
        r#"import { jssgTransform } from "codemod:ast-grep";
export default async function transform(root, options) {
  const file = root.relativeFilename().replaceAll("\\", "/");
  if (file === "src/a.ts") {
    await jssgTransform(
      async (secondary) => { secondary.rename("moved/b.ts"); return null; },
      options.targetDir + "/src/b.ts",
      "typescript",
    );
  }
  return { content: root.root().text().replaceAll("oldApi", "newApi"), output: { file } };
}"#,
    );
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");
    write(dir.path(), "src/b.ts", b"oldApi('b');\n");

    let completion = run(
        &dir,
        jssg(serde_json::json!({ "include": ["src/**/*.ts"] })),
    )
    .await;

    assert_eq!(
        completion.status,
        CompletionStatus::Succeeded,
        "{completion:?}"
    );
    assert_eq!(files_output(&completion), vec!["src/a.ts", "src/b.ts"]);
    assert_eq!(read(dir.path(), "src/a.ts"), "newApi('a');\n");
    assert!(
        !dir.path().join("src/b.ts").exists(),
        "renamed source removed at the end"
    );
    assert_eq!(read(dir.path(), "src/moved/b.ts"), "oldApi('b');\n");
}

#[tokio::test]
async fn relative_scripts_resolve_against_the_context_script_root() {
    let workflow_dir = tempfile::tempdir().expect("workflow dir");
    write(
        workflow_dir.path(),
        "scripts/transform.js",
        REPLACE_SCRIPT.as_bytes(),
    );
    let dir = tempfile::tempdir().expect("repo");
    write(dir.path(), "src/a.ts", b"oldApi('a');\n");
    let operation = jssg(serde_json::json!({ "script": "scripts/transform.js" }));

    let without = run_with_context(dir.path(), operation.clone(), serde_json::Value::Null).await;
    assert_eq!(without.status, CompletionStatus::Failed, "{without:?}");
    assert!(without
        .error
        .expect("error")
        .message
        .contains("failed to resolve JSSG script"));
    assert_eq!(read(dir.path(), "src/a.ts"), "oldApi('a');\n");

    let with = run_with_context(
        dir.path(),
        operation,
        serde_json::json!({ "scriptRoot": workflow_dir.path() }),
    )
    .await;
    assert_eq!(with.status, CompletionStatus::Succeeded, "{with:?}");
    assert_eq!(files_output(&with), vec!["src/a.ts"]);
    assert_eq!(read(dir.path(), "src/a.ts"), "newApi('a');\n");
}
