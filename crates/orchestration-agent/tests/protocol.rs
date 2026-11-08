//! The wire contract shared with `packages/orchestration/src/core/protocol.ts`
//! for `agent` operations: the JSON fixtures both sides check and strict
//! decoding.

use std::path::Path;

use orchestration_agent::external::{ClaudeCodeTool, CodexSandbox};
use orchestration_agent::{
    agent, parse_request, CompletionStatus, OperationCompletion, PROTOCOL_VERSION,
};
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
        "agent-request.json",
        "agent-claude-code-request.json",
        "agent-codex-request.json",
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
fn decoding_is_strict_and_agent_only() {
    let cases = [
        // (request body, expected error fragment)
        (fixture("shell-request.json"), "unknown variant `shell`"),
        (fixture("jssg-request.json"), "unknown variant `jssg`"),
        (
            fixture("assessment-request.json"),
            "unknown variant `assessment`",
        ),
        (
            fixture("agent-request.json").replace(
                &format!("\"protocolVersion\": {PROTOCOL_VERSION}"),
                "\"protocolVersion\": 99",
            ),
            "unsupported protocolVersion 99",
        ),
        (
            request(r#"{"kind":"agent","prompt":"x","backend":{"kind":"builtin","tools":[]}}"#)
                .replace(r#""commandId":"t""#, r#""commandId":"t","context":{}"#),
            "unknown field `context`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"builtin","tools":[]},"target":{"root":"apps"}}"#,
            ),
            "unknown field `target`",
        ),
        (
            request(r#"{"kind":"agent","prompt":"x"}"#),
            "missing field `backend`",
        ),
        // The pre-v8 shape: settings are backend-scoped now.
        (
            request(r#"{"kind":"agent","prompt":"x","tools":[]}"#),
            "unknown field `tools`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"builtin","tools":["shell"]}}"#,
            ),
            "unknown variant `shell`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"builtin","tools":[]},"responseFormat":"xml"}"#,
            ),
            "unknown variant `xml`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"builtin","tools":[],"maxSteps":-1}}"#,
            ),
            "invalid request JSON",
        ),
        // Unsupported backend settings are parse errors, not ignored.
        (
            request(r#"{"kind":"agent","prompt":"x","backend":{"kind":"lm-studio"}}"#),
            "unknown variant `lm-studio`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"claude-code","tools":["Read"],"maxSteps":3}}"#,
            ),
            "unknown field `maxSteps`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"claude-code","tools":["bash"]}}"#,
            ),
            "unknown variant `bash`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"codex","sandbox":"workspace-write","tools":[]}}"#,
            ),
            "unknown field `tools`",
        ),
        (
            request(
                r#"{"kind":"agent","prompt":"x","backend":{"kind":"codex","sandbox":"danger-full-access"}}"#,
            ),
            "unknown variant `danger-full-access`",
        ),
        (
            request(r#"{"kind":"agent","prompt":"x","backend":{"kind":"codex"}}"#),
            "missing field `sandbox`",
        ),
    ];
    for (text, expected) in cases {
        let error = parse_request(&text).expect_err(&text);
        assert!(error.contains(expected), "{text}: {error}");
    }
}

#[test]
fn agent_settings_follow_the_engine_llm_environment() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    };
    assert_eq!(
        agent::settings_from_env(env(&[])),
        Err("agent requires LLM_API_KEY".to_string())
    );
    assert_eq!(
        agent::settings_from_env(env(&[("LLM_API_KEY", "  ")])),
        Err("agent requires LLM_API_KEY".to_string())
    );
    assert_eq!(
        agent::settings_from_env(env(&[("LLM_API_KEY", "k")])),
        Ok(agent::AgentSettings {
            api_key: "k".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            endpoint: "https://api.openai.com/v1".to_string(),
        })
    );
    assert_eq!(
        agent::settings_from_env(env(&[
            ("LLM_API_KEY", "k"),
            ("LLM_PROVIDER", "anthropic"),
            ("LLM_MODEL", "claude-sonnet-4-5"),
        ])),
        Ok(agent::AgentSettings {
            api_key: "k".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            endpoint: "https://api.anthropic.com".to_string(),
        })
    );
    let custom = agent::settings_from_env(env(&[
        ("LLM_API_KEY", "k"),
        ("LLM_BASE_URL", "http://127.0.0.1:1/v1"),
    ]))
    .expect("settings");
    assert_eq!(custom.endpoint, "http://127.0.0.1:1/v1");
}

#[test]
fn agent_task_prompt_appends_input_and_the_json_requirement() {
    assert_eq!(agent::task_prompt("Fix it.", None, None), "Fix it.");
    assert_eq!(
        agent::task_prompt("Fix it.", Some(&json!({ "packages": ["web"] })), None),
        "Fix it.\n\nInput (JSON):\n```json\n{\n  \"packages\": [\n    \"web\"\n  ]\n}\n```"
    );
    assert_eq!(
        agent::task_prompt("Fix it.", None, Some(agent::ResponseFormat::Json)),
        format!("Fix it.\n\n{}", agent::JSON_RESPONSE_INSTRUCTION)
    );
}

