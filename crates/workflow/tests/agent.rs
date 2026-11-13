//! `agent(...)` steps, with a stand-in coding agent on PATH. In a test
//! binary of its own: it changes PATH for the whole process.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

use codemod_workflow::events::Event;
use common::{output, Harness};
use serde_json::{json, Value};

const CODEX_OUTPUT: &str = r#"{"type":"thread.started","thread_id":"thread-1"}
{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"touch done","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"touch done","aggregated_output":"","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"All done."}}
{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":5}}"#;

#[test]
fn agent_steps_run_the_named_coding_agent() {
    // A `codex` that records its stdin and replays recorded output.
    let bin = tempfile::tempdir().unwrap();
    let stdin = bin.path().join("stdin");
    let output_file = bin.path().join("output.jsonl");
    std::fs::write(&output_file, CODEX_OUTPUT).unwrap();
    let codex = bin.path().join("codex");
    std::fs::write(
        &codex,
        format!(
            "#!/bin/sh\ncat > \"{}\"\ncat \"{}\"\n",
            stdin.display(),
            output_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{path}", bin.path().display()));

    let h = Harness::new();
    let report = h.start(
        r#"
import { step, agent } from "codemod/workflow";
export default async function main() {
  return step("Migrate", agent("Migrate the tests", { agent: "codex", systemPrompt: "Be careful." }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result["agent"], json!("codex"));
    assert_eq!(result["result"], json!("All done."));
    assert_eq!(result["sessionId"], json!("thread-1"));
    assert_eq!(
        result["usage"],
        json!({ "inputTokens": 100, "outputTokens": 5 })
    );
    let prompt = std::fs::read_to_string(&stdin).unwrap();
    assert!(
        prompt.contains("Be careful.") && prompt.contains("Migrate the tests"),
        "{prompt}"
    );

    let activity: Vec<String> = h
        .events
        .take()
        .into_iter()
        .filter_map(|event| match event {
            Event::StepAgent {
                agent, activity, ..
            } => Some(format!(
                "{agent}: {}",
                serde_json::to_value(activity).unwrap()["event"]
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        activity,
        [
            "codex: \"started\"",
            "codex: \"tool_call\"",
            "codex: \"tool_result\"",
            "codex: \"text\""
        ]
    );

    // An agent that is unknown, or not installed, fails the step.
    let report = h.start(
        r#"
import { step, agent } from "codemod/workflow";
export default async function main() { await step(agent("x", { agent: "nope" })); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("unknown coding agent 'nope'"), "{error}");
    assert!(error.contains("claude-code, codex"), "{error}");
}
