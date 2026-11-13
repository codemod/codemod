//! Codex CLI: `codex exec --json -`, the prompt on stdin.
//!
//! Events: `thread.started` (session), `item.started` / `item.completed`
//! for items (`agent_message`, `reasoning`, `command_execution`,
//! `file_change`, `mcp_tool_call`, `web_search`), `turn.completed` (usage),
//! `turn.failed` and `error`. The answer is the last agent message.

use std::path::Path;

use serde_json::{json, Value};

use crate::adapter::{
    json_line, str_at, with_instructions, Adapter, Autonomy, Launch, Parser, Task, Transcript,
};
use crate::event::{AgentEvent, Stream, Usage};

pub struct Codex;

impl Adapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["codex-cli"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["codex"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &[
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "CODEX_SANDBOX",
            "CODEX_CI",
        ]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::default()
            .args([
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--color",
                "never",
            ])
            .arg("--cd")
            .arg(task.cwd)
            .option("--model", task.model);
        match task.autonomy {
            Autonomy::Full => launch.arg("--dangerously-bypass-approvals-and-sandbox"),
            Autonomy::ReadOnly => launch.args(["--sandbox", "read-only"]),
        }
        .arg("-")
        .env("RUST_LOG", "error")
        .stdin(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(CodexParser::default())
    }
}

#[derive(Default)]
pub(crate) struct CodexParser {
    /// Items whose start was reported, so their completion only adds the result.
    started: std::collections::HashSet<String>,
}

impl Parser for CodexParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let Some(event) = json_line(line) else {
            return vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }];
        };
        match str_at(&event, "/type").unwrap_or_default() {
            "thread.started" => {
                let session = str_at(&event, "/thread_id").map(str::to_string);
                if let Some(session) = &session {
                    transcript.session(session.clone());
                }
                vec![AgentEvent::Started {
                    session_id: session,
                    model: None,
                }]
            }
            "item.started" => {
                let Some(item) = event.get("item") else {
                    return Vec::new();
                };
                match call(item) {
                    Some(call) => {
                        if let Some(id) = str_at(item, "/id") {
                            self.started.insert(id.to_string());
                        }
                        vec![call]
                    }
                    None => Vec::new(),
                }
            }
            "item.completed" => {
                let Some(item) = event.get("item") else {
                    return Vec::new();
                };
                self.completed(item)
            }
            "turn.completed" => {
                transcript.turn_usage(Usage {
                    input_tokens: event.pointer("/usage/input_tokens").and_then(Value::as_u64),
                    output_tokens: event
                        .pointer("/usage/output_tokens")
                        .and_then(Value::as_u64),
                    cost_usd: None,
                });
                Vec::new()
            }
            "turn.failed" => {
                let message = str_at(&event, "/error/message")
                    .unwrap_or("the turn failed")
                    .to_string();
                transcript.fail(message.clone());
                vec![AgentEvent::Error { message }]
            }
            // Stream errors Codex may retry; a failure ends the turn above.
            "error" => vec![AgentEvent::Warning {
                message: str_at(&event, "/message").unwrap_or("error").to_string(),
            }],
            _ => Vec::new(),
        }
    }
}

impl CodexParser {
    fn completed(&mut self, item: &Value) -> Vec<AgentEvent> {
        let id = str_at(item, "/id").map(str::to_string);
        match str_at(item, "/type").unwrap_or_default() {
            "agent_message" => vec![AgentEvent::Text {
                text: str_at(item, "/text").unwrap_or_default().to_string(),
                delta: false,
            }],
            "reasoning" => vec![AgentEvent::Thinking {
                text: str_at(item, "/text").unwrap_or_default().to_string(),
            }],
            "error" => vec![AgentEvent::Warning {
                message: str_at(item, "/message").unwrap_or("error").to_string(),
            }],
            "command_execution" | "mcp_tool_call" | "web_search" | "file_change" => {
                let mut events = Vec::new();
                let started = id.as_ref().is_some_and(|id| self.started.remove(id));
                if !started {
                    events.extend(call(item));
                }
                events.extend(result(item, id));
                events
            }
            _ => Vec::new(),
        }
    }
}

/// The tool call an item stands for.
fn call(item: &Value) -> Option<AgentEvent> {
    let id = str_at(item, "/id").map(str::to_string);
    let (name, input) = match str_at(item, "/type")? {
        "command_execution" => (
            "shell".to_string(),
            json!({ "command": item.get("command") }),
        ),
        "file_change" => (
            "edit".to_string(),
            json!({ "changes": item.get("changes") }),
        ),
        "web_search" => (
            "web_search".to_string(),
            json!({ "query": item.get("query") }),
        ),
        "mcp_tool_call" => (
            format!(
                "{}.{}",
                str_at(item, "/server").unwrap_or("mcp"),
                str_at(item, "/tool").unwrap_or("tool")
            ),
            item.get("arguments").cloned().unwrap_or(Value::Null),
        ),
        _ => return None,
    };
    Some(AgentEvent::ToolCall { id, name, input })
}

/// What a finished item returned.
fn result(item: &Value, id: Option<String>) -> Option<AgentEvent> {
    let failed = str_at(item, "/status") == Some("failed");
    match str_at(item, "/type")? {
        "command_execution" => Some(AgentEvent::ToolResult {
            id,
            output: str_at(item, "/aggregated_output")
                .unwrap_or_default()
                .to_string(),
            is_error: failed || item.get("exit_code").and_then(Value::as_i64).unwrap_or(0) != 0,
        }),
        "mcp_tool_call" => Some(AgentEvent::ToolResult {
            id,
            output: item
                .pointer("/error/message")
                .or_else(|| item.get("result"))
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default(),
            is_error: failed || item.get("error").is_some_and(|e| !e.is_null()),
        }),
        "file_change" if failed => Some(AgentEvent::ToolResult {
            id,
            output: "the change was not applied".to_string(),
            is_error: true,
        }),
        _ => None,
    }
}
