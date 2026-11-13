//! Host tasks: the work a step hands to Rust. Each task is a thin adapter
//! over an existing executor.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::Event;
use crate::run::RunContext;

mod agent;
mod approval;
mod ast_grep;
pub(crate) mod changes;
mod codemod;
mod dependency_graph;
pub(crate) mod for_each_file;
mod function;
mod git;
mod install_skill;
pub(crate) mod isolate;
mod jssg;
mod shard;
mod shell;

/// One task invocation.
pub(crate) struct TaskContext<'a> {
    pub run: &'a Arc<RunContext>,
    /// Position of the step running this task, for events.
    pub position: u64,
    pub label: String,
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

    pub fn log(&self, line: impl Into<String>) {
        self.run.events.emit(Event::StepLog {
            position: self.position,
            line: line.into(),
        });
    }

    /// A progress callback for `CodemodExecutionConfig` walks.
    pub fn walk_progress(&self) -> butterflow_core::execution::ProgressCallback {
        let events = Arc::clone(&self.run.events);
        let position = self.position;
        butterflow_core::execution::ProgressCallback {
            callback: Arc::new(Box::new(move |_task, _path, status, total, processed| {
                if status == "increment" || status == "finish" {
                    events.emit(Event::StepProgress {
                        position,
                        processed: *processed as usize,
                        total: total.copied().unwrap_or(*processed) as usize,
                    });
                }
            })),
        }
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

    pub fn with_state(mut self, state: BTreeMap<String, Value>) -> Self {
        if !state.is_empty() {
            self.state = Some(state);
        }
        self
    }
}

/// Why a task did not produce an outcome.
#[derive(Debug)]
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

pub(crate) type TaskResult = Result<TaskOutcome, TaskError>;

pub(crate) fn dispatch(task: &TaskContext<'_>, content: Value) -> TaskResult {
    if task.run.canceled() {
        return Err(TaskError::Canceled);
    }
    let kind = content
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let result = match kind.as_str() {
        "forEachFile" => for_each_file::run(task, content).map_err(TaskError::from),
        "function" => function::run(task, content).map_err(TaskError::from),
        "dependencyGraph" => dependency_graph::run(task, content).map_err(TaskError::from),
        "shell" => shell::run(task, parse(content)?),
        "jssg" => jssg::run(task, parse(content)?),
        "astGrep" => ast_grep::run(task, parse(content)?),
        "shard" => shard::run(task, content),
        "codemod" => codemod::run(task, parse(content)?),
        "agent" => agent::run(task, parse(content)?),
        "installSkill" => install_skill::run(task, content),
        "approval" => approval::run(task, parse(content)?),
        "branch" => git::branch(task, parse(content)?),
        "commit" => git::commit(task, parse(content)?),
        "pullRequest" => git::pull_request(task, parse(content)?),
        other => Err(TaskError::Failed(format!("unknown task kind '{other}'"))),
    };
    if task.run.canceled() && result.is_err() {
        return Err(TaskError::Canceled);
    }
    result
}

fn parse<T: DeserializeOwned>(content: Value) -> Result<T, TaskError> {
    let kind = content
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("task")
        .to_string();
    serde_json::from_value(content)
        .map_err(|e| TaskError::Failed(format!("invalid {kind} options: {e}")))
}

/// Run a future to completion on a fresh single-threaded runtime.
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
}

/// A glob or a list of globs.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum Globs {
    One(String),
    Many(Vec<String>),
}

impl Globs {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(glob) => vec![glob],
            Self::Many(globs) => globs,
        }
    }
}

/// Parent environment plus the variables every child process gets.
pub(crate) fn child_env(task: &TaskContext<'_>) -> std::collections::HashMap<String, String> {
    let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
    if !task.run.interaction.is_interactive() {
        env.insert("npm_config_yes".into(), "true".into());
    }
    env.insert(
        "CODEMOD_PATH".into(),
        task.run.bundle_dir.to_string_lossy().into_owned(),
    );
    env.insert("CODEMOD_WORKFLOW_RUN_ID".into(), task.run.run_id.clone());
    env.insert(
        "CODEMOD_TASK_ID".into(),
        format!("{}:{}", task.run.run_id, task.position),
    );
    env
}
