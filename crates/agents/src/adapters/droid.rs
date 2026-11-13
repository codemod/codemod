//! Factory Droid: `droid exec --output-format stream-json`, the prompt on
//! stdin.
//!
//! Factory documents the stream loosely and its own tools read it two ways,
//! so this reads both: `message`, `reasoning`, `tool_call` (`toolName` or
//! `toolId`), `tool_result`, `error`, and a final `result` or `completion`.

use std::path::Path;

use serde_json::Value;

use super::claude_like::text_of;
use crate::adapter::{json_line, str_at, Adapter, Autonomy, Launch, Parser, Task, Transcript};
use crate::event::{AgentEvent, Stream, Usage};

pub struct Droid;

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

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::default()
            .args(["exec", "--output-format", "stream-json"])
            .arg("--cwd")
            .arg(task.cwd)
            .option("--model", task.model)
            .option("--append-system-prompt", task.system_prompt);
        // Droid's default is read-only.
        match task.autonomy {
            Autonomy::Full => launch.arg("--skip-permissions-unsafe"),
            Autonomy::ReadOnly => launch,
        }
        .stdin(task.prompt)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(DroidParser)
    }
}

struct DroidParser;

impl Parser for DroidParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let Some(event) = json_line(line) else {
            return vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }];
        };
        if let Some(session) = str_at(&event, "/session_id") {
            transcript.session(session);
        }
        match str_at(&event, "/type").unwrap_or_default() {
            "system" if str_at(&event, "/subtype") == Some("init") => {
                let model = str_at(&event, "/model").map(str::to_string);
                if let Some(model) = &model {
                    transcript.model(model.clone());
                }
                vec![AgentEvent::Started {
                    session_id: str_at(&event, "/session_id").map(str::to_string),
                    model,
                }]
            }
            "message" if str_at(&event, "/role") == Some("assistant") => str_at(&event, "/text")
                .map(|text| AgentEvent::Text {
                    text: text.to_string(),
                    delta: false,
                })
                .into_iter()
                .collect(),
            "reasoning" => str_at(&event, "/text")
                .map(|text| AgentEvent::Thinking {
                    text: text.to_string(),
                })
                .into_iter()
                .collect(),
            "tool_call" => vec![AgentEvent::ToolCall {
                id: str_at(&event, "/id").map(str::to_string),
                name: str_at(&event, "/toolName")
                    .or_else(|| str_at(&event, "/toolId"))
                    .unwrap_or("tool")
                    .to_string(),
                input: event
                    .get("parameters")
                    .or_else(|| event.get("input"))
                    .cloned()
                    .unwrap_or(Value::Null),
            }],
            "tool_result" => vec![AgentEvent::ToolResult {
                id: str_at(&event, "/id").map(str::to_string),
                output: event.get("value").map(text_of).unwrap_or_default(),
                is_error: event
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }],
            "error" => {
                let message = str_at(&event, "/message").unwrap_or("error").to_string();
                transcript.fail(message.clone());
                vec![AgentEvent::Error { message }]
            }
            "result" | "completion" => {
                transcript.total_usage(Usage {
                    input_tokens: event.pointer("/usage/input_tokens").and_then(Value::as_u64),
                    output_tokens: event
                        .pointer("/usage/output_tokens")
                        .and_then(Value::as_u64),
                    cost_usd: event.get("total_cost_usd").and_then(Value::as_f64),
                });
                let text = str_at(&event, "/result")
                    .or_else(|| str_at(&event, "/finalText"))
                    .unwrap_or_default();
                if event.get("is_error").and_then(Value::as_bool) == Some(true) {
                    transcript.fail(if text.is_empty() { "error" } else { text });
                } else if !text.is_empty() {
                    transcript.result(text);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
