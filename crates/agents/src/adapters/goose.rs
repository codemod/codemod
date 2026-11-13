//! Goose: `goose run --instructions - --output-format stream-json`, the prompt
//! on stdin, with `GOOSE_MODE` deciding what it may do.
//!
//! Events: `message` (content blocks `text`, `thinking`, `toolRequest`,
//! `toolResponse`), `notification`, `error` and a final `complete` with
//! token counts. Older versions print text only.

use std::path::Path;

use serde_json::Value;

use super::claude_like::text_of;
use crate::adapter::{
    json_line, str_at, Adapter, Autonomy, Launch, Parser, Task, TextParser, Transcript,
};
use crate::event::{AgentEvent, Stream, Usage};
use crate::probe::supports_flag;

pub struct Goose;

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

    fn session_env(&self) -> &'static [&'static str] {
        &["GOOSE_TERMINAL"]
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let mut launch = Launch::default()
            .args(["run", "--instructions", "-"])
            .option("--model", task.model)
            .option("--system", task.system_prompt)
            .env(
                "GOOSE_MODE",
                match task.autonomy {
                    Autonomy::Full => "auto",
                    Autonomy::ReadOnly => "chat",
                },
            );
        if supports_flag(executable, &["run", "--help"], "--output-format") {
            launch = launch.args(["--output-format", "stream-json"]);
        }
        launch.stdin(task.prompt)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(GooseParser::default())
    }
}

#[derive(Default)]
struct GooseParser {
    text: Option<TextParser>,
    /// The message the last text came from: more text from it continues it.
    last_message: Option<String>,
}

impl Parser for GooseParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        if let Some(text) = &mut self.text {
            return text.parse(line, transcript);
        }
        let Some(event) = json_line(line) else {
            if self.last_message.is_none() {
                let text = self.text.insert(TextParser::default());
                return text.parse(line, transcript);
            }
            return vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }];
        };
        match str_at(&event, "/type").unwrap_or_default() {
            "message" => self.message(event.get("message").unwrap_or(&Value::Null)),
            "error" => {
                let message = str_at(&event, "/error").unwrap_or("error").to_string();
                transcript.fail(message.clone());
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
        if str_at(message, "/role") != Some("assistant") && str_at(message, "/role") != Some("user")
        {
            return Vec::new();
        }
        let id = str_at(message, "/id").map(str::to_string);
        let blocks = message
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut events = Vec::new();
        for block in &blocks {
            match str_at(block, "/type").unwrap_or_default() {
                "text" if str_at(message, "/role") == Some("assistant") => {
                    let delta = id.is_some() && self.last_message == id;
                    self.last_message = id.clone().or(Some(String::new()));
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
                    self.last_message = Some(String::new());
                    let call = block.pointer("/toolCall/value").unwrap_or(&Value::Null);
                    events.push(AgentEvent::ToolCall {
                        id: str_at(block, "/id").map(str::to_string),
                        name: str_at(call, "/name").unwrap_or("tool").to_string(),
                        input: call.get("arguments").cloned().unwrap_or(Value::Null),
                    });
                }
                "toolResponse" => {
                    let result = block.get("toolResult").unwrap_or(&Value::Null);
                    let failed = str_at(result, "/status") == Some("error")
                        || result.pointer("/value/isError").and_then(Value::as_bool) == Some(true);
                    let output = match result.pointer("/value/content") {
                        Some(Value::Array(parts)) => parts
                            .iter()
                            .filter_map(|part| str_at(part, "/text"))
                            .collect::<Vec<_>>()
                            .join("\n"),
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
