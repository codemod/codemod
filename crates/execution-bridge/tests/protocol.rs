//! The wire contract shared with `packages/orchestration/src/protocol.ts`:
//! the JSON fixtures both sides check, strict decoding, and `exec` through
//! the real `DirectRunner`.

use std::path::Path;

use butterflow_execution_bridge::{
    completion_from_result, execute, parse_request, CompletionStatus, Operation,
    OperationCompletion, RequestContext, SemanticAnalysis, SemanticMode, Target, PROTOCOL_VERSION,
};
use butterflow_models::Error;
use butterflow_runners::direct_runner::DirectRunner;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/orchestration/fixtures/protocol"
);

fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(FIXTURES).join(name))
        .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"))
}

fn request(operation: &str) -> String {
    format!(r#"{{"protocolVersion":{PROTOCOL_VERSION},"commandId":"t","operation":{operation}}}"#)
}

#[test]
fn fixtures_round_trip_to_identical_json() {
    for name in [
        "exec-request.json",
        "jssg-request.json",
        "jssg-target-request.json",
    ] {
        let text = fixture(name);
        let parsed = parse_request(&text).expect(name);
        let expected: Value = serde_json::from_str(&text).expect("fixture is JSON");
        assert_eq!(
            serde_json::to_value(&parsed).expect("serializes"),
            expected,
            "{name}"
        );
    }
    for (name, status) in [
        ("succeeded-completion.json", CompletionStatus::Succeeded),
        ("failed-completion.json", CompletionStatus::Failed),
        ("cancelled-completion.json", CompletionStatus::Cancelled),
        ("unknown-completion.json", CompletionStatus::Unknown),
    ] {
        let text = fixture(name);
        let completion: OperationCompletion = serde_json::from_str(&text).expect(name);
        assert_eq!(completion.status, status, "{name}");
        let expected: Value = serde_json::from_str(&text).expect("fixture is JSON");
        assert_eq!(
            serde_json::to_value(&completion).expect("serializes"),
            expected,
            "{name}"
        );
    }
}

#[test]
fn jssg_fixtures_decode_every_field() {
    let plain = parse_request(&fixture("jssg-request.json")).expect("parse");
    assert_eq!(plain.context, None);
    let Operation::Jssg {
        script,
        language,
        include,
        semantic_analysis,
        target,
        input,
        ..
    } = plain.operation
    else {
        panic!("expected jssg");
    };
    assert_eq!(script, "scripts/migrate.ts");
    assert_eq!(language, "tsx");
    assert_eq!(include, Some(vec!["**/*.tsx".to_string()]));
    assert_eq!(
        semantic_analysis,
        Some(SemanticAnalysis::Mode(SemanticMode::Workspace))
    );
    assert_eq!(target, None);
    assert_eq!(input, Some(json!({ "needsMigration": true })));

    let targeted = parse_request(&fixture("jssg-target-request.json")).expect("parse");
    let Operation::Jssg { target, .. } = targeted.operation else {
        panic!("expected jssg");
    };
    assert_eq!(
        target,
        Some(Target {
            root: Some("apps/web".to_string()),
            include: Some(vec!["src/**".to_string()]),
            exclude: Some(vec!["**/generated/**".to_string()]),
        })
    );
}

#[test]
fn context_carries_roots_and_files_and_omits_absent_fields() {
    let text = format!(
        r#"{{"protocolVersion":{PROTOCOL_VERSION},"commandId":"t","operation":{{"kind":"jssg","script":"p.ts","language":"typescript","semanticAnalysis":{{"mode":"workspace"}}}},"context":{{"scriptRoot":"/w","targetRoot":"/r","files":[{{"path":"a.ts","content":"x"}}]}}}}"#
    );
    let parsed = parse_request(&text).expect("parse");
    let context = parsed.context.clone().expect("context");
    assert_eq!(context.script_root.as_deref(), Some("/w"));
    assert_eq!(context.target_root.as_deref(), Some("/r"));
    assert_eq!(context.files.as_ref().map(Vec::len), Some(1));
    let value = serde_json::to_value(&parsed).expect("serialize");
    assert_eq!(
        value["operation"]["semanticAnalysis"],
        json!({ "mode": "workspace" })
    );
    assert_eq!(
        value["context"]["files"][0],
        json!({ "path": "a.ts", "content": "x" })
    );
    assert_eq!(
        serde_json::to_value(RequestContext::default()).expect("serialize"),
        json!({})
    );
}

#[test]
fn decoding_is_strict() {
    let cases = [
        // (request body, expected error fragment)
        (
            request(r#"{"kind":"exec","command":"true","target":{"root":"apps"}}"#),
            "unknown field `target`",
        ),
        (
            request(r#"{"kind":"ai","prompt":"x","target":{"root":"apps"}}"#),
            "unknown field `target`",
        ),
        (
            request(r#"{"kind":"exec","command":"true","package":"p"}"#),
            "unknown field",
        ),
        (
            request(r#"{"kind":"jssg","script":"p.ts","language":"typescript","command":"true"}"#),
            "unknown field",
        ),
        (
            request(r#"{"kind":"jssg","script":"p.ts","language":"typescript","target":"apps"}"#),
            "invalid request JSON",
        ),
        (
            request(
                r#"{"kind":"jssg","script":"p.ts","language":"typescript","target":{"root":"a","files":[]}}"#,
            ),
            "unknown field `files`",
        ),
        (
            request(
                r#"{"kind":"jssg","script":"p.ts","language":"typescript","semanticAnalysis":{"mode":"workspace","threads":4}}"#,
            ),
            "invalid request JSON",
        ),
        (
            request(r#"{"kind":"exec","command":"true"}"#)
                .replace(r#""commandId""#, r#""cwd":"/","commandId""#),
            "unknown field `cwd`",
        ),
        (
            request(r#"{"kind":"exec","command":"true"}"#).replace(
                r#""commandId":"t""#,
                r#""commandId":"t","context":{"cwd":"/tmp"}"#,
            ),
            "unknown field `cwd`",
        ),
        (
            request(r#"{"kind":"exec","command":"true"}"#).replace(
                r#""commandId":"t""#,
                r#""commandId":"t","context":{"files":[{"path":"a","content":"","mode":1}]}"#,
            ),
            "unknown field `mode`",
        ),
        (
            fixture("exec-request.json").replace(
                &format!("\"protocolVersion\": {PROTOCOL_VERSION}"),
                "\"protocolVersion\": 99",
            ),
            "unsupported protocolVersion 99",
        ),
    ];
    for (text, expected) in cases {
        let error = parse_request(&text).expect_err(&text);
        assert!(error.contains(expected), "{text}: {error}");
    }
    let extra = format!(
        r#"{{"protocolVersion":{PROTOCOL_VERSION},"commandId":"x","status":"failed","error":{{"message":"m","stack":"s"}}}}"#
    );
    assert!(serde_json::from_str::<OperationCompletion>(&extra).is_err());
}

#[test]
fn runner_results_convert_to_completions() {
    let ok = completion_from_result("inspect", Ok("hello\n".to_string()));
    let mut expected: Value = serde_json::from_str(&fixture("succeeded-completion.json")).unwrap();
    expected["output"]["stdout"] = Value::String("hello\n".to_string());
    assert_eq!(serde_json::to_value(&ok).unwrap(), expected);

    let failed = completion_from_result(
        "inspect",
        Err(Error::ShellCommandFailed {
            exit_code: 3,
            output: "boom\n".to_string(),
        }),
    );
    let expected: Value = serde_json::from_str(&fixture("failed-completion.json")).unwrap();
    assert_eq!(serde_json::to_value(&failed).unwrap(), expected);

    let unknown = completion_from_result(
        "inspect",
        Err(Error::Runtime("Failed to wait for command".to_string())),
    );
    assert_eq!(unknown.status, CompletionStatus::Unknown);
    assert_eq!(
        unknown.error.expect("error").message,
        "Runtime error: Failed to wait for command"
    );
}

#[tokio::test]
async fn ai_is_refused() {
    let request = parse_request(&request(r#"{"kind":"ai","prompt":"summarize"}"#)).expect("parse");
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
    // (command, env, expected status, expected stdout / error output, exit code)
    let cases = [
        (
            "printf '{\"ok\":true}'",
            None,
            CompletionStatus::Succeeded,
            "{\"ok\":true}\n",
            None,
        ),
        // DirectRunner combines stdout and stderr on Unix.
        (
            "printf out; printf err >&2",
            None,
            CompletionStatus::Succeeded,
            "outerr\n",
            None,
        ),
        (
            "printf '%s' \"$BRIDGE_TEST\"",
            Some("from-request"),
            CompletionStatus::Succeeded,
            "from-request\n",
            None,
        ),
        (
            "echo boom >&2; exit 3",
            None,
            CompletionStatus::Failed,
            "boom\n",
            Some(3),
        ),
    ];
    for (command, env, status, text, exit_code) in cases {
        let env = env.map_or_else(String::new, |value| {
            format!(r#","env":{{"BRIDGE_TEST":"{value}"}}"#)
        });
        let request = parse_request(&request(&format!(
            r#"{{"kind":"exec","command":{}{env}}}"#,
            Value::String(command.to_string())
        )))
        .expect("parse");
        let completion = execute(&DirectRunner::with_quiet(true), &request).await;
        assert_eq!(completion.status, status, "{command}");
        assert_eq!(completion.command_id, "t");
        match status {
            CompletionStatus::Succeeded => {
                assert_eq!(completion.output.unwrap()["stdout"], text, "{command}");
            }
            _ => {
                let error = completion.error.unwrap();
                assert_eq!(error.exit_code, exit_code, "{command}");
                assert_eq!(error.output.as_deref(), Some(text), "{command}");
            }
        }
    }
}
