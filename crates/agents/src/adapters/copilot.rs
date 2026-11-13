//! GitHub Copilot CLI: `copilot --prompt=<prompt> --output-format json`.
//!
//! JSONL events `{ type, data }`: `assistant.message` (and `_delta`),
//! `tool.execution_start`, `tool.execution_complete`, `assistant.usage`,
//! `session.error`, `session.warning`, and a final `result` with the exit
//! code.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{Adapter, Autonomy, Launch, Parser, ReadOnly, Task};
use crate::event::{AgentEvent, Usage};
use crate::parse::{arguments, str_at, text_of, tool_call, with_instructions};
use crate::probe::help_text;
use crate::transcript::Transcript;

pub(crate) struct Copilot;

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

    fn env_markers(&self) -> &'static [&'static str] {
        &["COPILOT_CLI"]
    }

    /// Without `--allow-all-tools` Copilot asks before a tool writes or
    /// runs a command.
    fn read_only(&self) -> ReadOnly {
        ReadOnly::AgentDefaults
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let help = help_text(executable, &["--help"]);
        let mut launch = Launch::new(CopilotParser)
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
        launch.joined(
            "--prompt",
            &with_instructions(task.prompt, task.system_prompt),
        )
    }
}

struct CopilotParser;

impl Parser for CopilotParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let data = event.get("data").unwrap_or(&Value::Null);
        if let Some(session) = str_at(data, "/sessionId") {
            transcript.session(session);
        }
        match str_at(event, "/type").unwrap_or_default() {
            // Deltas repeat what the full message says; the full message is enough.
            "assistant.message" => str_at(data, "/content")
                .filter(|text| !text.trim().is_empty())
                .map(|text| AgentEvent::Text {
                    text: text.to_string(),
                    delta: false,
                })
                .into_iter()
                .collect(),
            "assistant.reasoning" => str_at(data, "/content")
                .map(|text| AgentEvent::Thinking {
                    text: text.to_string(),
                })
                .into_iter()
                .collect(),
            "tool.execution_start" => vec![tool_call(
                str_at(data, "/toolCallId"),
                str_at(data, "/toolName").unwrap_or("tool"),
                arguments(data.get("arguments")),
            )],
            "tool.execution_complete" => {
                let failed = data.get("success").and_then(Value::as_bool) == Some(false);
                let output = if failed {
                    str_at(data, "/error/message")
                        .unwrap_or("the tool failed")
                        .to_string()
                } else {
                    data.pointer("/result/content")
                        .map(text_of)
                        .unwrap_or_default()
                };
                vec![AgentEvent::ToolResult {
                    id: str_at(data, "/toolCallId").map(str::to_string),
                    output,
                    is_error: failed,
                }]
            }
            "assistant.usage" => {
                if let Some(model) = str_at(data, "/model") {
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
                message: str_at(data, "/message").unwrap_or("warning").to_string(),
            }],
            "session.error" => {
                let message = str_at(data, "/message").unwrap_or("error").to_string();
                transcript.error(&message);
                vec![AgentEvent::Error { message }]
            }
            "result" => {
                match event.get("exitCode").and_then(Value::as_i64).unwrap_or(0) {
                    0 => transcript.succeeded(None),
                    code => transcript.failed(&format!("exited with code {code}")),
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
