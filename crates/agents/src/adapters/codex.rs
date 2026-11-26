//! Codex CLI: `codex exec --json -`, the prompt on stdin.
//!
//! Events: `thread.started` (session), `item.started` / `item.completed`
//! for items (`agent_message`, `reasoning`, `command_execution`,
//! `file_change`, `mcp_tool_call`, `web_search`), `turn.completed` (usage),
//! `turn.failed` and `error`. The answer is the last agent message.

use std::path::Path;

use serde_json::{json, Value};

use crate::adapter::{Adapter, Autonomy, Launch, Parser, ReadOnly, Task};
use crate::event::{AgentEvent, FileChange, FileChangeKind};
use crate::parse::{started, str_at, token_usage, with_instructions};
use crate::transcript::Transcript;

pub(crate) struct Codex;

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

    fn env_markers(&self) -> &'static [&'static str] {
        &[
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "CODEX_SANDBOX",
            "CODEX_CI",
        ]
    }

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Enforced
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::new(CodexParser::default())
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
}

#[derive(Default)]
struct CodexParser {
    /// Items whose start was reported, so their completion only adds the result.
    started: std::collections::HashSet<String>,
}

impl Parser for CodexParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        match str_at(event, "/type").unwrap_or_default() {
            "thread.started" => vec![started(transcript, str_at(event, "/thread_id"), None)],
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
                transcript.turn_usage(token_usage(event.get("usage")));
                Vec::new()
            }
            "turn.failed" => {
                let message = str_at(event, "/error/message")
                    .unwrap_or("the turn failed")
                    .to_string();
                transcript.failed(&message);
                vec![AgentEvent::Error { message }]
            }
            // Stream errors Codex may retry; a failure ends the turn above.
            "error" => vec![AgentEvent::Warning {
                message: str_at(event, "/message").unwrap_or("error").to_string(),
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
    let mut changes = Vec::new();
    let (name, input) = match str_at(item, "/type")? {
        "command_execution" => (
            "shell".to_string(),
            json!({ "command": item.get("command") }),
        ),
        "file_change" => {
            changes = file_changes(item);
            (
                "edit".to_string(),
                json!({ "changes": item.get("changes") }),
            )
        }
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
    Some(AgentEvent::ToolCall {
        id,
        name,
        input,
        changes,
    })
}

/// The files a `file_change` item names. Codex says which files and
/// whether each is added, updated or deleted, not what changed in them.
fn file_changes(item: &Value) -> Vec<FileChange> {
    item.get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            Some(FileChange {
                path: str_at(change, "/path")?.to_string(),
                kind: match str_at(change, "/kind") {
                    Some("add") => FileChangeKind::Create,
                    Some("delete") => FileChangeKind::Delete,
                    _ => FileChangeKind::Update,
                },
                old: None,
                new: None,
                diff: None,
            })
        })
        .collect()
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
