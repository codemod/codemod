//! Claude Code: `claude -p --output-format stream-json --verbose`, the prompt
//! on stdin.
//!
//! Events: `system/init` (session, model), `assistant` and `user` messages
//! with content blocks (`text`, `thinking`, `tool_use`, `tool_result`), and a
//! final `result` with the answer, usage and cost.
//!
//! Amp, Qwen Code and Cursor print the same format; their adapters use
//! [`ClaudeParser`] too.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{json_line, str_at, Adapter, Autonomy, Launch, Parser, Task, Transcript};
use crate::event::{AgentEvent, Stream, Usage};

pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["claude"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["claude"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &[
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SESSION_ID",
        ]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::default()
            .args(["--print", "--output-format", "stream-json", "--verbose"])
            .option("--model", task.model)
            .option("--append-system-prompt", task.system_prompt);
        match task.autonomy {
            Autonomy::Full => launch.arg("--dangerously-skip-permissions"),
            Autonomy::ReadOnly => launch.args(["--permission-mode", "plan"]),
        }
        .stdin(task.prompt)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(ClaudeParser)
    }
}

#[derive(Default)]
pub(crate) struct ClaudeParser;

impl Parser for ClaudeParser {
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
            "system" if matches!(str_at(&event, "/subtype"), Some("init" | "session_start")) => {
                let model = str_at(&event, "/model").map(str::to_string);
                if let Some(model) = &model {
                    transcript.model(model.clone());
                }
                vec![AgentEvent::Started {
                    session_id: str_at(&event, "/session_id").map(str::to_string),
                    model,
                }]
            }
            "assistant" => content(&event)
                .iter()
                .filter_map(|block| match str_at(block, "/type")? {
                    "text" => Some(AgentEvent::Text {
                        text: str_at(block, "/text")?.to_string(),
                        delta: false,
                    }),
                    "thinking" => Some(AgentEvent::Thinking {
                        text: str_at(block, "/thinking")?.to_string(),
                    }),
                    "tool_use" => Some(AgentEvent::ToolCall {
                        id: str_at(block, "/id").map(str::to_string),
                        name: str_at(block, "/name").unwrap_or("tool").to_string(),
                        input: block.get("input").cloned().unwrap_or(Value::Null),
                    }),
                    _ => None,
                })
                .collect(),
            "user" => content(&event)
                .iter()
                .filter(|block| str_at(block, "/type") == Some("tool_result"))
                .map(|block| AgentEvent::ToolResult {
                    id: str_at(block, "/tool_use_id").map(str::to_string),
                    output: tool_output(block.get("content")),
                    is_error: block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect(),
            "result" => {
                transcript.total_usage(Usage {
                    input_tokens: input_tokens(&event),
                    output_tokens: event
                        .pointer("/usage/output_tokens")
                        .and_then(Value::as_u64),
                    cost_usd: event.get("total_cost_usd").and_then(Value::as_f64),
                });
                let text = str_at(&event, "/result").unwrap_or_default();
                let failed = event.get("is_error").and_then(Value::as_bool) == Some(true)
                    || str_at(&event, "/subtype").is_some_and(|s| s != "success");
                if failed {
                    let message = str_at(&event, "/error/message")
                        .or_else(|| str_at(&event, "/error"))
                        .filter(|message| !message.is_empty())
                        .or(Some(text).filter(|text| !text.is_empty()))
                        .or_else(|| str_at(&event, "/subtype"))
                        .unwrap_or("error")
                        .to_string();
                    transcript.fail(message.clone());
                    return vec![AgentEvent::Error { message }];
                }
                transcript.result(text);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

fn content(event: &Value) -> &[Value] {
    event
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Input tokens, counting cached ones, which Claude reports apart.
fn input_tokens(event: &Value) -> Option<u64> {
    let usage = event.get("usage")?;
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    usage.get("input_tokens")?;
    Some(
        count("input_tokens")
            + count("cache_creation_input_tokens")
            + count("cache_read_input_tokens"),
    )
}

/// A tool result's content: a string, or text blocks.
fn tool_output(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| str_at(block, "/text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
