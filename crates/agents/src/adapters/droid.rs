//! Factory Droid: `droid exec --output-format stream-json`, the prompt on
//! stdin.
//!
//! Factory documents the stream loosely and its own tools read it two ways,
//! so this reads both: `message`, `reasoning`, `tool_call` (`toolName` or
//! `toolId`), `tool_result`, `error`, and a final `result` or `completion`.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{Adapter, Autonomy, Launch, Parser, ReadOnly, Task};
use crate::event::{AgentEvent, Usage};
use crate::parse::{started, str_at, text_of, token_usage, tool_call};
use crate::transcript::Transcript;

pub(crate) struct Droid;

impl Adapter for Droid {
    fn id(&self) -> &'static str {
        "droid"
    }

    fn label(&self) -> &'static str {
        "Factory Droid"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["factory"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["droid"]
    }

    /// `droid exec` is read-only unless told otherwise.
    fn read_only(&self) -> ReadOnly {
        ReadOnly::AgentDefaults
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::new(DroidParser)
            .args(["exec", "--output-format", "stream-json"])
            .arg("--cwd")
            .arg(task.cwd)
            .option("--model", task.model)
            .option("--append-system-prompt", task.system_prompt);
        match task.autonomy {
            Autonomy::Full => launch.arg("--skip-permissions-unsafe"),
            Autonomy::ReadOnly => launch,
        }
        .stdin(task.prompt)
    }
}

struct DroidParser;

impl Parser for DroidParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let session = str_at(event, "/session_id");
        if let Some(session) = session {
            transcript.session(session);
        }
        match str_at(event, "/type").unwrap_or_default() {
            "system" if str_at(event, "/subtype") == Some("init") => {
                vec![started(transcript, session, str_at(event, "/model"))]
            }
            "message" if str_at(event, "/role") == Some("assistant") => str_at(event, "/text")
                .map(|text| AgentEvent::Text {
                    text: text.to_string(),
                    delta: false,
                })
                .into_iter()
                .collect(),
            "reasoning" => str_at(event, "/text")
                .map(|text| AgentEvent::Thinking {
                    text: text.to_string(),
                })
                .into_iter()
                .collect(),
            "tool_call" => vec![tool_call(
                str_at(event, "/id"),
                str_at(event, "/toolName")
                    .or_else(|| str_at(event, "/toolId"))
                    .unwrap_or("tool"),
                event
                    .get("parameters")
                    .or_else(|| event.get("input"))
                    .cloned()
                    .unwrap_or(Value::Null),
            )],
            "tool_result" => vec![AgentEvent::ToolResult {
                id: str_at(event, "/id").map(str::to_string),
                output: event.get("value").map(text_of).unwrap_or_default(),
                is_error: event
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }],
            "error" => {
                let message = str_at(event, "/message").unwrap_or("error").to_string();
                transcript.error(&message);
                vec![AgentEvent::Error { message }]
            }
            "result" | "completion" => {
                transcript.total_usage(Usage {
                    cost_usd: event.get("total_cost_usd").and_then(Value::as_f64),
                    ..token_usage(event.get("usage"))
                });
                let text = str_at(event, "/result")
                    .or_else(|| str_at(event, "/finalText"))
                    .unwrap_or_default();
                if event.get("is_error").and_then(Value::as_bool) == Some(true) {
                    transcript.failed(if text.is_empty() { "error" } else { text });
                } else {
                    transcript.succeeded(Some(text));
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
