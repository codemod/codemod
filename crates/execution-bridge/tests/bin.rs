//! Tests for the one-shot file-protocol binary.

use std::path::Path;
use std::process::Command;

use butterflow_execution_bridge::{CompletionStatus, OperationCompletion};

const BIN: &str = env!("CARGO_BIN_EXE_butterflow-execution-bridge");

fn run_bridge(dir: &Path, request: &str) -> (i32, Option<OperationCompletion>) {
    let request_path = dir.join("request.json");
    let response_path = dir.join("response.json");
    std::fs::write(&request_path, request).expect("write request");
    let status = Command::new(BIN)
        .arg(&request_path)
        .arg(&response_path)
        .current_dir(dir)
        .status()
        .expect("bridge binary runs");
    let completion = std::fs::read_to_string(&response_path)
        .ok()
        .map(|text| serde_json::from_str(&text).expect("response is a completion"));
    (status.code().unwrap_or(-1), completion)
}

#[test]
fn executes_jssg_with_target_intersection_edits_and_ordered_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("app/src")).expect("src");
    std::fs::create_dir_all(dir.path().join("app/other")).expect("other");
    std::fs::write(dir.path().join("app/src/b.ts"), "oldApi('b');\n").expect("b");
    std::fs::write(dir.path().join("app/src/a.ts"), "oldApi('a');\n").expect("a");
    std::fs::write(
        dir.path().join("app/src/skip.generated.ts"),
        "oldApi('skip');\n",
    )
    .expect("generated");
    std::fs::write(
        dir.path().join("app/other/outside.ts"),
        "oldApi('outside');\n",
    )
    .expect("outside");
    std::fs::write(
        dir.path().join("transform.js"),
        r#"export function getSelector() {
  return { rule: { kind: "expression_statement" } };
}
export default async function transform(root, options) {
  const content = root.root().text();
  return {
    content: content.replaceAll("oldApi", "newApi"),
    output: { file: root.relativeFilename().replaceAll("\\", "/"), input: options.params.input },
  };
}"#,
    )
    .expect("script");
    let request = serde_json::json!({
        "protocolVersion": 2,
        "commandId": "migrate",
        "operation": {
            "kind": "jssg",
            "script": "transform.js",
            "language": "typescript",
            "include": ["app/**/*.ts"],
            "semanticAnalysis": "workspace",
            "input": "marker",
            "target": {
                "root": "app",
                "include": ["src/**"],
                "exclude": ["**/*.generated.ts"]
            }
        }
    })
    .to_string();

    let (code, completion) = run_bridge(dir.path(), &request);
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(
        completion.output,
        Some(serde_json::json!([
            { "file": "src/a.ts", "input": "marker" },
            { "file": "src/b.ts", "input": "marker" }
        ]))
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("app/src/a.ts")).expect("a result"),
        "newApi('a');\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("app/src/skip.generated.ts"))
            .expect("generated result"),
        "oldApi('skip');\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("app/other/outside.ts")).expect("outside result"),
        "oldApi('outside');\n"
    );
}

#[cfg(unix)]
#[test]
fn writes_a_succeeded_completion_for_an_exec_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request = r#"{"protocolVersion":2,"commandId":"hello","operation":{"kind":"exec","command":"printf ok"}}"#;
    let (code, completion) = run_bridge(dir.path(), request);
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.command_id, "hello");
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(completion.output.unwrap()["stdout"], "ok\n");
}

#[test]
fn malformed_request_exits_nonzero_and_writes_an_error_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request =
        r#"{"protocolVersion":7,"commandId":"bad","operation":{"kind":"exec","command":"true"}}"#;
    let (code, completion) = run_bridge(dir.path(), request);
    let completion = completion.expect("error response written");
    assert_eq!(code, 3);
    assert_eq!(completion.command_id, "bad");
    assert_eq!(completion.status, CompletionStatus::Failed);
    assert!(completion
        .error
        .unwrap()
        .message
        .contains("unsupported protocolVersion 7"));
}

#[test]
fn missing_arguments_exit_with_usage_code() {
    let status = Command::new(BIN).status().expect("bridge binary runs");
    assert_eq!(status.code(), Some(2));
}
