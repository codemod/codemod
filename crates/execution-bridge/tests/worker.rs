//! The JSONL worker loop over in-memory streams: message strictness, fatal
//! versus recoverable errors, and the open/index/transform/close lifecycle.

use std::io::Cursor;
use std::path::Path;

use butterflow_execution_bridge::{
    worker::{run_worker, WorkerRequest, WorkerResponse, EXIT_OK, EXIT_PROTOCOL},
    PROTOCOL_VERSION,
};

const SCRIPT: &str = r#"export default async function transform(root) {
  if (root.root().text().includes("boom")) throw new Error("boom");
  return { content: root.root().text().replaceAll("old", "new"), output: { file: root.relativeFilename() } };
}"#;

struct Fixture {
    repo: tempfile::TempDir,
    workflow: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let workflow = tempfile::tempdir().expect("workflow");
        std::fs::write(workflow.path().join("transform.js"), SCRIPT).expect("script");
        let repo = tempfile::tempdir().expect("repo");
        std::fs::write(repo.path().join("a.ts"), "old();\n").expect("a");
        Self { repo, workflow }
    }

    fn open(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "open",
            "protocolVersion": PROTOCOL_VERSION,
            "script": "transform.js",
            "scriptRoot": self.workflow.path(),
            "language": "typescript",
            "targetRoot": self.repo.path(),
        })
    }
}

