//! Host tasks: the work a step hands to Rust. Each task is a thin adapter
//! over an existing executor.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::events::Event;
use crate::run::RunContext;

pub(crate) mod for_each_file;

/// One task invocation.
pub(crate) struct TaskContext<'a> {
    pub run: &'a Arc<RunContext>,
    /// Position of the step running this task, for events.
    pub position: u64,
    /// Workflow state when the step started.
    pub state: serde_json::Map<String, Value>,
}

impl TaskContext<'_> {
    pub fn progress(&self, processed: usize, total: usize) {
        self.run.events.emit(Event::StepProgress {
            position: self.position,
            processed,
            total,
        });
    }

    #[allow(dead_code)]
    pub fn log(&self, line: impl Into<String>) {
        self.run.events.emit(Event::StepLog {
            position: self.position,
            line: line.into(),
        });
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct TaskOutcome {
    pub result: Value,
    /// State the task set: keys to values, `null` removes a key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<BTreeMap<String, Value>>,
}

impl TaskOutcome {
    pub fn new(result: Value) -> Self {
        Self {
            result,
            state: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_state(mut self, state: BTreeMap<String, Value>) -> Self {
        if !state.is_empty() {
            self.state = Some(state);
        }
        self
    }
}

/// Why a task did not produce an outcome.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum TaskError {
    Failed(String),
    /// A manual approval nobody could give now.
    AwaitingApproval(String),
    Canceled,
}

impl From<String> for TaskError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl TaskError {
    /// The payload the runtime turns back into a JavaScript error.
    pub fn to_payload(&self) -> String {
        let (name, message) = match self {
            Self::Failed(message) => ("Error", message.as_str()),
            Self::AwaitingApproval(message) => ("AwaitingApprovalError", message.as_str()),
            Self::Canceled => ("CanceledError", "the run was canceled"),
        };
        serde_json::json!({ "name": name, "message": message }).to_string()
    }
}

pub(crate) fn dispatch(task: &TaskContext<'_>, content: Value) -> Result<TaskOutcome, TaskError> {
    if task.run.canceled() {
        return Err(TaskError::Canceled);
    }
    let kind = content
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match kind.as_str() {
        "forEachFile" => for_each_file::run(task, content).map_err(TaskError::from),
        other => Err(TaskError::Failed(format!("unknown task kind '{other}'"))),
    }
}
