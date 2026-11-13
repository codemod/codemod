//! Google's agents: Gemini CLI and its successor, Antigravity CLI.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{Adapter, Autonomy, Launch, Parser, ReadOnly, Task};
use crate::event::AgentEvent;
use crate::parse::{
    not_a_flag, started, str_at, text_of, token_usage, tool_call, with_instructions, TextParser,
};
use crate::probe::{help_text, supports_flag};
use crate::transcript::Transcript;

/// Gemini CLI: `gemini --output-format stream-json --prompt=<prompt>`.
///
/// Older versions have no structured output; their text is read as is.
/// Events: `init`, `message` (with `delta`), `tool_use`, `tool_result`,
/// `error` and a final `result` with stats.
pub(crate) struct GeminiCli;

impl Adapter for GeminiCli {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn label(&self) -> &'static str {
        "Gemini CLI"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["gemini-cli"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["gemini"]
    }

    fn env_markers(&self) -> &'static [&'static str] {
        &["GEMINI_CLI"]
    }

    /// Without an approval mode, a prompt run leaves out the tools that
    /// need approval: writing files and running commands.
    fn read_only(&self) -> ReadOnly {
        ReadOnly::AgentDefaults
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let help = help_text(executable, &["--help"]);
        let mut launch = if help.contains("stream-json") {
            Launch::new(GeminiParser).args(["--output-format", "stream-json"])
        } else {
            Launch::new(TextParser)
        }
        .option("--model", task.model);
        if help.contains("--skip-trust") {
            launch = launch.arg("--skip-trust");
        }
        if task.autonomy == Autonomy::Full {
            launch = if help.contains("--approval-mode") {
                launch.args(["--approval-mode", "yolo"])
            } else {
                launch.arg("--yolo")
            };
        }
        launch.joined(
            "--prompt",
            &with_instructions(task.prompt, task.system_prompt),
        )
    }
}

struct GeminiParser;

impl Parser for GeminiParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        match str_at(event, "/type").unwrap_or_default() {
            "init" => vec![started(
                transcript,
                str_at(event, "/session_id"),
                str_at(event, "/model"),
            )],
            "message" if str_at(event, "/role") == Some("assistant") => {
                vec![AgentEvent::Text {
                    text: str_at(event, "/content").unwrap_or_default().to_string(),
                    delta: event.get("delta").and_then(Value::as_bool).unwrap_or(false),
                }]
            }
            "tool_use" => vec![tool_call(
                str_at(event, "/tool_id"),
                str_at(event, "/tool_name").unwrap_or("tool"),
                event.get("parameters").cloned().unwrap_or(Value::Null),
            )],
            "tool_result" => {
                let is_error = str_at(event, "/status") == Some("error");
                let output = if is_error {
                    str_at(event, "/error/message")
                        .unwrap_or("the tool failed")
                        .to_string()
                } else {
                    event.get("output").map(text_of).unwrap_or_default()
                };
                vec![AgentEvent::ToolResult {
                    id: str_at(event, "/tool_id").map(str::to_string),
                    output,
                    is_error,
                }]
            }
            "error" => {
                let message = str_at(event, "/message").unwrap_or("error").to_string();
                if str_at(event, "/severity") == Some("warning") {
                    vec![AgentEvent::Warning { message }]
                } else {
                    vec![AgentEvent::Error { message }]
                }
            }
            "result" => {
                transcript.total_usage(token_usage(event.get("stats")));
                match str_at(event, "/status") {
                    Some(status) if status != "success" => {
                        transcript.failed(str_at(event, "/error/message").unwrap_or(status))
                    }
                    _ => transcript.succeeded(None),
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// Antigravity CLI: `agy --print <prompt> --output-format stream-json`.
///
/// Events are keyed by `event`: `init`, `step_update` (text deltas and tool
/// steps) and a final `result` with the status and usage. A run whose
/// actions were refused still exits 0, so the status decides.
pub(crate) struct Antigravity;

impl Adapter for Antigravity {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn label(&self) -> &'static str {
        "Antigravity CLI"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["agy"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["agy"]
    }

    fn env_markers(&self) -> &'static [&'static str] {
        &["ANTIGRAVITY_AGENT"]
    }

    /// Without `--dangerously-skip-permissions` the actions that need
    /// permission are refused.
    fn read_only(&self) -> ReadOnly {
        ReadOnly::AgentDefaults
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let launch = Launch::new(AntigravityParser)
            .args(["--output-format", "stream-json"])
            .option("--model", task.model);
        let launch = match task.autonomy {
            Autonomy::Full
                if supports_flag(executable, &["--help"], "--dangerously-skip-permissions") =>
            {
                launch.arg("--dangerously-skip-permissions")
            }
            _ => launch,
        };
        launch.arg("--print").arg(not_a_flag(with_instructions(
            task.prompt,
            task.system_prompt,
        )))
    }
}

struct AntigravityParser;

impl Parser for AntigravityParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let kind = str_at(event, "/event").unwrap_or_default();
        let body = event.get(kind).unwrap_or(&Value::Null);
        match kind {
            "init" => vec![started(transcript, None, str_at(body, "/model"))],
            "step_update" => {
                if let Some(session) = str_at(body, "/conversation_id") {
                    transcript.session(session);
                }
                let mut events = Vec::new();
                if let Some(text) = str_at(body, "/text_delta").filter(|t| !t.is_empty()) {
                    events.push(AgentEvent::Text {
                        text: text.to_string(),
                        delta: true,
                    });
                }
                if str_at(body, "/step_type") == Some("tool") {
                    let id = body.get("step_index").map(|index| index.to_string());
                    let info = body.get("tool_info").unwrap_or(&Value::Null);
                    if str_at(body, "/state") == Some("ACTIVE") {
                        events.push(tool_call(
                            id.as_deref(),
                            str_at(info, "/name")
                                .or_else(|| str_at(body, "/tool_name"))
                                .unwrap_or("tool"),
                            info.get("parameters").cloned().unwrap_or(Value::Null),
                        ));
                    } else if str_at(body, "/state") == Some("DONE") {
                        let error = info.get("error").filter(|e| !e.is_null());
                        events.push(AgentEvent::ToolResult {
                            id,
                            output: error
                                .or_else(|| info.get("output"))
                                .map(text_of)
                                .unwrap_or_default(),
                            is_error: error.is_some(),
                        });
                    }
                }
                events
            }
            "result" => {
                if let Some(session) = str_at(body, "/conversation_id") {
                    transcript.session(session);
                }
                transcript.total_usage(token_usage(body.get("usage")));
                match str_at(body, "/status") {
                    Some(status) if status != "SUCCESS" => transcript.failed(
                        str_at(body, "/error/message")
                            .or_else(|| str_at(body, "/error"))
                            .unwrap_or(status),
                    ),
                    _ => transcript.succeeded(str_at(body, "/response")),
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
