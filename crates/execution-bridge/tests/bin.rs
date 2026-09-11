//! Tests for the binary: the one-shot file protocol and the JSONL worker mode
//! over real pipes.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use butterflow_execution_bridge::{worker::WorkerResponse, CompletionStatus, OperationCompletion};

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

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Worker {
    fn spawn() -> Self {
        let mut child = Command::new(BIN)
            .arg("--jssg-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("worker spawns");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn send(&mut self, message: serde_json::Value) -> WorkerResponse {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line}: {e}"))
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait(mut self) -> i32 {
        self.child.wait().expect("wait").code().unwrap_or(-1)
    }
}

fn workspace() -> (tempfile::TempDir, tempfile::TempDir) {
    let workflow = tempfile::tempdir().expect("workflow");
    std::fs::write(
        workflow.path().join("transform.js"),
        r#"export default async function transform(root) {
  return { content: root.root().text().replaceAll("old", "new"), output: { file: root.relativeFilename() } };
}"#,
    )
    .expect("script");
    let repo = tempfile::tempdir().expect("repo");
    std::fs::write(repo.path().join("a.ts"), "old();\n").expect("a");
    (workflow, repo)
}

fn open(workflow: &Path, repo: &Path) -> serde_json::Value {
    serde_json::json!({
        "type": "open",
        "protocolVersion": 3,
        "script": "transform.js",
        "scriptRoot": workflow,
        "language": "typescript",
        "targetRoot": repo,
    })
}

#[test]
fn worker_mode_answers_jsonl_over_stdio_and_exits_on_close() {
    let (workflow, repo) = workspace();
    let mut worker = Worker::spawn();
    assert!(matches!(
        worker.send(open(workflow.path(), repo.path())),
        WorkerResponse::Opened { .. }
    ));
    match worker
        .send(serde_json::json!({ "type": "transform", "path": "a.ts", "content": "old();\n" }))
    {
        WorkerResponse::Transformed { result } => {
            assert_eq!(result.output, Some(serde_json::json!({ "file": "a.ts" })));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        worker.send(serde_json::json!({ "type": "close" })),
        WorkerResponse::Closed {}
    );
    assert_eq!(worker.wait(), 0);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.ts")).expect("a"),
        "old();\n"
    );
}

#[test]
fn worker_exits_when_the_host_closes_its_stdin() {
    let (workflow, repo) = workspace();
    let mut worker = Worker::spawn();
    assert!(matches!(
        worker.send(open(workflow.path(), repo.path())),
        WorkerResponse::Opened { .. }
    ));
    worker.close_stdin();
    assert_eq!(worker.wait(), 0);
}

#[test]
fn worker_protocol_errors_exit_nonzero_after_an_error_line() {
    let mut worker = Worker::spawn();
    match worker.send(serde_json::json!({ "type": "transform", "path": "a.ts", "content": "" })) {
        WorkerResponse::Error { fatal, .. } => assert!(fatal),
        other => panic!("{other:?}"),
    }
    assert_eq!(worker.wait(), 3);
}

#[cfg(unix)]
#[test]
fn writes_a_succeeded_completion_for_an_exec_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request = r#"{"protocolVersion":3,"commandId":"hello","operation":{"kind":"exec","command":"printf ok"}}"#;
    let (code, completion) = run_bridge(dir.path(), request);
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.command_id, "hello");
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(completion.output.unwrap()["stdout"], "ok\n");
}

#[test]
fn one_shot_jssg_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request = r#"{"protocolVersion":3,"commandId":"migrate","operation":{"kind":"jssg","script":"t.js","language":"typescript"}}"#;
    let (code, completion) = run_bridge(dir.path(), request);
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.status, CompletionStatus::Failed);
    assert!(completion
        .error
        .unwrap()
        .message
        .contains("worker protocol"));
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
fn wrong_arguments_exit_with_usage_code() {
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
