use std::path::Path;

use butterflow_execution_bridge::{
    completion_from_result, execute, parse_request, CompletionStatus, Operation,
    OperationCompletion, OperationRequest, RequestContext, SemanticAnalysis,
    SemanticAnalysisDetails, SemanticMode, Target, PROTOCOL_VERSION,
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
        context: None,
    }
}

fn jssg_request(operation: &str) -> String {
    format!(r#"{{"protocolVersion":2,"commandId":"t","operation":{operation}}}"#)
}

#[test]
fn request_fixtures_round_trip_to_identical_json() {
    for name in [
        "exec-request.json",
        "jssg-request.json",
        "jssg-target-request.json",
    ] {
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
fn jssg_request_fixture_has_no_target() {
    let request = parse_request(&fixture("jssg-request.json")).expect("parse");
    match request.operation {
        Operation::Jssg {
            script,
            language,
            include,
            semantic_analysis,
            target,
            input,
            ..
        } => {
            assert_eq!(script, "scripts/migrate.ts");
            assert_eq!(language, "tsx");
            assert_eq!(include, Some(vec!["**/*.tsx".to_string()]));
            assert_eq!(
                semantic_analysis,
                Some(SemanticAnalysis::Mode(SemanticMode::Workspace))
            );
            assert_eq!(target, None);
            assert_eq!(input, Some(serde_json::json!({ "needsMigration": true })));
        }
        other => panic!("expected jssg operation, got {}", other.kind()),
    }
}

#[test]
fn jssg_target_request_fixture_decodes_root_include_and_exclude() {
    let request = parse_request(&fixture("jssg-target-request.json")).expect("parse");
    assert_eq!(request.command_id, "rename-api");
    match request.operation {
        Operation::Jssg {
            script,
            language,
            include,
            exclude,
            target,
            input,
            ..
        } => {
            assert_eq!(script, "scripts/rename-api.ts");
            assert_eq!(language, "typescript");
            assert_eq!(include, Some(vec!["**/*.ts".to_string()]));
            assert_eq!(exclude, Some(vec!["**/*.d.ts".to_string()]));
            assert_eq!(
                target,
                Some(Target {
                    root: Some("apps/web".to_string()),
                    include: Some(vec!["src/**".to_string()]),
                    exclude: Some(vec!["**/generated/**".to_string()]),
                })
            );
            assert_eq!(input, None);
        }
        other => panic!("expected jssg operation, got {}", other.kind()),
    }
}

#[test]
fn partial_targets_omit_absent_fields_when_serialized() {
    let text = r#"{"protocolVersion":2,"commandId":"t","operation":{"kind":"jssg","script":"p.ts","language":"typescript","target":{"root":"packages/a"}}}"#;
    let request = parse_request(text).expect("parse");
    let value = serde_json::to_value(&request).expect("serialize");
    assert_eq!(
        value["operation"]["target"],
        serde_json::json!({ "root": "packages/a" })
    );
}

#[test]
fn malformed_targets_are_rejected() {
    for target in [
        r#""apps/web""#,
        r#"{"root":1}"#,
        r#"{"include":"src/**"}"#,
        r#"{"exclude":[null]}"#,
        r#"{"root":"apps/web","files":["a.ts"]}"#,
    ] {
        let text = format!(
            r#"{{"protocolVersion":2,"commandId":"t","operation":{{"kind":"jssg","script":"p.ts","language":"typescript","target":{target}}}}}"#
        );
        let error = parse_request(&text).expect_err("malformed target must not parse");
        assert!(error.contains("invalid request JSON"), "{target}: {error}");
    }
}

#[test]
fn exec_and_ai_operations_reject_a_target() {
    // Only `jssg` carries a target. A target on `exec` or `ai` is a parse error,
    // never a silently dropped field, so it cannot reach the runner unenforced.
    for operation in [
        r#"{"kind":"exec","command":"true","target":{"root":"apps"}}"#,
        r#"{"kind":"ai","prompt":"summarize","target":{"root":"apps"}}"#,
    ] {
        let text = format!(r#"{{"protocolVersion":2,"commandId":"t","operation":{operation}}}"#);
        let error = parse_request(&text).expect_err("target on exec/ai must not parse");
        assert!(
            error.contains("invalid request JSON"),
            "{operation}: {error}"
        );
        assert!(
            error.contains("unknown field `target`"),
            "{operation}: {error}"
        );
    }
}

#[test]
fn operations_reject_fields_from_other_variants() {
    for operation in [
        r#"{"kind":"exec","command":"true","package":"p"}"#,
        r#"{"kind":"jssg","script":"p.ts","language":"typescript","command":"true"}"#,
        r#"{"kind":"ai","prompt":"x","env":{}}"#,
    ] {
        let text = format!(r#"{{"protocolVersion":2,"commandId":"t","operation":{operation}}}"#);
        let error = parse_request(&text).expect_err("unknown operation field must not parse");
        assert!(error.contains("unknown field"), "{operation}: {error}");
    }
}

#[test]
fn semantic_analysis_rejects_unknown_fields() {
    let text = r#"{"protocolVersion":2,"commandId":"t","operation":{"kind":"jssg","script":"p.ts","language":"typescript","semanticAnalysis":{"mode":"workspace","threads":4}}}"#;
    let error = parse_request(text).expect_err("unknown semantic field must not parse");
    assert!(error.contains("invalid request JSON"), "{error}");
}

#[test]
fn jssg_definition_rejects_invalid_intrinsic_fields() {
    for operation in [
        r#"{"kind":"jssg","script":"","language":"typescript"}"#,
        r#"{"kind":"jssg","script":"p.ts","language":" "}"#,
        r#"{"kind":"jssg","script":"p.ts","language":"typescript","include":[] }"#,
        r#"{"kind":"jssg","script":"p.ts","language":"typescript","exclude":[" "] }"#,
        r#"{"kind":"jssg","script":"p.ts","language":"typescript","semanticAnalysis":{"mode":"file","root":"src"}}"#,
    ] {
        assert!(
            parse_request(&jssg_request(operation)).is_err(),
            "{operation}"
        );
    }
}

#[test]
fn jssg_paths_must_be_safe_and_relative_on_every_platform() {
    // Same rules as `isSafeRelativePath` in packages/orchestration/src/paths.ts.
    for bad in [
        "/abs/p.ts",
        "\\\\server\\p.ts",
        "C:\\p.ts",
        "c:/p.ts",
        "../p.ts",
        "scripts/../p.ts",
        "scripts\\..\\p.ts",
    ] {
        let escaped = bad.replace('\\', "\\\\");
        for operation in [
            format!(r#"{{"kind":"jssg","script":"{escaped}","language":"typescript"}}"#),
            format!(
                r#"{{"kind":"jssg","script":"p.ts","language":"typescript","target":{{"root":"{escaped}"}}}}"#
            ),
            format!(
                r#"{{"kind":"jssg","script":"p.ts","language":"typescript","semanticAnalysis":{{"mode":"workspace","root":"{escaped}"}}}}"#
            ),
        ] {
            let error = parse_request(&jssg_request(&operation)).expect_err("must reject");
            assert!(error.contains("safe relative path"), "{operation}: {error}");
        }
    }
    // A `..` inside a segment is an ordinary name, not an escape.
    let ok = r#"{"kind":"jssg","script":"scripts/foo..bar.ts","language":"typescript","target":{"root":"apps/a..b"},"semanticAnalysis":{"mode":"workspace","root":"src..gen"}}"#;
    parse_request(&jssg_request(ok)).expect("foo..bar is a valid name");
}

#[test]
fn semantic_details_omit_absent_root_when_serialized() {
    let text = jssg_request(
        r#"{"kind":"jssg","script":"p.ts","language":"typescript","semanticAnalysis":{"mode":"workspace"}}"#,
    );
    let request = parse_request(&text).expect("parse");
    match &request.operation {
        Operation::Jssg {
            semantic_analysis, ..
        } => assert_eq!(
            semantic_analysis,
            &Some(SemanticAnalysis::Detailed(SemanticAnalysisDetails {
                mode: SemanticMode::Workspace,
                root: None,
            }))
        ),
        other => panic!("expected jssg operation, got {}", other.kind()),
    }
    let value = serde_json::to_value(&request).expect("serialize");
    assert_eq!(
        value["operation"]["semanticAnalysis"],
        serde_json::json!({ "mode": "workspace" })
    );
    let file_only = jssg_request(
        r#"{"kind":"jssg","script":"p.ts","language":"typescript","semanticAnalysis":{"mode":"file"}}"#,
    );
    let value = serde_json::to_value(parse_request(&file_only).expect("parse")).expect("json");
    assert_eq!(
        value["operation"]["semanticAnalysis"],
        serde_json::json!({ "mode": "file" })
    );
}

#[test]
fn request_context_is_optional_strict_and_never_serialized_when_absent() {
    let without = parse_request(&fixture("jssg-request.json")).expect("parse");
    assert_eq!(without.context, None);
    let value = serde_json::to_value(&without).expect("serialize");
    assert!(value.get("context").is_none());

    let text = r#"{"protocolVersion":2,"commandId":"t","operation":{"kind":"jssg","script":"p.ts","language":"typescript"},"context":{"scriptRoot":"/tmp/workflow"}}"#;
    let request = parse_request(text).expect("parse");
    assert_eq!(
        request.context,
        Some(RequestContext {
            script_root: Some("/tmp/workflow".to_string())
        })
    );
    let value = serde_json::to_value(&request).expect("serialize");
    assert_eq!(
        value["context"],
        serde_json::json!({ "scriptRoot": "/tmp/workflow" })
    );

    let empty = r#"{"protocolVersion":2,"commandId":"t","operation":{"kind":"exec","command":"true"},"context":{}}"#;
    assert_eq!(
        parse_request(empty).expect("empty context parses").context,
        Some(RequestContext::default())
    );
    let blank = r#"{"protocolVersion":2,"commandId":"t","operation":{"kind":"exec","command":"true"},"context":{"scriptRoot":" "}}"#;
    assert!(parse_request(blank)
        .expect_err("blank root")
        .contains("scriptRoot"));
    let unknown = r#"{"protocolVersion":2,"commandId":"t","operation":{"kind":"exec","command":"true"},"context":{"cwd":"/tmp"}}"#;
    assert!(parse_request(unknown)
        .expect_err("unknown context field")
        .contains("unknown field"));
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
    let text = text.replace("\"protocolVersion\": 2", "\"protocolVersion\": 99");
    let error = parse_request(&text).expect_err("version 99 must be rejected");
    assert!(error.contains("unsupported protocolVersion 99"), "{error}");
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
async fn ai_operations_are_rejected_without_running() {
    let request = OperationRequest {
        protocol_version: PROTOCOL_VERSION,
        command_id: "ai".to_string(),
        operation: Operation::Ai {
            prompt: "summarize".to_string(),
            input: None,
        },
        context: None,
    };
    let completion = execute(&DirectRunner::with_quiet(true), &request).await;
    assert_eq!(completion.status, CompletionStatus::Failed);
    assert!(completion
        .error
        .expect("error")
        .message
        .contains("no executor adapter"));
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
async fn exec_preserves_direct_runners_combined_unix_output() {
    let request = exec_request("stdio", "printf out; printf err >&2");
    let completion = execute(&DirectRunner::with_quiet(true), &request).await;
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(
        completion.output.unwrap()["stdout"],
        Value::String("outerr\n".to_string())
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
