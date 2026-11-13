//! The interface each coding agent implements: how to launch it headlessly,
//! and how to read what it prints.

use std::ffi::OsString;
use std::path::Path;

use crate::event::{AgentEvent, Usage};

/// What to ask an agent.
#[derive(Debug, Clone)]
pub struct Task<'a> {
    pub prompt: &'a str,
    /// Extra instructions. Agents with a flag for them get them there; the
    /// others get them in front of the prompt.
    pub system_prompt: Option<&'a str>,
    /// A model name the agent understands; `None` uses the agent's default.
    pub model: Option<&'a str>,
    /// The directory the agent works in.
    pub cwd: &'a Path,
    pub autonomy: Autonomy,
}

/// How much the agent may do without asking. Headless runs cannot ask, so
/// whatever needs approval beyond this is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Autonomy {
    /// Edit files and run commands without asking.
    #[default]
    Full,
    /// Read and answer only: the agent's read-only or planning mode where it
    /// has one, otherwise its default permissions.
    ReadOnly,
}

/// How to start the agent for a task.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
    /// Written to the agent's stdin, which is then closed. `None` when the
    /// prompt is in `args`; stdin is then empty.
    pub stdin: Option<String>,
}

impl Launch {
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// `flag value` when `value` is set.
    pub fn option(self, flag: &str, value: Option<&str>) -> Self {
        match value {
            Some(value) => self.arg(flag).arg(value),
            None => self,
        }
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }
}

/// A coding agent.
pub trait Adapter: Sync {
    /// Stable id: `"claude-code"`, `"codex"`, ...
    fn id(&self) -> &'static str;

    /// Name for people: `"Claude Code"`.
    fn label(&self) -> &'static str;

    /// Other names it is known by (`--agent claude`).
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    /// Executable names, in order of preference.
    fn executables(&self) -> &'static [&'static str];

    /// Environment variables the agent sets in the processes it starts: when
    /// one is set, this program runs inside the agent.
    fn session_env(&self) -> &'static [&'static str] {
        &[]
    }

    /// The command line for a task. `executable` is the program that will
    /// run, for adapters that need to look at it.
    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch;

    /// A fresh reader for one run's stdout.
    fn parser(&self) -> Box<dyn Parser>;
}

/// Reads an agent's stdout, line by line.
pub trait Parser: Send {
    /// The events in one line. Facts about the whole run (the final answer,
    /// the session, usage, a failure) go to `transcript`.
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent>;

    /// The output has ended: events and facts from what was buffered, for
    /// agents that print one report at the end.
    fn finish(&mut self, _transcript: &mut Transcript) -> Vec<AgentEvent> {
        Vec::new()
    }
}

/// What a run comes to, as the parser learns it.
#[derive(Debug, Default)]
pub struct Transcript {
    result: Option<String>,
    session_id: Option<String>,
    model: Option<String>,
    usage: Usage,
    usage_is_total: bool,
    error: Option<String>,
    last_text: String,
    text_open: bool,
}

impl Transcript {
    /// The agent's final answer, when it reports one explicitly.
    pub fn result(&mut self, text: impl Into<String>) {
        self.result = Some(text.into());
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn session(&mut self, id: impl Into<String>) {
        self.session_id.get_or_insert_with(|| id.into());
    }

    pub fn model(&mut self, model: impl Into<String>) {
        self.model.get_or_insert_with(|| model.into());
    }

    /// Usage for one turn; turns add up.
    pub fn turn_usage(&mut self, usage: Usage) {
        if !self.usage_is_total {
            self.usage.add(&usage);
        }
    }

    /// Usage for the whole run; replaces what turns added up to.
    pub fn total_usage(&mut self, usage: Usage) {
        self.usage = usage;
        self.usage_is_total = true;
    }

    /// The agent reported that the run failed.
    pub fn fail(&mut self, message: impl Into<String>) {
        self.error.get_or_insert_with(|| message.into());
    }

    /// Text the agent wrote: the last text written is the answer unless
    /// the agent reports one with [`Transcript::result`].
    pub fn text(&mut self, text: &str, delta: bool) {
        if delta && self.text_open {
            self.last_text.push_str(text);
        } else {
            self.last_text = text.to_string();
        }
        self.text_open = true;
    }

    /// Something other than text happened: the next text starts over.
    pub fn close_text(&mut self) {
        self.text_open = false;
    }

    pub(crate) fn finish(self) -> Finished {
        let result = self.result.or_else(|| {
            let text = self.last_text.trim();
            (!text.is_empty()).then(|| text.to_string())
        });
        Finished {
            result,
            session_id: self.session_id,
            model: self.model,
            usage: self.usage,
            error: self.error,
        }
    }
}

pub(crate) struct Finished {
    pub result: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub usage: Usage,
    pub error: Option<String>,
}

/// The prompt with extra instructions in front, for agents without a flag
/// for them.
pub fn with_instructions(prompt: &str, system_prompt: Option<&str>) -> String {
    match system_prompt.map(str::trim).filter(|s| !s.is_empty()) {
        Some(instructions) => {
            format!("<instructions>\n{instructions}\n</instructions>\n\n{prompt}")
        }
        None => prompt.to_string(),
    }
}

/// The string at a JSON pointer.
pub fn str_at<'a>(value: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(serde_json::Value::as_str)
}

/// A line as JSON, when it is a JSON object.
pub fn json_line(line: &str) -> Option<serde_json::Value> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    serde_json::from_str(line)
        .ok()
        .filter(serde_json::Value::is_object)
}

/// Reads lines that are not JSON as text: for agents that only print text.
#[derive(Default)]
pub struct TextParser {
    text: String,
}

impl Parser for TextParser {
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        self.text.push_str(line);
        self.text.push('\n');
        transcript.result(self.text.trim_end().to_string());
        vec![AgentEvent::Text {
            text: format!("{line}\n"),
            delta: true,
        }]
    }
}
