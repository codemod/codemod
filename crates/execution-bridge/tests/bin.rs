//! The binary over real files: exit codes and one request of each kind.

use std::path::Path;
use std::process::Command;

use butterflow_execution_bridge::{CompletionStatus, OperationCompletion, PROTOCOL_VERSION};
use serde_json::json;

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

#[cfg(unix)]
#[test]
fn exec_writes_a_succeeded_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "commandId": "hello",
        "operation": { "kind": "exec", "command": "printf ok" },
    });
    let (code, completion) = run_bridge(dir.path(), &request.to_string());
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.command_id, "hello");
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(completion.output.unwrap()["stdout"], "ok\n");
}

#[test]
fn jssg_transforms_the_supplied_files_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workflow = dir.path().join("workflow");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&workflow).expect("workflow");
    std::fs::create_dir_all(&repo).expect("repo");
    std::fs::write(
        workflow.join("transform.js"),
        r#"export default async function transform(root) {
  return { content: root.root().text().replaceAll("old", "new"), output: { file: root.relativeFilename() } };
}"#,
    )
    .expect("script");
    std::fs::write(repo.join("a.ts"), "old();\n").expect("a");
    let request = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "commandId": "migrate",
        "operation": { "kind": "jssg", "script": "transform.js", "language": "typescript" },
        "context": {
            "scriptRoot": workflow,
            "targetRoot": repo,
            "files": [{ "path": "a.ts", "content": "old();\n" }],
        },
    });
    let (code, completion) = run_bridge(dir.path(), &request.to_string());
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(
        completion.output.unwrap()["files"],
        json!([{
            "path": "a.ts",
            "edits": [{ "path": "a.ts", "content": "new();\n" }],
            "output": { "file": "a.ts" },
        }])
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("a.ts")).unwrap(),
        "old();\n"
    );
}

#[test]
fn malformed_requests_and_wrong_arguments_exit_nonzero() {
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

    assert_eq!(Command::new(BIN).status().expect("runs").code(), Some(2));
    assert_eq!(
        Command::new(BIN)
            .arg("--nope")
            .status()
            .expect("runs")
            .code(),
        Some(2)
    );
}
