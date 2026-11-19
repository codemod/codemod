//! Goose: `goose run --instructions - --output-format stream-json`, the prompt
//! on stdin, with `GOOSE_MODE` deciding what it may do.
//!
//! Events: `message` (content blocks `text`, `thinking`, `toolRequest`,
//! `toolResponse`), `notification`, `error` and a final `complete` with
//! token counts. Older versions print text only.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{Adapter, Autonomy, Launch, Parser, ReadOnly, Task};
use crate::event::{AgentEvent, Usage};
use crate::parse::{str_at, text_of, tool_call, TextParser};
use crate::probe::supports_flag;
use crate::transcript::Transcript;

pub(crate) struct Goose;

impl Adapter for Goose {
    fn id(&self) -> &'static str {
        "goose"
    }

    fn label(&self) -> &'static str {
        "Goose"
    }

    fn executables(&self) -> &'static [&'static str] {
        &["goose"]
    }

    fn env_markers(&self) -> &'static [&'static str] {
        &["GOOSE_TERMINAL"]
    }

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Enforced
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let launch = if supports_flag(executable, &["run", "--help"], "--output-format") {
            Launch::new(GooseParser::default()).args(["run", "--output-format", "stream-json"])
        } else {
            Launch::new(TextParser).arg("run")
        };
        launch
            .args(["--instructions", "-"])
            .option("--model", task.model)
            .option("--system", task.system_prompt)
            .env(
                "GOOSE_MODE",
                match task.autonomy {
                    Autonomy::Full => "auto",
                    Autonomy::ReadOnly => "chat",
                },
            )
            .stdin(task.prompt)
    }
}

#[derive(Default)]
struct GooseParser {
    /// The message the last text came from, until a tool call follows it:
    /// more text from that message continues it.
    text_message: Option<String>,
}

impl Parser for GooseParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        match str_at(event, "/type").unwrap_or_default() {
            "message" => self.message(event.get("message").unwrap_or(&Value::Null)),
            "error" => {
                let message = str_at(event, "/error").unwrap_or("error").to_string();
                transcript.error(&message);
                vec![AgentEvent::Error { message }]
            }
            "complete" => {
                transcript.total_usage(Usage {
                    input_tokens: event.get("input_tokens").and_then(Value::as_u64),
                    output_tokens: event.get("output_tokens").and_then(Value::as_u64),
                    cost_usd: event.get("cost_usd").and_then(Value::as_f64),
                });
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

impl GooseParser {
    fn message(&mut self, message: &Value) -> Vec<AgentEvent> {
        let role = str_at(message, "/role");
        if role != Some("assistant") && role != Some("user") {
            return Vec::new();
        }
        let id = str_at(message, "/id");
        let blocks = message
            .get("content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut events = Vec::new();
        for block in blocks {
            match str_at(block, "/type").unwrap_or_default() {
                "text" if role == Some("assistant") => {
                    let delta = id.is_some() && self.text_message.as_deref() == id;
                    self.text_message = id.map(str::to_string);
                    events.push(AgentEvent::Text {
                        text: str_at(block, "/text").unwrap_or_default().to_string(),
                        delta,
                    });
                }
                "thinking" => events.push(AgentEvent::Thinking {
                    text: str_at(block, "/thinking")
                        .or_else(|| str_at(block, "/text"))
                        .unwrap_or_default()
                        .to_string(),
                }),
                "toolRequest" => {
                    self.text_message = None;
                    let call = block.pointer("/toolCall/value").unwrap_or(&Value::Null);
                    events.push(tool_call(
                        str_at(block, "/id"),
                        str_at(call, "/name").unwrap_or("tool"),
                        call.get("arguments").cloned().unwrap_or(Value::Null),
                    ));
                }
                "toolResponse" => {
                    let result = block.get("toolResult").unwrap_or(&Value::Null);
                    let failed = str_at(result, "/status") == Some("error")
                        || result.pointer("/value/isError").and_then(Value::as_bool) == Some(true);
                    let output = match result.pointer("/value/content") {
                        Some(content @ Value::Array(_)) => text_of(content),
                        _ => result.get("error").map(text_of).unwrap_or_default(),
                    };
                    events.push(AgentEvent::ToolResult {
                        id: str_at(block, "/id").map(str::to_string),
                        output,
                        is_error: failed,
                    });
                }
                _ => {}
            }
        }
        events
    }
}
