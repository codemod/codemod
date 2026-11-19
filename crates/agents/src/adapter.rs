//! The interface each coding agent implements: how to launch it headlessly,
//! and how to read what it prints.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::event::{AgentEvent, Stream};
use crate::parse::json_line;
use crate::transcript::Transcript;

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
    /// Read and answer only. How an agent is held to that is its
    /// [`Adapter::read_only`]; an agent that cannot be held to it does not run.
    ReadOnly,
}

/// How an agent is kept from changing anything in a read-only task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnly {
    /// The launch passes the agent's own read-only flag or mode.
    Enforced,
    /// Nothing is passed. Left to its defaults the agent asks before it
    /// writes a file or runs a command, and a headless run cannot answer.
    /// The person's own configuration of the agent can allow more.
    AgentDefaults,
    /// The agent cannot be kept from writing: it does not run read-only tasks.
    Unsupported,
}

/// How to start the agent for a task, and how to read what it then prints.
pub struct Launch {
    pub(crate) args: Vec<OsString>,
    pub(crate) env: Vec<(String, String)>,
    /// Written to the agent's stdin, which is then closed. `None` when the
    /// prompt is in `args`; stdin is then empty.
    pub(crate) stdin: Option<String>,
    /// Reads the output format these arguments ask for.
    pub(crate) parser: Box<dyn Parser>,
}

impl Launch {
    pub(crate) fn new(parser: impl Parser + 'static) -> Self {
        Self {
            args: Vec::new(),
            env: Vec::new(),
            stdin: None,
            parser: Box::new(parser),
        }
    }

    pub(crate) fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub(crate) fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// `flag value` when `value` is set. A value that starts with `-` is
    /// joined to its flag, or the agent would read it as another flag.
    pub(crate) fn option(self, flag: &str, value: Option<&str>) -> Self {
        match value {
            Some(value) if value.starts_with('-') => self.joined(flag, value),
            Some(value) => self.arg(flag).arg(value),
            None => self,
        }
    }

    /// `flag=value`: the value cannot be read as a flag, whatever it starts
    /// with.
    pub(crate) fn joined(self, flag: &str, value: &str) -> Self {
        self.arg(format!("{flag}={value}"))
    }

    /// `-- value`: after `--` nothing is read as a flag.
    pub(crate) fn positional(self, value: impl Into<OsString>) -> Self {
        self.arg("--").arg(value)
    }

    pub(crate) fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    pub(crate) fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }
}

/// A coding agent. The adapters are in this crate: get one from
/// [`crate::find`] or [`crate::agents`].
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

    /// Environment variables the agent sets for the commands it runs. One
    /// being set means this program runs inside the agent. A run of the
    /// agent takes them out of the environment it starts the agent in, so
    /// the agent starts a session of its own, not one inside ours.
    fn env_markers(&self) -> &'static [&'static str] {
        &[]
    }

    /// Environment variables the agent's editor sets in its terminal. A
    /// weaker sign than [`Adapter::env_markers`]: a person, or another
    /// agent, may be what runs there.
    fn editor_env_markers(&self) -> &'static [&'static str] {
        &[]
    }

    /// How a read-only task is kept read-only.
    fn read_only(&self) -> ReadOnly;

    /// The command line for a task, with the parser for the output it asks
    /// for. `executable` is the program that will run, for adapters that
    /// need to look at it. A [`crate::Run`] does not ask for the launch of a
    /// read-only task when [`Adapter::read_only`] is `Unsupported`.
    fn launch(&self, task: &Task<'_>, executable: &Path) -> Launch;

    /// Where the agent's executable is, when it is installed.
    fn installed(&self) -> Option<PathBuf> {
        self.executables()
            .iter()
            .find_map(|name| crate::which::which(name))
    }
}

/// Reads an agent's stdout, line by line.
pub trait Parser: Send {
    /// The events in one line. Facts about the whole run (the final answer,
    /// the session, usage, a failure) go to `transcript`.
    ///
    /// Unless an agent's output is not JSON lines, implement
    /// [`Parser::event`] instead: this hands it each line that is a JSON
    /// object, skips blank lines, and reports any other line as output the
    /// adapter does not understand.
    fn parse(&mut self, line: &str, transcript: &mut Transcript) -> Vec<AgentEvent> {
        if line.trim().is_empty() {
            return Vec::new();
        }
        match json_line(line) {
            Some(event) => self.event(&event, transcript),
            None => vec![AgentEvent::Output {
                stream: Stream::Stdout,
                line: line.to_string(),
            }],
        }
    }

    /// The events in one JSON object the agent printed.
    fn event(&mut self, _event: &Value, _transcript: &mut Transcript) -> Vec<AgentEvent> {
        Vec::new()
    }

    /// The output has ended: events and facts from what was buffered, for
    /// agents that print one report at the end.
    fn finish(&mut self, _transcript: &mut Transcript) -> Vec<AgentEvent> {
        Vec::new()
    }
}
