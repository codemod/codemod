//! OpenCode: `opencode run --format json`, the prompt on stdin.
//!
//! Events carry a `sessionID` and a `part`: `text`, `reasoning`, `tool_use`
//! (with the tool's input, output and status), `step_finish` (tokens and
//! cost) and `error`.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{
    json_line, str_at, with_instructions, Adapter, Autonomy, Launch, Parser, Task, Transcript,
};
use crate::event::{AgentEvent, Stream, Usage};
use crate::probe::supports_flag;

pub struct OpenCode;

impl Adapter for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["open-code"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["opencode"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &["OPENCODE", "OPENCODE_SESSION_ID"]
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let launch = Launch::default()
            .args(["run", "--format", "json"])
            .option("--model", task.model);
        let launch = match task.autonomy {
            // `--auto` replaced `--dangerously-skip-permissions` in 1.x.
            Autonomy::Full if supports_flag(executable, &["run", "--help"], "--auto") => {
                launch.arg("--auto")
            }
            Autonomy::Full => launch.arg("--dangerously-skip-permissions"),
            Autonomy::ReadOnly => launch.args(["--agent", "plan"]),
        };
        launch.stdin(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(OpenCodeParser)
    }
}

pub(crate) struct OpenCodeParser;

impl Parser for OpenCodeParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let Some(event) = json_line(line) else {
            return vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }];
        };
        let mut events = Vec::new();
        if let Some(session) = str_at(&event, "/sessionID") {
            if transcript_session_is_new(transcript, session) {
                events.push(AgentEvent::Started {
                    session_id: Some(session.to_string()),
                    model: None,
                });
            }
        }
        let part = event.get("part").cloned().unwrap_or(Value::Null);
        match str_at(&event, "/type").unwrap_or_default() {
            "text" => events.extend(str_at(&part, "/text").map(|text| AgentEvent::Text {
                text: text.to_string(),
                delta: false,
            })),
            "reasoning" => events.extend(str_at(&part, "/text").map(|text| AgentEvent::Thinking {
                text: text.to_string(),
            })),
            "tool_use" => {
                let id = str_at(&part, "/callID").map(str::to_string);
                events.push(AgentEvent::ToolCall {
                    id: id.clone(),
                    name: str_at(&part, "/tool").unwrap_or("tool").to_string(),
                    input: part.pointer("/state/input").cloned().unwrap_or(Value::Null),
                });
                match str_at(&part, "/state/status") {
                    Some("completed") => events.push(AgentEvent::ToolResult {
                        id,
                        output: str_at(&part, "/state/output")
                            .unwrap_or_default()
                            .to_string(),
                        is_error: false,
                    }),
                    Some("error") => events.push(AgentEvent::ToolResult {
                        id,
                        output: str_at(&part, "/state/error")
                            .unwrap_or_default()
                            .to_string(),
                        is_error: true,
                    }),
                    _ => {}
                }
            }
            "step_finish" => transcript.turn_usage(Usage {
                input_tokens: part
                    .pointer("/tokens/input")
                    .and_then(Value::as_u64)
                    .map(|n| {
                        n + part
                            .pointer("/tokens/cache/read")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                    }),
                output_tokens: part.pointer("/tokens/output").and_then(Value::as_u64),
                cost_usd: part.get("cost").and_then(Value::as_f64),
            }),
            "error" => {
                let message = str_at(&event, "/error/data/message")
                    .or_else(|| str_at(&event, "/error/message"))
                    .or_else(|| str_at(&event, "/error/name"))
                    .unwrap_or("error")
                    .to_string();
                transcript.fail(message.clone());
                events.push(AgentEvent::Error { message });
            }
            _ => {}
        }
        events
    }
}

/// Record the session; true the first time it is seen.
fn transcript_session_is_new(transcript: &mut Transcript, session: &str) -> bool {
    let new = transcript.session_id().is_none();
    transcript.session(session);
    new
}
