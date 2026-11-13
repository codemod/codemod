//! Agents that print Claude Code's stream-json format: Amp, Qwen Code and
//! Cursor Agent.

use std::path::Path;

use serde_json::{json, Value};

use super::claude::ClaudeParser;
use crate::adapter::{
    json_line, str_at, with_instructions, Adapter, Autonomy, Launch, Parser, Task, Transcript,
};
use crate::event::AgentEvent;
use crate::probe::supports_flag;

/// Amp: `amp --execute --stream-json`, the prompt on stdin.
pub struct Amp;

impl Adapter for Amp {
    fn id(&self) -> &'static str {
        "amp"
    }

    fn label(&self) -> &'static str {
        "Amp"
    }

    fn executables(&self) -> &'static [&'static str] {
        &["amp"]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        // Amp picks models through modes (`--mode`), not model names.
        let launch = Launch::default().args(["--execute", "--stream-json"]);
        match task.autonomy {
            Autonomy::Full => launch.arg("--dangerously-allow-all"),
            Autonomy::ReadOnly => launch,
        }
        .stdin(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(ClaudeParser)
    }
}

/// Qwen Code: `qwen --output-format stream-json --prompt <prompt>`.
pub struct QwenCode;

impl Adapter for QwenCode {
    fn id(&self) -> &'static str {
        "qwen-code"
    }

    fn label(&self) -> &'static str {
        "Qwen Code"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["qwen"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["qwen"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &["QWEN_CODE"]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        Launch::default()
            .args(["--output-format", "stream-json"])
            .option("--model", task.model)
            .option("--append-system-prompt", task.system_prompt)
            .args(match task.autonomy {
                Autonomy::Full => ["--approval-mode", "yolo"],
                Autonomy::ReadOnly => ["--approval-mode", "plan"],
            })
            .arg("--prompt")
            .arg(task.prompt)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(ClaudeParser)
    }
}

/// Cursor Agent: `cursor-agent --print --output-format stream-json <prompt>`.
///
/// Like Claude Code, plus `tool_call` events (`started`, `completed`) that
/// carry the tool as `{ readToolCall: { args } }` and the like.
pub struct Cursor;

impl Adapter for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn label(&self) -> &'static str {
        "Cursor Agent"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["cursor-agent"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["cursor-agent"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &["CURSOR_AGENT"]
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let launch = Launch::default()
            .args(["--print", "--output-format", "stream-json"])
            .arg("--workspace")
            .arg(task.cwd)
            .option("--model", task.model);
        // Without trust, print mode stops at the workspace trust question.
        let launch = if supports_flag(executable, &["--help"], "--trust") {
            launch.arg("--trust")
        } else {
            launch
        };
        match task.autonomy {
            Autonomy::Full => launch.arg("--force"),
            Autonomy::ReadOnly => launch.args(["--mode", "ask"]),
        }
        .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(CursorParser)
    }
}

struct CursorParser;

impl Parser for CursorParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let Some(event) = json_line(line).filter(|e| str_at(e, "/type") == Some("tool_call"))
        else {
            return ClaudeParser.parse(line, transcript);
        };
        let id = str_at(&event, "/call_id").map(str::to_string);
        let Some((kind, call)) = event
            .get("tool_call")
            .and_then(Value::as_object)
            .and_then(|call| call.iter().next())
        else {
            return Vec::new();
        };
        match str_at(&event, "/subtype") {
            Some("started") => {
                let (name, input) = if kind == "function" {
                    (
                        str_at(call, "/name").unwrap_or("tool").to_string(),
                        arguments(call.get("arguments")),
                    )
                } else {
                    (
                        kind.trim_end_matches("ToolCall").to_string(),
                        call.get("args").cloned().unwrap_or(Value::Null),
                    )
                };
                vec![AgentEvent::ToolCall { id, name, input }]
            }
            Some("completed") => {
                let result = call.get("result").cloned().unwrap_or(Value::Null);
                let error = result.get("error").filter(|e| !e.is_null()).cloned();
                let output = error
                    .as_ref()
                    .or_else(|| result.get("success"))
                    .map(text_of)
                    .unwrap_or_default();
                vec![AgentEvent::ToolResult {
                    id,
                    output,
                    is_error: error.is_some(),
                }]
            }
            _ => Vec::new(),
        }
    }
}

/// Tool arguments, which some agents send as a JSON string.
pub(crate) fn arguments(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// A tool's output as text: a string, its `content` or `message`, or JSON.
pub(crate) fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => str_at(other, "/content")
            .or_else(|| str_at(other, "/message"))
            .map(str::to_string)
            .unwrap_or_else(|| other.to_string()),
    }
}
