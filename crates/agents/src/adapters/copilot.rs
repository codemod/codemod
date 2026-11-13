//! GitHub Copilot CLI: `copilot --prompt <prompt> --output-format json`.
//!
//! JSONL events `{ type, data }`: `assistant.message` (and `_delta`),
//! `tool.execution_start`, `tool.execution_complete`, `assistant.usage`,
//! `session.error`, `session.warning`, and a final `result` with the exit
//! code.

use std::path::Path;

use serde_json::Value;

use super::claude_like::{arguments, text_of};
use crate::adapter::{
    json_line, str_at, with_instructions, Adapter, Autonomy, Launch, Parser, Task, Transcript,
};
use crate::event::{AgentEvent, Stream, Usage};
use crate::probe::help_text;

pub struct Copilot;

impl Adapter for Copilot {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn label(&self) -> &'static str {
        "GitHub Copilot CLI"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["copilot-cli", "github-copilot"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["copilot"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &["COPILOT_CLI"]
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let help = help_text(executable, &["--help"]);
        let mut launch = Launch::default()
            .args(["--output-format", "json"])
            .option("--model", task.model);
        if help.contains("--no-ask-user") {
            launch = launch.arg("--no-ask-user");
        }
        if task.autonomy == Autonomy::Full {
            launch = if help.contains("--allow-all-paths") {
                launch.args(["--allow-all-tools", "--allow-all-paths"])
            } else {
                launch.arg("--allow-all-tools")
            };
        }
        // Copilot ignores stdin when given a prompt, so the prompt is the argument.
        launch
            .arg("--prompt")
            .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(CopilotParser)
    }
}

struct CopilotParser;

impl Parser for CopilotParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let Some(event) = json_line(line) else {
            return vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }];
        };
        let data = event.get("data").cloned().unwrap_or(Value::Null);
        if let Some(session) = str_at(&data, "/sessionId") {
            transcript.session(session);
        }
        match str_at(&event, "/type").unwrap_or_default() {
            // Deltas repeat what the full message says; the full message is enough.
            "assistant.message" => str_at(&data, "/content")
                .filter(|text| !text.trim().is_empty())
                .map(|text| AgentEvent::Text {
                    text: text.to_string(),
                    delta: false,
                })
                .into_iter()
                .collect(),
            "assistant.reasoning" => str_at(&data, "/content")
                .map(|text| AgentEvent::Thinking {
                    text: text.to_string(),
                })
                .into_iter()
                .collect(),
            "tool.execution_start" => vec![AgentEvent::ToolCall {
                id: str_at(&data, "/toolCallId").map(str::to_string),
                name: str_at(&data, "/toolName").unwrap_or("tool").to_string(),
                input: arguments(data.get("arguments")),
            }],
            "tool.execution_complete" => {
                let failed = data.get("success").and_then(Value::as_bool) == Some(false);
                let output = if failed {
                    str_at(&data, "/error/message")
                        .unwrap_or("the tool failed")
                        .to_string()
                } else {
                    data.pointer("/result/content")
                        .map(text_of)
                        .unwrap_or_default()
                };
                vec![AgentEvent::ToolResult {
                    id: str_at(&data, "/toolCallId").map(str::to_string),
                    output,
                    is_error: failed,
                }]
            }
            "assistant.usage" => {
                if let Some(model) = str_at(&data, "/model") {
                    transcript.model(model);
                }
                transcript.turn_usage(Usage {
                    input_tokens: data.get("inputTokens").and_then(Value::as_u64),
                    output_tokens: data.get("outputTokens").and_then(Value::as_u64),
                    cost_usd: data.get("cost").and_then(Value::as_f64),
                });
                Vec::new()
            }
            "session.warning" => vec![AgentEvent::Warning {
                message: str_at(&data, "/message").unwrap_or("warning").to_string(),
            }],
            "session.error" => {
                let message = str_at(&data, "/message").unwrap_or("error").to_string();
                transcript.fail(message.clone());
                vec![AgentEvent::Error { message }]
            }
            "result" => {
                if event.get("exitCode").and_then(Value::as_i64).unwrap_or(0) != 0 {
                    transcript.fail(format!(
                        "exited with code {}",
                        event["exitCode"].as_i64().unwrap_or(1)
                    ));
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
