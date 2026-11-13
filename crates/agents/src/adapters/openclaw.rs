//! OpenClaw: `openclaw agent exec --json --message-file -`, the prompt on
//! stdin.
//!
//! It streams nothing: one JSON report comes at the end, `{ ok, final,
//! usage, costUsd, sessionId, error }`.

use std::path::Path;

use serde_json::Value;

use crate::adapter::{Adapter, Launch, Parser, ReadOnly, Task};
use crate::event::{AgentEvent, Usage};
use crate::parse::{str_at, with_instructions};
use crate::transcript::Transcript;

pub(crate) struct OpenClaw;

impl Adapter for OpenClaw {
    fn id(&self) -> &'static str {
        "openclaw"
    }

    fn label(&self) -> &'static str {
        "OpenClaw"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["open-claw"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["openclaw"]
    }

    fn env_markers(&self) -> &'static [&'static str] {
        &["OPENCLAW_SHELL", "OPENCLAW_CLI"]
    }

    /// `agent exec` runs with full tool access, its file tools kept to --cwd.
    fn read_only(&self) -> ReadOnly {
        ReadOnly::Unsupported
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        Launch::new(OpenClawParser::default())
            .args(["agent", "exec", "--json", "--message-file", "-"])
            .arg("--cwd")
            .arg(task.cwd)
            .option("--model", task.model)
            .stdin(with_instructions(task.prompt, task.system_prompt))
    }
}

#[derive(Default)]
struct OpenClawParser {
    output: String,
}

impl Parser for OpenClawParser {
    fn parse(&mut self, line: &str, _transcript: &mut Transcript) -> Vec<AgentEvent> {
        self.output.push_str(line);
        self.output.push('\n');
        Vec::new()
    }

    fn finish(&mut self, transcript: &mut Transcript) -> Vec<AgentEvent> {
        let text = |text: &str| AgentEvent::Text {
            text: text.to_string(),
            delta: false,
        };
        // The report may be preceded by log lines: read from its first brace.
        let start = self.output.find('{').unwrap_or(0);
        let Ok(report) = serde_json::from_str::<Value>(self.output[start..].trim()) else {
            return match self.output.trim() {
                "" => Vec::new(),
                output => vec![text(output)],
            };
        };
        if let Some(session) = str_at(&report, "/sessionId") {
            transcript.session(session);
        }
        if let Some(model) = str_at(&report, "/model") {
            transcript.model(model);
        }
        transcript.total_usage(Usage {
            input_tokens: report.pointer("/usage/input").and_then(Value::as_u64),
            output_tokens: report.pointer("/usage/output").and_then(Value::as_u64),
            cost_usd: report.get("costUsd").and_then(Value::as_f64),
        });
        if report.get("ok").and_then(Value::as_bool) == Some(false) {
            let message = str_at(&report, "/error/message")
                .or_else(|| str_at(&report, "/status"))
                .unwrap_or("error")
                .to_string();
            transcript.failed(&message);
            return vec![AgentEvent::Error { message }];
        }
        let answer = str_at(&report, "/final");
        transcript.succeeded(answer);
        answer.map(text).into_iter().collect()
    }
}
