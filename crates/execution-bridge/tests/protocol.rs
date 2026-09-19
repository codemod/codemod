//! The wire contract shared with `packages/orchestration/src/core/protocol.ts`:
//! the JSON fixtures both sides check, strict decoding, and `shell` through
//! the real `DirectRunner`.

use std::path::Path;

use butterflow_execution_bridge::external::{ClaudeCodeTool, CodexSandbox};
use butterflow_execution_bridge::{
    agent, completion_from_result, execute, parse_request, ArtifactRef, AssessmentQuestion,
    CompletionStatus, Operation, OperationCompletion, RequestContext, SemanticAnalysis,
    SemanticMode, Target, PROTOCOL_VERSION,
};
use butterflow_models::Error;
use butterflow_runners::direct_runner::DirectRunner;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/orchestration/fixtures/protocol"
);

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(FIXTURES).join(name))
        .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"))
}

fn request(operation: &str) -> String {
    format!(r#"{{"protocolVersion":{PROTOCOL_VERSION},"commandId":"t","operation":{operation}}}"#)
}

fn jssg(extra: &str) -> String {
    format!(
        r#"{{"kind":"jssg","transform":{{"name":"migrate","hash":"{HASH}"}},"language":"typescript"{extra}}}"#
    )
}

#[test]
fn fixtures_round_trip_to_identical_json() {
    for name in [
        "shell-request.json",
        "jssg-request.json",
        "jssg-target-request.json",
        "agent-request.json",
        "agent-claude-code-request.json",
        "agent-codex-request.json",
        "assessment-request.json",
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
        ("assessment-completion.json", CompletionStatus::Succeeded),
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
        transform,
        language,
        include,
        semantic_analysis,
        selector,
        target,
        input,
        ..
    } = plain.operation
    else {
        panic!("expected jssg");
    };
    assert_eq!(
        transform,
        ArtifactRef {
            name: "migrate".to_string(),
            hash: "3a1b8c6d5e4f7a2b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b".to_string(),
        }
    );
    assert_eq!(language, "tsx");
    assert_eq!(include, Some(vec!["**/*.tsx".to_string()]));
    assert_eq!(
        semantic_analysis,
        Some(SemanticAnalysis::Mode(SemanticMode::Workspace))
    );
    let selector = selector.expect("selector");
    assert_eq!(selector.rule, json!({ "pattern": "createSignal($VALUE)" }));
    assert_eq!(selector.constraints, None);
    assert_eq!(target, None);
    assert_eq!(input, Some(json!({ "needsMigration": true })));

    let targeted = parse_request(&fixture("jssg-target-request.json")).expect("parse");
    let Operation::Jssg {
        selector, target, ..
    } = targeted.operation
    else {
        panic!("expected jssg");
    };
    assert_eq!(selector, None);
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
fn context_carries_root_files_and_artifact_and_omits_absent_fields() {
    let text = format!(
        r#"{{"protocolVersion":{PROTOCOL_VERSION},"commandId":"t","operation":{},"context":{{"targetRoot":"/r","files":[{{"path":"a.ts","content":"x"}}],"artifact":{{"source":"export default () => null;"}}}}}}"#,
        jssg(r#","semanticAnalysis":{"mode":"workspace"}"#)
    );
    let parsed = parse_request(&text).expect("parse");
    let context = parsed.context.clone().expect("context");
    assert_eq!(context.target_root.as_deref(), Some("/r"));
    assert_eq!(context.files.as_ref().map(Vec::len), Some(1));
    assert_eq!(
        context.artifact.map(|artifact| artifact.source),
        Some("export default () => null;".to_string())
    );
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
            request(r#"{"kind":"exec","command":"true"}"#),
            "unknown variant `exec`",
        ),
        (
            request(r#"{"kind":"shell","command":"true","target":{"root":"apps"}}"#),
            "unknown field `target`",
        ),
        (
            request(r#"{"kind":"ai","prompt":"x"}"#),
            "unknown variant `ai`",
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
        (
            request(r#"{"kind":"assessment","state":"s","questions":{},"target":{"root":"apps"}}"#),
            "unknown field `target`",
        ),
        (
            request(
                r#"{"kind":"assessment","state":"s","questions":{"q":{"type":"rank","instructions":"x"}}}"#,
            ),
            "unknown variant `rank`",
        ),
        (
            request(
                r#"{"kind":"assessment","state":"s","questions":{"q":{"type":"noul","instructions":"x","options":[]}}}"#,
            ),
            "unknown field `options`",
        ),
        (
            request(
                r#"{"kind":"assessment","state":"s","questions":{"q":{"type":"score","instructions":"x","criteria":{"a":null}}}}"#,
            ),
            "invalid request JSON",
        ),
        (
            request(r#"{"kind":"shell","command":"true","package":"p"}"#),
            "unknown field",
        ),
        (request(&jssg(r#","command":"true""#)), "unknown field"),
        // The path-based form is gone: a `script` field is unknown.
        (
            request(&jssg(r#","script":"p.ts""#)),
            "unknown field `script`",
        ),
        (
            request(r#"{"kind":"jssg","language":"typescript"}"#),
            "missing field `transform`",
        ),
        (
            request(
                r#"{"kind":"jssg","transform":{"name":"m","hash":"h","source":"x"},"language":"typescript"}"#,
            ),
            "unknown field `source`",
        ),
        (
            request(r#"{"kind":"jssg","transform":{"name":"m"},"language":"typescript"}"#),
            "missing field `hash`",
        ),
        (
            request(&jssg(r#","selector":{"pattern":"x"}"#)),
            "unknown field `pattern`",
        ),
        (
            request(&jssg(
                r#","selector":{"rule":{"pattern":"x"},"language":"tsx"}"#,
            )),
            "unknown field `language`",
        ),
        (
            request(&jssg(r#","target":"apps""#)),
            "invalid request JSON",
        ),
        (
            request(&jssg(r#","target":{"root":"a","files":[]}"#)),
            "unknown field `files`",
        ),
        (
            request(&jssg(
                r#","semanticAnalysis":{"mode":"workspace","threads":4}"#,
            )),
            "invalid request JSON",
        ),
        (
            request(r#"{"kind":"shell","command":"true"}"#)
                .replace(r#""commandId""#, r#""cwd":"/","commandId""#),
            "unknown field `cwd`",
        ),
        (
            request(r#"{"kind":"shell","command":"true"}"#).replace(
                r#""commandId":"t""#,
                r#""commandId":"t","context":{"scriptRoot":"/w"}"#,
            ),
            "unknown field `scriptRoot`",
        ),
        (
            request(r#"{"kind":"shell","command":"true"}"#).replace(
                r#""commandId":"t""#,
                r#""commandId":"t","context":{"artifact":{"source":"x","hash":"h"}}"#,
            ),
            "unknown field `hash`",
        ),
        (
            request(r#"{"kind":"shell","command":"true"}"#).replace(
                r#""commandId":"t""#,
                r#""commandId":"t","context":{"files":[{"path":"a","content":"","mode":1}]}"#,
            ),
            "unknown field `mode`",
        ),
        (
            fixture("shell-request.json").replace(
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

#[test]
fn assessment_fixture_decodes_every_question_type() {
    let parsed = parse_request(&fixture("assessment-request.json")).expect("parse");
    let Operation::Assessment {
        state,
        questions,
        model,
    } = parsed.operation
    else {
        panic!("expected assessment");
    };
    assert_eq!(state["package"], json!("web"));
    assert_eq!(model.as_deref(), Some("jev-latest"));
    assert!(matches!(
        &questions["risk"],
        AssessmentQuestion::Choice { criteria, .. } if criteria.len() == 3 && criteria["medium"].is_null()
    ));
    assert!(matches!(
        &questions["completeness"],
        AssessmentQuestion::Score { criteria, .. } if criteria.len() == 3
    ));
    assert!(matches!(
        &questions["touchesTests"],
        AssessmentQuestion::Noul {
            criteria: Some(_),
            ..
        }
    ));
}

#[tokio::test]
async fn assessment_is_refused_by_the_bridge() {
    let request = parse_request(&fixture("assessment-request.json")).expect("parse");
    let completion = execute(&DirectRunner::with_quiet(true), &request).await;
    assert_eq!(completion.status, CompletionStatus::Failed);
    assert!(completion
        .error
        .expect("error")
        .message
        .contains("executed by the TypeScript host"));
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
