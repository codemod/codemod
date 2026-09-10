use std::path::Path;

use butterflow_execution_bridge::{
    completion_from_result, execute, parse_request, CompletionStatus, Operation,
    OperationCompletion, OperationRequest, PROTOCOL_VERSION,
};
use butterflow_models::Error;
use butterflow_runners::direct_runner::DirectRunner;
use serde_json::Value;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/orchestration/fixtures/protocol"
);

fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(FIXTURES).join(name))
        .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"))
}

#[cfg(unix)]
fn exec_request(command_id: &str, command: &str) -> OperationRequest {
    OperationRequest {
        protocol_version: PROTOCOL_VERSION,
        command_id: command_id.to_string(),
        operation: Operation::Exec {
            command: command.to_string(),
            env: Default::default(),
        },
    }
}

#[test]
fn request_fixtures_round_trip_to_identical_json() {
    for name in ["exec-request.json", "jssg-request.json"] {
        let text = fixture(name);
        let request = parse_request(&text).expect("fixture should parse");
        let expected: Value = serde_json::from_str(&text).expect("fixture is JSON");
        let actual = serde_json::to_value(&request).expect("request serializes");
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn exec_request_fixture_carries_command_and_env() {
    let request = parse_request(&fixture("exec-request.json")).expect("parse");
    assert_eq!(request.command_id, "inspect");
    match request.operation {
        Operation::Exec { command, env } => {
            assert_eq!(command, "printf '{\"needsMigration\":true}'");
            assert_eq!(env.get("CI").map(String::as_str), Some("1"));
        }
        other => panic!("expected exec operation, got {}", other.kind()),
    }
}

#[test]
fn completion_fixtures_round_trip_to_identical_json() {
    let cases = [
        ("succeeded-completion.json", CompletionStatus::Succeeded),
        ("failed-completion.json", CompletionStatus::Failed),
        ("cancelled-completion.json", CompletionStatus::Cancelled),
        ("unknown-completion.json", CompletionStatus::Unknown),
    ];
    for (name, status) in cases {
        let text = fixture(name);
        let completion: OperationCompletion =
            serde_json::from_str(&text).expect("fixture should parse");
        assert_eq!(completion.protocol_version, PROTOCOL_VERSION);
        assert_eq!(completion.status, status, "{name}");
        let expected: Value = serde_json::from_str(&text).expect("fixture is JSON");
        let actual = serde_json::to_value(&completion).expect("completion serializes");
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn parse_request_rejects_other_protocol_versions() {
    let text = fixture("exec-request.json");
    let text = text.replace("\"protocolVersion\": 1", "\"protocolVersion\": 2");
    let error = parse_request(&text).expect_err("version 2 must be rejected");
    assert!(error.contains("unsupported protocolVersion 2"), "{error}");
}

#[test]
fn success_converts_to_stdout_output() {
    let completion = completion_from_result("inspect", Ok("hello\n".to_string()));
    let mut expected: Value = serde_json::from_str(&fixture("succeeded-completion.json")).unwrap();
    expected["output"]["stdout"] = Value::String("hello\n".to_string());
    assert_eq!(serde_json::to_value(&completion).unwrap(), expected);
}

#[test]
fn shell_failure_converts_to_failed_with_exit_code() {
    let completion = completion_from_result(
        "inspect",
        Err(Error::ShellCommandFailed {
            exit_code: 3,
            output: "boom\n".to_string(),
        }),
    );
    let expected: Value = serde_json::from_str(&fixture("failed-completion.json")).unwrap();
    assert_eq!(serde_json::to_value(&completion).unwrap(), expected);
}

#[test]
fn other_runner_errors_convert_to_unknown() {
    let completion = completion_from_result(
        "inspect",
        Err(Error::Runtime("Failed to wait for command".to_string())),
    );
    let expected: Value = serde_json::from_str(&fixture("unknown-completion.json")).unwrap();
    assert_eq!(serde_json::to_value(&completion).unwrap(), expected);
}

#[tokio::test]
async fn non_exec_operations_are_rejected_without_running() {
    let request = parse_request(&fixture("jssg-request.json")).expect("parse");
    let completion = execute(&DirectRunner::with_quiet(true), &request).await;
    assert_eq!(completion.status, CompletionStatus::Failed);
    assert!(completion.error.unwrap().message.contains("jssg"));
}

#[cfg(unix)]
#[tokio::test]
async fn exec_runs_through_direct_runner() {
    let request = exec_request("hello", "printf '{\"ok\":true}'");
    let completion = execute(&DirectRunner::with_quiet(true), &request).await;
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(completion.command_id, "hello");
    assert_eq!(
        completion.output.unwrap()["stdout"],
        Value::String("{\"ok\":true}\n".to_string())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn exec_request_env_reaches_the_command() {
    let mut request = exec_request("env", "printf '%s' \"$BRIDGE_TEST\"");
    if let Operation::Exec { env, .. } = &mut request.operation {
        env.insert("BRIDGE_TEST".to_string(), "from-request".to_string());
    }
    let completion = execute(&DirectRunner::with_quiet(true), &request).await;
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(
        completion.output.unwrap()["stdout"],
        Value::String("from-request\n".to_string())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn exec_nonzero_exit_is_failed() {
    let request = exec_request("bad", "echo boom >&2; exit 3");
    let completion = execute(&DirectRunner::with_quiet(true), &request).await;
    assert_eq!(completion.status, CompletionStatus::Failed);
    let error = completion.error.unwrap();
    assert_eq!(error.exit_code, Some(3));
    assert_eq!(error.output.as_deref(), Some("boom\n"));
}
