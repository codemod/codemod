//! Kimi Code: `kimi --prompt=<prompt> --output-format stream-json`.
//!
//! Prompt mode always runs in Kimi's auto permission mode and refuses the
//! other mode flags, so it cannot be kept read-only. Lines are chat
//! messages: `{ role: "assistant", content, tool_calls }`,
//! `{ role: "tool", ... }` and `{ role: "meta", type }`.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{Adapter, Launch, Parser, ReadOnly, Task};
use crate::event::AgentEvent;
use crate::parse::{arguments, str_at, tool_call, with_instructions};
use crate::transcript::Transcript;

pub(crate) struct Kimi;

impl Adapter for Kimi {
    fn id(&self) -> &'static str {
        "kimi"
    }

    fn label(&self) -> &'static str {
        "Kimi Code"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["kimi-code"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["kimi"]
    }

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Unsupported
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        Launch::new(KimiParser)
            .args(["--output-format", "stream-json"])
            .option("--model", task.model)
            .joined(
                "--prompt",
                &with_instructions(task.prompt, task.system_prompt),
            )
    }
}

struct KimiParser;

impl Parser for KimiParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        match str_at(event, "/role").unwrap_or_default() {
            "assistant" => {
                let mut events = Vec::new();
                if let Some(text) = str_at(event, "/content").filter(|t| !t.trim().is_empty()) {
                    events.push(AgentEvent::Text {
                        text: text.to_string(),
                        delta: false,
                    });
                }
                for call in event
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    events.push(tool_call(
                        str_at(call, "/id"),
                        str_at(call, "/function/name").unwrap_or("tool"),
                        arguments(call.pointer("/function/arguments")),
                    ));
                }
                events
            }
            "tool" => vec![AgentEvent::ToolResult {
                id: str_at(event, "/tool_call_id").map(str::to_string),
                output: str_at(event, "/content").unwrap_or_default().to_string(),
                is_error: false,
            }],
            "meta" => match str_at(event, "/type").unwrap_or_default() {
                "session.resume_hint" => {
                    if let Some(session) = str_at(event, "/session_id") {
                        transcript.session(session);
                    }
                    Vec::new()
                }
                "turn.step.retrying" => vec![AgentEvent::Warning {
                    message: format!(
                        "retrying: {}",
                        str_at(event, "/error_message").unwrap_or("the request failed")
                    ),
                }],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}
