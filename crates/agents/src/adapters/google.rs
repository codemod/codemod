//! Google's agents: Gemini CLI and its successor, Antigravity CLI.

use std::path::Path;

use serde_json::Value;

use super::claude_like::text_of;
use crate::adapter::{
    json_line, str_at, with_instructions, Adapter, Autonomy, Launch, Parser, Task, TextParser,
    Transcript,
};
use crate::event::{AgentEvent, Stream, Usage};
use crate::probe::{help_text, supports_flag};

/// Gemini CLI: `gemini --output-format stream-json --prompt <prompt>`.
///
/// Older versions have no structured output; their text is read as is.
/// Events: `init`, `message` (with `delta`), `tool_use`, `tool_result`,
/// `error` and a final `result` with stats.
pub struct GeminiCli;

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

    fn session_env(&self) -> &'static [&'static str] {
        &["GEMINI_CLI"]
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let help = help_text(executable, &["--help"]);
        let mut launch = Launch::default().option("--model", task.model);
        if help.contains("stream-json") {
            launch = launch.args(["--output-format", "stream-json"]);
        }
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
        launch
            .arg("--prompt")
            .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(GeminiParser::default())
    }
}

#[derive(Default)]
struct GeminiParser {
    /// Older versions print text only.
    text: Option<TextParser>,
}

impl Parser for GeminiParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        if let Some(text) = &mut self.text {
            return text.parse(line, transcript);
        }
        let Some(event) = json_line(line) else {
            if transcript.session_id().is_none() {
                // Not stream-json: an older version.
                let text = self.text.insert(TextParser::default());
                return text.parse(line, transcript);
            }
            return vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }];
        };
        match str_at(&event, "/type").unwrap_or_default() {
            "init" => {
                let session = str_at(&event, "/session_id").map(str::to_string);
                let model = str_at(&event, "/model").map(str::to_string);
                if let Some(session) = &session {
                    transcript.session(session.clone());
                }
                if let Some(model) = &model {
                    transcript.model(model.clone());
                }
                vec![AgentEvent::Started {
                    session_id: session,
                    model,
                }]
            }
            "message" if str_at(&event, "/role") == Some("assistant") => {
                vec![AgentEvent::Text {
                    text: str_at(&event, "/content").unwrap_or_default().to_string(),
                    delta: event.get("delta").and_then(Value::as_bool).unwrap_or(false),
                }]
            }
            "tool_use" => vec![AgentEvent::ToolCall {
                id: str_at(&event, "/tool_id").map(str::to_string),
                name: str_at(&event, "/tool_name").unwrap_or("tool").to_string(),
                input: event.get("parameters").cloned().unwrap_or(Value::Null),
            }],
            "tool_result" => {
                let is_error = str_at(&event, "/status") == Some("error");
                let output = if is_error {
                    str_at(&event, "/error/message")
                        .unwrap_or("the tool failed")
                        .to_string()
                } else {
                    event.get("output").map(text_of).unwrap_or_default()
                };
                vec![AgentEvent::ToolResult {
                    id: str_at(&event, "/tool_id").map(str::to_string),
                    output,
                    is_error,
                }]
            }
            "error" => {
                let message = str_at(&event, "/message").unwrap_or("error").to_string();
                if str_at(&event, "/severity") == Some("warning") {
                    vec![AgentEvent::Warning { message }]
                } else {
                    vec![AgentEvent::Error { message }]
                }
            }
            "result" => {
                transcript.total_usage(Usage {
                    input_tokens: event.pointer("/stats/input_tokens").and_then(Value::as_u64),
                    output_tokens: event
                        .pointer("/stats/output_tokens")
                        .and_then(Value::as_u64),
                    cost_usd: None,
                });
                if str_at(&event, "/status").is_some_and(|status| status != "success") {
                    transcript.fail(
                        str_at(&event, "/error/message")
                            .or_else(|| str_at(&event, "/status"))
                            .unwrap_or("error"),
                    );
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
pub struct Antigravity;

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

    fn session_env(&self) -> &'static [&'static str] {
        &["ANTIGRAVITY_AGENT"]
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let launch = Launch::default()
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
        launch
            .arg("--print")
            .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(AntigravityParser)
    }
}

struct AntigravityParser;

impl Parser for AntigravityParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let Some(event) = json_line(line) else {
            return vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }];
        };
        let kind = str_at(&event, "/event").unwrap_or_default();
        let body = event.get(kind).cloned().unwrap_or(Value::Null);
        match kind {
            "init" => {
                let model = str_at(&body, "/model").map(str::to_string);
                if let Some(model) = &model {
                    transcript.model(model.clone());
                }
                vec![AgentEvent::Started {
                    session_id: None,
                    model,
                }]
            }
            "step_update" => {
                if let Some(session) = str_at(&body, "/conversation_id") {
                    transcript.session(session);
                }
                let mut events = Vec::new();
                if let Some(text) = str_at(&body, "/text_delta").filter(|t| !t.is_empty()) {
                    events.push(AgentEvent::Text {
                        text: text.to_string(),
                        delta: true,
                    });
                }
                if str_at(&body, "/step_type") == Some("tool") {
                    let id = body.get("step_index").map(|index| index.to_string());
                    let info = body.get("tool_info").cloned().unwrap_or(Value::Null);
                    if str_at(&body, "/state") == Some("ACTIVE") {
                        events.push(AgentEvent::ToolCall {
                            id,
                            name: str_at(&info, "/name")
                                .or_else(|| str_at(&body, "/tool_name"))
                                .unwrap_or("tool")
                                .to_string(),
                            input: info.get("parameters").cloned().unwrap_or(Value::Null),
                        });
                    } else if str_at(&body, "/state") == Some("DONE") {
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
                if let Some(session) = str_at(&body, "/conversation_id") {
                    transcript.session(session);
                }
                transcript.total_usage(Usage {
                    input_tokens: body.pointer("/usage/input_tokens").and_then(Value::as_u64),
                    output_tokens: body.pointer("/usage/output_tokens").and_then(Value::as_u64),
                    cost_usd: None,
                });
                if let Some(response) = str_at(&body, "/response") {
                    transcript.result(response);
                }
                if str_at(&body, "/status").is_some_and(|status| status != "SUCCESS") {
                    transcript.fail(
                        str_at(&body, "/error/message")
                            .or_else(|| str_at(&body, "/error"))
                            .or_else(|| str_at(&body, "/status"))
                            .unwrap_or("error"),
                    );
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
