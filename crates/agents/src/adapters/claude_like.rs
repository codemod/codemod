//! Agents that print Claude Code's stream-json format: Amp, Qwen Code and
//! Cursor Agent.

use std::path::Path;

use serde_json::Value;

use super::claude::ClaudeParser;
use crate::adapter::{Adapter, Autonomy, Launch, Parser, ReadOnly, Task};
use crate::event::AgentEvent;
use crate::parse::{arguments, str_at, text_of, tool_call, with_instructions};
use crate::probe::supports_flag;
use crate::transcript::Transcript;

/// Amp: `amp --execute --stream-json`, the prompt on stdin.
///
/// Amp's own permissions ask before commands but not before file edits, and
/// it has no read-only mode, so it cannot be kept read-only.
pub(crate) struct Amp;

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

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Unsupported
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        // Amp picks models through modes (`--mode`), not model names.
        let launch = Launch::new(ClaudeParser).args(["--execute", "--stream-json"]);
        match task.autonomy {
            Autonomy::Full => launch.arg("--dangerously-allow-all"),
            Autonomy::ReadOnly => launch,
        }
        .stdin(with_instructions(task.prompt, task.system_prompt))
    }
}

/// Qwen Code: `qwen --output-format stream-json --prompt=<prompt>`.
pub(crate) struct QwenCode;

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

    fn env_markers(&self) -> &'static [&'static str] {
        &["QWEN_CODE"]
    }

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Enforced
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        Launch::new(ClaudeParser)
            .args(["--output-format", "stream-json"])
            .option("--model", task.model)
            .option("--append-system-prompt", task.system_prompt)
            .args(match task.autonomy {
                Autonomy::Full => ["--approval-mode", "yolo"],
                Autonomy::ReadOnly => ["--approval-mode", "plan"],
            })
            .joined("--prompt", task.prompt)
    }
}

/// Cursor Agent: `cursor-agent --print --output-format stream-json -- <prompt>`.
///
/// Like Claude Code, plus `tool_call` events (`started`, `completed`) that
/// carry the tool as `{ readToolCall: { args } }` and the like.
pub(crate) struct Cursor;

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

    fn env_markers(&self) -> &'static [&'static str] {
        &["CURSOR_AGENT"]
    }

    fn editor_env_markers(&self) -> &'static [&'static str] {
        &["CURSOR_TRACE_ID"]
    }

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Enforced
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let launch = Launch::new(CursorParser)
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
        .positional(with_instructions(task.prompt, task.system_prompt))
    }
}

struct CursorParser;

impl Parser for CursorParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        if str_at(event, "/type") != Some("tool_call") {
            return ClaudeParser.event(event, transcript);
        }
        let id = str_at(event, "/call_id");
        let Some((kind, call)) = event
            .get("tool_call")
            .and_then(Value::as_object)
            .and_then(|call| call.iter().next())
        else {
            return Vec::new();
        };
        match str_at(event, "/subtype") {
            Some("started") if kind == "function" => vec![tool_call(
                id,
                str_at(call, "/name").unwrap_or("tool"),
                arguments(call.get("arguments")),
            )],
            Some("started") => vec![tool_call(
                id,
                kind.trim_end_matches("ToolCall"),
                call.get("args").cloned().unwrap_or(Value::Null),
            )],
            Some("completed") => {
                let result = call.get("result").cloned().unwrap_or(Value::Null);
                let error = result.get("error").filter(|e| !e.is_null()).cloned();
                let output = error
                    .as_ref()
                    .or_else(|| result.get("success"))
                    .map(text_of)
                    .unwrap_or_default();
                vec![AgentEvent::ToolResult {
                    id: id.map(str::to_string),
                    output,
                    is_error: error.is_some(),
                }]
            }
            _ => Vec::new(),
        }
    }
}
