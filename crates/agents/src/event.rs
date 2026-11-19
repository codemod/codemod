//! What a coding agent did, in one shape for every agent.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One thing an agent did, normalized from its own output format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent started a session.
    Started {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Text the agent wrote. `delta` text continues the previous text.
    Text { text: String, delta: bool },
    /// The agent's reasoning, when it shows it.
    Thinking { text: String },
    /// The agent called a tool: ran a command, edited a file, searched, ...
    ToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        input: Value,
        /// The files the call changes, when the agent's event says.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changes: Vec<FileChange>,
    },
    /// What a tool call returned.
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        output: String,
        is_error: bool,
    },
    /// Something went wrong but the agent carries on.
    Warning { message: String },
    /// The agent reported a failure.
    Error { message: String },
    /// A line of output the adapter does not understand.
    Output { stream: Stream, line: String },
}

/// A change a tool call makes to a file, in one shape for every agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
    /// The text that is replaced, when the agent gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    /// The new text: the whole file for `create` and `write`, otherwise what
    /// replaces `old`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
    /// A unified diff of the change, when the agent gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileChangeKind {
    Create,
    /// The file gets new content, whether or not it existed.
    Write,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Tokens and cost of a run, as far as the agent reports them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none() && self.output_tokens.is_none() && self.cost_usd.is_none()
    }

    /// Add another report to this one (agents report usage per turn).
    pub fn add(&mut self, other: &Usage) {
        fn sum<T: std::ops::Add<Output = T> + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            }
        }
        self.input_tokens = sum(self.input_tokens, other.input_tokens);
        self.output_tokens = sum(self.output_tokens, other.output_tokens);
        self.cost_usd = sum(self.cost_usd, other.cost_usd);
    }
}
