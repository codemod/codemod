//! Host tasks: the work a step hands to Rust. Each task is a thin adapter
//! over an existing executor.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use butterflow_state::run_store::FileCount;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::{Event, StepAt};
use crate::interaction::{Approval, Ask, Decision};
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
mod http;
mod install_skill;
pub(crate) mod isolate;
pub(crate) mod overlay;
mod reduce;
mod shard;
mod shell;
mod v1_host;
mod wait;

pub(crate) use wait::describe as describe_duration;

/// `f` of each of `items`, in order, over as many threads as there are cores.
pub(crate) fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(items.len().div_ceil(64))
        .max(1);
    if threads == 1 {
        return items.iter().map(f).collect();
    }
    let f = &f;
    std::thread::scope(|scope| {
        items
            .chunks(items.len().div_ceil(threads))
            .map(|chunk| scope.spawn(move || chunk.iter().map(f).collect::<Vec<R>>()))
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

/// One task invocation.
pub(crate) struct TaskContext<'a> {
    pub run: &'a Arc<RunContext>,
    /// Position of the step running this task, for events.
    pub position: u64,
    pub label: String,
    /// Set when the task must stop: the run was canceled, or the step ran
    /// out of time. The run's own flag for a step without a timeout.
    pub cancel: Arc<AtomicBool>,
    /// When the step began to wait, for `sleep` and `waitForEvent`, in
    /// milliseconds since the epoch.
    pub since: Option<i64>,
    /// Other steps were still running when the step was issued
    /// (`StartRequest.alongside` in `js/orchestrator.ts`).
    pub alongside: bool,
}

impl TaskContext<'_> {
    pub fn canceled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Which step this is, for events.
    pub fn at(&self) -> StepAt {
        self.run.at(self.position)
    }

    /// Ask for `approval` as this step. A deferred approval is noted as the
    /// one the run waits for.
    pub fn approve(&self, approval: &Approval) -> Decision {
        let step = self.at().step;
        let ask = Ask {
            id: approval.question_id(Some(&step)),
            step: Some(step),
            stop: Some(Arc::clone(&self.cancel)),
        };
        let decision = self.run.interaction.ask(&ask, approval);
        if decision == Decision::Deferred {
            self.run.halt.wait_for(ask.id);
        }
        decision
    }

    pub fn progress(&self, processed: usize, total: usize) {
        self.run.events.emit(Event::StepProgress {
            at: self.at(),
            processed,
            total,
        });
    }

    pub fn log(&self, line: impl Into<String>) {
        self.run.events.emit(Event::StepLog {
            at: self.at(),
            line: line.into(),
        });
    }

    /// The files a task could not process, as step log lines: each path,
    /// then its error.
    pub fn log_failures<'f>(&self, failures: impl IntoIterator<Item = &'f (String, String)>) {
        for (path, error) in failures {
            self.log(path.clone());
            for line in error.lines() {
                self.log(format!("  {line}"));
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct TaskOutcome {
    pub result: Value,
    /// Set when some of the task's files failed: how many, as a message. The
    /// step is then recorded as partial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    /// For a task that works on files: how many failed, of how many.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<FileCount>,
}

impl TaskOutcome {
    pub fn new(result: Value) -> Self {
        Self {
            result,
            partial: None,
            files: None,
        }
    }

    /// Count the task's files, and mark the outcome partial when some of the
    /// `total` files failed.
    pub fn with_failures(mut self, failed: usize, total: usize) -> Self {
        self.files = Some(FileCount { failed, total });
        if failed > 0 {
            self.partial = Some(failed_files(failed, total));
        }
        self
    }
}

/// "2 of 10 files failed", for a partial step.
pub(crate) fn failed_files(failed: usize, total: usize) -> String {
    let files = if total == 1 { "file" } else { "files" };
    format!("{failed} of {total} {files} failed")
}

/// Why a task did not produce an outcome.
#[derive(Debug)]
pub(crate) enum TaskError {
    Failed(String),
    /// A manual approval nobody could give now.
    AwaitingApproval(String),
    /// A manual approval was rejected: the run ends.
    Rejected(String),
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
            Self::Rejected(message) => ("RejectedError", message.as_str()),
            Self::Canceled => ("CanceledError", "the run was canceled"),
        };
        serde_json::json!({ "name": name, "message": message }).to_string()
    }
}

pub(crate) type TaskResult = Result<TaskOutcome, TaskError>;

pub(crate) fn dispatch(task: &TaskContext<'_>, content: Value) -> TaskResult {
    if task.canceled() {
        return Err(TaskError::Canceled);
    }
    let kind = content
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let experimental = match kind.as_str() {
        "http" => Some("http()"),
        "agent" => Some("agent()"),
        "installSkill" => Some("installSkill()"),
        "sleep" => Some("sleep()"),
        "waitForEvent" => Some("waitForEvent()"),
        "shard" if !task.run.v1_compat() => Some("shard()"),
        "dependencyGraph" => Some("codemod/deps.dependencyGraph"),
        "branch" => Some("codemod/git.branch"),
        "commit" => Some("codemod/git.commit"),
        "pullRequest" => Some("codemod/git.pullRequest"),
        _ => None,
    };
    if let Some(api) = experimental {
        task.run.experimental(api);
    }
    // Steps that change the target outside the file commits `before_write`
    // already guards. Git steps only touch git with `--managed-git`.
    let git_step = matches!(kind.as_str(), "branch" | "commit" | "pullRequest");
    if matches!(kind.as_str(), "shell" | "agent" | "installSkill")
        || (git_step && task.run.services.managed_git)
    {
        task.run.before_write().map_err(TaskError::Failed)?;
    }
    let result = match kind.as_str() {
        "forEachFile" => for_each_file::run(task, content).map_err(TaskError::from),
        "function" => function::run(task, content).map_err(TaskError::from),
        "dependencyGraph" => dependency_graph::run(task, content).map_err(TaskError::from),
        "shell" => shell::run(task, parse(content)?),
        "http" => http::run(task, parse(content)?),
        "astGrep" => ast_grep::run(task, parse(content)?),
        "shard" => shard::run(task, content),
        "codemod" => codemod::run(task, parse(content)?),
        "agent" => agent::run(task, parse(content)?),
        "installSkill" => install_skill::run(task, content),
        "approval" => approval::run(task, parse(content)?),
        "sleep" => wait::sleep(task, parse(content)?),
        "waitForEvent" => wait::for_event(task, parse(content)?),
        "branch" => git::branch(task, parse(content)?),
        "commit" => git::commit(task, parse(content)?),
        "pullRequest" => git::pull_request(task, parse(content)?),
        other => Err(TaskError::Failed(format!("unknown task kind '{other}'"))),
    };
    // A step stopped mid-way was canceled. An approval left unanswered
    // because the run was paused still waits: the paused run keeps its
    // question, and resuming it asks again (or `--approve` gives it).
    let still_waiting = |error: &TaskError| {
        matches!(error, TaskError::AwaitingApproval(_))
            && task.run.halt.reason() <= crate::run::StopReason::Pause
    };
    if task.canceled()
        && matches!(result, Err(ref error) if !matches!(error, TaskError::Rejected(_)) && !still_waiting(error))
    {
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