fn run(lines: &[serde_json::Value]) -> (u8, Vec<WorkerResponse>) {
    run_text(
        &lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn run_text(input: &str) -> (u8, Vec<WorkerResponse>) {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let mut output = Vec::new();
    let code = run_worker(
        runtime.handle(),
        Cursor::new(input.to_string()),
        &mut output,
    );
    let responses = String::from_utf8(output)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}")))
        .collect();
    (code, responses)
}

#[test]
fn lifecycle_open_index_transform_close() {
    let fixture = Fixture::new();
    let (code, responses) = run(&[
        fixture.open(),
        serde_json::json!({ "type": "index", "path": "a.ts", "content": "old();\n" }),
        serde_json::json!({ "type": "transform", "path": "a.ts", "content": "old();\n" }),
        serde_json::json!({ "type": "transform", "path": "a.ts", "content": "boom();\n" }),
        serde_json::json!({ "type": "transform", "path": "a.ts", "content": "old();\n" }),
        serde_json::json!({ "type": "close" }),
    ]);
    assert_eq!(code, EXIT_OK);
    assert_eq!(responses.len(), 6, "{responses:?}");
    match &responses[0] {
        WorkerResponse::Opened {
            protocol_version,
            extensions,
            semantic_mode,
        } => {
            assert_eq!(*protocol_version, PROTOCOL_VERSION);
            assert!(extensions.contains(&".ts".to_string()));
            assert_eq!(*semantic_mode, None);
        }
        other => panic!("expected opened, got {other:?}"),
    }
    assert_eq!(responses[1], WorkerResponse::Indexed {});
    match &responses[2] {
        WorkerResponse::Transformed { result } => {
            assert_eq!(result.output, Some(serde_json::json!({ "file": "a.ts" })));
        }
        other => panic!("expected transformed, got {other:?}"),
    }
    match &responses[3] {
        WorkerResponse::Error { message, fatal } => {
            assert!(message.contains("boom"), "{message}");
            assert!(!fatal);
        }
        other => panic!("expected recoverable error, got {other:?}"),
    }
    assert!(matches!(responses[4], WorkerResponse::Transformed { .. }));
    assert_eq!(responses[5], WorkerResponse::Closed {});
    assert_eq!(
        std::fs::read_to_string(fixture.repo.path().join("a.ts")).expect("a"),
        "old();\n"
    );
}

#[test]
fn eof_without_close_ends_cleanly() {
    let fixture = Fixture::new();
    let (code, responses) = run(&[fixture.open()]);
    assert_eq!(code, EXIT_OK);
    assert_eq!(responses.len(), 1);
    let (code, responses) = run_text("\n\n");
    assert_eq!(code, EXIT_OK);
    assert!(responses.is_empty());
}

#[test]
fn messages_before_open_are_fatal() {
    let (code, responses) = run(&[
        serde_json::json!({ "type": "transform", "path": "a.ts", "content": "" }),
        serde_json::json!({ "type": "close" }),
    ]);
    assert_eq!(code, EXIT_PROTOCOL);
    assert_eq!(
        responses,
        vec![WorkerResponse::Error {
            message: "transform before open".to_string(),
            fatal: true
        }]
    );
}

#[test]
fn unknown_fields_and_malformed_lines_are_fatal() {
    let fixture = Fixture::new();
    let mut open = fixture.open();
    open["target"] = serde_json::json!({ "root": "src" });
    let (code, responses) = run(&[open]);
    assert_eq!(code, EXIT_PROTOCOL);
    match &responses[0] {
        WorkerResponse::Error { message, fatal } => {
            assert!(message.contains("unknown field `target`"), "{message}");
            assert!(fatal);
        }
        other => panic!("{other:?}"),
    }

    let (code, responses) = run_text("{not json");
    assert_eq!(code, EXIT_PROTOCOL);
    assert!(matches!(
        &responses[0],
        WorkerResponse::Error { fatal: true, .. }
    ));

    let (code, responses) =
        run_text(r#"{"type":"transform","path":"a.ts","content":"x","extra":1}"#);
    assert_eq!(code, EXIT_PROTOCOL);
    assert!(matches!(
        &responses[0],
        WorkerResponse::Error { fatal: true, .. }
    ));
}

#[test]
fn wrong_protocol_version_and_double_open_are_fatal() {
    let fixture = Fixture::new();
    let mut open = fixture.open();
    open["protocolVersion"] = serde_json::json!(2);
    let (code, responses) = run(&[open]);
    assert_eq!(code, EXIT_PROTOCOL);
    match &responses[0] {
        WorkerResponse::Error { message, fatal } => {
            assert!(
                message.contains("unsupported protocolVersion 2"),
                "{message}"
            );
            assert!(fatal);
        }
        other => panic!("{other:?}"),
    }

    let (code, responses) = run(&[fixture.open(), fixture.open()]);
    assert_eq!(code, EXIT_PROTOCOL);
    assert!(matches!(&responses[0], WorkerResponse::Opened { .. }));
    assert_eq!(
        responses[1],
        WorkerResponse::Error {
            message: "session is already open".to_string(),
            fatal: true
        }
    );
}

#[test]
fn failed_open_is_fatal() {
    let fixture = Fixture::new();
    let mut open = fixture.open();
    open["script"] = serde_json::json!("missing.js");
    let (code, responses) = run(&[open, serde_json::json!({ "type": "close" })]);
    assert_eq!(code, EXIT_PROTOCOL);
    assert_eq!(responses.len(), 1);
    assert!(matches!(
        &responses[0],
        WorkerResponse::Error { fatal: true, .. }
    ));
}

#[test]
fn request_and_response_shapes_round_trip_strictly() {
    let close: WorkerRequest = serde_json::from_str(r#"{"type":"close"}"#).expect("close");
    assert_eq!(close, WorkerRequest::Close {});
    assert_eq!(
        serde_json::to_string(&close).expect("json"),
        r#"{"type":"close"}"#
    );
    assert!(serde_json::from_str::<WorkerRequest>(r#"{"type":"close","force":true}"#).is_err());
    assert!(serde_json::from_str::<WorkerRequest>(r#"{"type":"open"}"#).is_err());
    let indexed = serde_json::to_string(&WorkerResponse::Indexed {}).expect("json");
    assert_eq!(indexed, r#"{"type":"indexed"}"#);
    assert!(Path::new("a.ts").is_relative());
}