#[test]
fn agent_fixtures_decode_each_backend() {
    let request = parse_request(&fixture("agent-request.json")).expect("parse");
    let task = agent::Task::from_operation(&request.operation).expect("agent");
    let agent::AgentBackend::Builtin { tools, max_steps } = task.backend else {
        panic!("expected builtin");
    };
    assert_eq!(
        tools.iter().map(|tool| tool.name()).collect::<Vec<_>>(),
        [
            "str_replace_based_edit_tool",
            "json_edit_tool",
            "glob",
            "sequentialthinking",
            "task_done"
        ]
    );
    assert!(!tools.contains(&agent::AgentTool::Bash));
    assert_eq!(*max_steps, Some(40));
    assert_eq!(task.response_format, Some(agent::ResponseFormat::Json));
    assert_eq!(task.input, Some(&json!({ "packages": ["web", "api"] })));
    for tool in [
        agent::AgentTool::Bash,
        agent::AgentTool::Edit,
        agent::AgentTool::JsonEdit,
        agent::AgentTool::Glob,
        agent::AgentTool::SequentialThinking,
        agent::AgentTool::TaskDone,
        agent::AgentTool::Ckg,
        agent::AgentTool::Mcp,
    ] {
        assert_eq!(
            serde_json::to_value(tool).expect("serializes"),
            json!(tool.name())
        );
    }

    let claude = parse_request(&fixture("agent-claude-code-request.json")).expect("parse");
    let task = agent::Task::from_operation(&claude.operation).expect("agent");
    assert_eq!(
        task.backend,
        &agent::AgentBackend::ClaudeCode {
            tools: vec![
                ClaudeCodeTool::Read,
                ClaudeCodeTool::Glob,
                ClaudeCodeTool::Grep
            ]
        }
    );
    assert!(!task.backend.is_builtin());
    for tool in [
        ClaudeCodeTool::Read,
        ClaudeCodeTool::Edit,
        ClaudeCodeTool::Write,
        ClaudeCodeTool::Glob,
        ClaudeCodeTool::Grep,
        ClaudeCodeTool::Bash,
    ] {
        assert_eq!(
            serde_json::to_value(tool).expect("serializes"),
            json!(tool.name())
        );
    }

    let codex = parse_request(&fixture("agent-codex-request.json")).expect("parse");
    let task = agent::Task::from_operation(&codex.operation).expect("agent");
    assert_eq!(
        task.backend,
        &agent::AgentBackend::Codex {
            sandbox: CodexSandbox::WorkspaceWrite
        }
    );
    for sandbox in [CodexSandbox::ReadOnly, CodexSandbox::WorkspaceWrite] {
        assert_eq!(
            serde_json::to_value(sandbox).expect("serializes"),
            json!(sandbox.name())
        );
    }
}

#[tokio::test]
async fn agent_config_failures_report_an_untouched_repository() {
    let request = parse_request(&fixture("agent-request.json")).expect("parse");
    let task = agent::Task::from_operation(&request.operation).expect("agent");
    let settings = || agent::settings_from_env(|_| Some("k".to_string()));
    let repeated = agent::AgentBackend::Builtin {
        tools: vec![agent::AgentTool::Glob, agent::AgentTool::Glob],
        max_steps: None,
    };
    let zero_steps = agent::AgentBackend::Builtin {
        tools: vec![],
        max_steps: Some(0),
    };
    let repeated_claude = agent::AgentBackend::ClaudeCode {
        tools: vec![ClaudeCodeTool::Read, ClaudeCodeTool::Read],
    };
    let cases = [
        (
            agent::run(
                &request.command_id,
                task,
                || agent::settings_from_env(|_| None),
                std::env::current_dir(),
            )
            .await,
            "agent requires LLM_API_KEY",
        ),
        (
            agent::run(
                &request.command_id,
                agent::Task {
                    backend: &repeated,
                    ..task
                },
                settings,
                std::env::current_dir(),
            )
            .await,
            "agent tools must not repeat",
        ),
        (
            agent::run(
                &request.command_id,
                agent::Task {
                    backend: &zero_steps,
                    ..task
                },
                settings,
                std::env::current_dir(),
            )
            .await,
            "agent maxSteps must be at least 1",
        ),
        (
            agent::run(
                &request.command_id,
                agent::Task {
                    backend: &repeated_claude,
                    ..task
                },
                || panic!("external backends never read LLM settings"),
                std::env::current_dir(),
            )
            .await,
            "claude-code tools must not repeat",
        ),
    ];
    for (completion, message) in cases {
        assert_eq!(completion.status, CompletionStatus::Failed);
        assert_eq!(completion.command_id, "fix-tests");
        let error = completion.error.expect("error");
        assert_eq!(error.message, message);
        assert_eq!(
            error.details,
            Some(json!({ "phase": "config", "repositoryMayBeModified": false }))
        );
    }
}
