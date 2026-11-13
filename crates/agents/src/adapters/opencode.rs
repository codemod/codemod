//! OpenCode: `opencode run --format json`, the prompt on stdin.
//!
//! Events carry a `sessionID` and a `part`: `text`, `reasoning`, `tool_use`
//! (with the tool's input, output and status), `step_finish` (tokens and
//! cost) and `error`.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{Adapter, Autonomy, Launch, Parser, ReadOnly, Task};
use crate::event::{AgentEvent, Usage};
use crate::parse::{started, str_at, tool_call, with_instructions};
use crate::probe::supports_flag;
use crate::transcript::Transcript;

pub(crate) struct OpenCode;

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

    fn env_markers(&self) -> &'static [&'static str] {
        &["OPENCODE", "OPENCODE_SESSION_ID", "OPENCODE_CLIENT"]
    }

    fn read_only(&self) -> ReadOnly {
        ReadOnly::Enforced
    }

    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch {
        let launch = Launch::new(OpenCodeParser)
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
}

struct OpenCodeParser;

impl Parser for OpenCodeParser {
    fn event(&mut self, event: &Value, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if let Some(session) = str_at(event, "/sessionID") {
            if transcript.session_id().is_none() {
                events.push(started(transcript, Some(session), None));
            }
        }
        let part = event.get("part").unwrap_or(&Value::Null);
        match str_at(event, "/type").unwrap_or_default() {
            "text" => events.extend(str_at(part, "/text").map(|text| AgentEvent::Text {
                text: text.to_string(),
                delta: false,
            })),
            "reasoning" => events.extend(str_at(part, "/text").map(|text| AgentEvent::Thinking {
                text: text.to_string(),
            })),
            "tool_use" => {
                let id = str_at(part, "/callID");
                let mut call = tool_call(
                    id,
                    str_at(part, "/tool").unwrap_or("tool"),
                    part.pointer("/state/input").cloned().unwrap_or(Value::Null),
                );
                // A finished edit carries the diff it made.
                let diff = str_at(part, "/state/metadata/diff").filter(|d| !d.trim().is_empty());
                if let (AgentEvent::ToolCall { changes, .. }, Some(diff)) = (&mut call, diff) {
                    if let [change] = changes.as_mut_slice() {
                        change.diff = Some(diff.to_string());
                    }
                }
                events.push(call);
                let result = |output: &str, is_error| AgentEvent::ToolResult {
                    id: id.map(str::to_string),
                    output: str_at(part, output).unwrap_or_default().to_string(),
                    is_error,
                };
                match str_at(part, "/state/status") {
                    Some("completed") => events.push(result("/state/output", false)),
                    Some("error") => events.push(result("/state/error", true)),
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
                let message = str_at(event, "/error/data/message")
                    .or_else(|| str_at(event, "/error/message"))
                    .or_else(|| str_at(event, "/error/name"))
                    .unwrap_or("error")
                    .to_string();
                transcript.error(&message);
                events.push(AgentEvent::Error { message });
            }
            _ => {}
        }
        events
    }
}
