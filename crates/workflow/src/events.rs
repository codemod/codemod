//! Events the engine reports while a run progresses. The engine never writes
//! to the terminal; the CLI decides how to render these (text, JSONL, ...).
//!
//! Serialized, these are the events of the CLI's `--format jsonl` output and
//! of a run's journal; `crates/cli/npm/types/protocol.d.ts` declares them for
//! TypeScript, and a test keeps the two in step.

use std::path::PathBuf;
use std::sync::Mutex;

use butterflow_state::run_store::{RunSettings, RunStatus, StepRecord, StepStatus};
use serde::Serialize;

use crate::interaction::{AgentChoice, Approval};

/// Which step an event is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepAt {
    /// The step's place in its workflow's list of steps.
    pub position: u64,
    /// The step's key, stable across resumes: its position, after the key of
    /// the step it is nested in for a step of a nested codemod (`"3"`,
    /// `"3/0"`).
    pub step: String,
    /// The key of the step whose nested codemod this step belongs to; `null`
    /// for the run's own steps.
    pub parent: Option<String>,
}

impl StepAt {
    /// One of the run's own steps.
    pub fn top(position: u64) -> Self {
        Self {
            position,
            step: position.to_string(),
            parent: None,
        }
    }

    /// The step of the run's own that this one is, or is nested in.
    pub fn root(&self) -> Self {
        Self::top(root_position(&self.step).unwrap_or(self.position))
    }
}

/// The position of the run's own step a step key is, or is under.
fn root_position(key: &str) -> Option<u64> {
    key.split('/').next()?.parse().ok()
}

/// The settings a run was started with, as `run_started` echoes them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub capabilities: Vec<String>,
    pub managed_git: bool,
    pub agent: Option<String>,
    /// What runs the workflow's JavaScript: `"quickjs"` (the CLI), `"node"`.
    pub host: Option<String>,
}

impl From<&RunSettings> for Settings {
    fn from(settings: &RunSettings) -> Self {
        Self {
            capabilities: settings.capabilities.clone(),
            managed_git: settings.managed_git,
            agent: settings.agent.clone(),
            host: settings.host.clone(),
        }
    }
}

/// What a question asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// Yes or no: `approve`, `reject`, or (manual approvals) `later`.
    Approval,
    /// A coding agent for an `agent(...)` step, or `null` for the built-in one.
    Agent,
}

/// What closed a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedBy {
    /// A command answered it.
    Answer,
    /// The run was asked to stop.
    Stop,
    /// The controlling process closed stdin.
    Eof,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        run_id: String,
        workflow: PathBuf,
        resumed: bool,
        target: PathBuf,
        params: serde_json::Value,
        dry_run: bool,
        settings: Settings,
    },
    StepStarted {
        #[serde(flatten)]
        at: StepAt,
        label: String,
        /// The step's task: `forEachFile`, `function`, `approval`, `shell`,
        /// `agent`, `codemod`, ...
        kind: Option<String>,
    },
    /// A finished step whose recorded result was reused.
    StepReplayed {
        #[serde(flatten)]
        at: StepAt,
        label: String,
        kind: Option<String>,
    },
    StepProgress {
        #[serde(flatten)]
        at: StepAt,
        processed: usize,
        total: usize,
    },
    StepLog {
        #[serde(flatten)]
        at: StepAt,
        line: String,
    },
    /// `console` output from a workflow body: the run's own (`step` is
    /// `null`), or a nested codemod's (`step` is the step running it).
    RunLog { line: String, step: Option<String> },
    /// What the coding agent running an `agent(...)` step did.
    StepAgent {
        #[serde(flatten)]
        at: StepAt,
        /// The agent's id (`"claude-code"`, `"codex"`, ...).
        agent: String,
        activity: codemod_agents::AgentEvent,
    },
    /// A step failed and runs again, as its `retries` allow.
    StepRetrying {
        #[serde(flatten)]
        at: StepAt,
        label: String,
        /// The attempt that failed, counting from 1.
        attempt: u32,
        /// How many attempts the step has in all.
        attempts: u32,
        error: String,
    },
    StepFinished {
        #[serde(flatten)]
        at: StepAt,
        label: String,
        kind: Option<String>,
        status: StepStatus,
        error: Option<String>,
        duration_ms: u64,
    },
    /// One file a step of a real run changed, as a unified diff: reported
    /// when the step writes it, before `StepFinished`. A renamed file is its
    /// old path removed and its new one added. Dry runs report `RunDiff`
    /// instead.
    StepDiff {
        #[serde(flatten)]
        at: StepAt,
        path: String,
        /// Empty when it is over [`STEP_DIFF_LIMIT`] (`diff_omitted` says so).
        diff: String,
        additions: usize,
        deletions: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        diff_omitted: Option<Omitted>,
    },
    /// The run waits for an answer (only with a control channel, which the
    /// CLI owns): `answer`, `approve` or `reject` it by `id`.
    Question {
        /// Stable across resumes: `approval:<step>`, `shell:<step>`,
        /// `pull-request:<step>`, `capabilities:<step>`, `agent:<step>`, or
        /// `uncommitted-changes`.
        id: String,
        kind: QuestionKind,
        /// The key of the step asking; `null` when no step does.
        step: Option<String>,
        /// The question, to show as it is.
        message: String,
        /// For approvals: what is to be approved.
        #[serde(skip_serializing_if = "Option::is_none")]
        approval: Option<Approval>,
        /// For agent questions: the agents to pick from.
        #[serde(skip_serializing_if = "Option::is_none")]
        choices: Option<Vec<AgentChoice>>,
    },
    /// A question was answered, or closed unanswered.
    QuestionClosed {
        id: String,
        /// `"approved"`, `"rejected"` or `"later"` for an approval; the
        /// agent's id, or `null` for the built-in one, for an agent question.
        answer: serde_json::Value,
        by: ClosedBy,
    },
    /// A command on the control channel could not be followed.
    ControlError { message: String, line: String },
    /// What a dry run would have changed in one file, all its steps
    /// together, as a unified diff: reported when the run ends, before
    /// `RunFinished`, one event per file. A renamed file is its old path
    /// removed and its new one added.
    RunDiff {
        path: String,
        diff: String,
        additions: usize,
        deletions: usize,
    },
    RunFinished {
        run_id: String,
        status: RunStatus,
        error: Option<String>,
        /// For a run waiting for an approval: what it asks, and the
        /// question's id (`workflow resume --approve <id>`).
        awaiting: Option<String>,
        question: Option<String>,
        /// How many files failed in the run's partial steps, counting steps a
        /// resume reused. A run that succeeded with failed files did not
        /// change those files.
        failed_files: usize,
        /// The steps some files failed in, by position.
        partial_steps: Vec<PartialStep>,
        /// What the workflow returned, or gave `fail()`; `null` otherwise,
        /// and when it is over [`OUTPUT_LIMIT`] as JSON (`output_omitted`
        /// says so). Dry runs report it too.
        output: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_omitted: Option<Omitted>,
        /// Every recorded step with its result, by position. Results share
        /// [`STEP_RESULTS_LIMIT`]: one that does not fit is `null`, with
        /// `result_omitted`.
        steps: Vec<StepResult>,
    },
}

impl Event {
    /// The event as a view of the run's own steps shows it, the way the
    /// text output and the launcher do: what a nested codemod's steps do
    /// becomes log lines, progress, agent activity and diffs of the run's
    /// step running it. `None`: nothing to show.
    pub fn top_level(self) -> Option<Event> {
        let nested = |at: &StepAt| at.parent.is_some();
        let line = |at: &StepAt, line: String| {
            Some(Event::StepLog {
                at: at.root(),
                line,
            })
        };
        match self {
            Event::StepStarted { at, label, .. } if nested(&at) => line(&at, format!("▸ {label}")),
            Event::StepReplayed { at, label, .. } if nested(&at) => line(&at, format!("↺ {label}")),
            Event::StepRetrying {
                at,
                label,
                attempt,
                attempts,
                error,
            } if nested(&at) => line(
                &at,
                format!("↻ {label}: attempt {attempt} of {attempts} failed: {error}"),
            ),
            Event::StepFinished {
                at,
                label,
                status,
                error,
                ..
            } if nested(&at) => {
                let text = match (status, error) {
                    (StepStatus::Succeeded, _) => format!("✓ {label}"),
                    (StepStatus::Partial, Some(error)) => format!("◐ {label}: {error}"),
                    (StepStatus::Partial, None) => format!("◐ {label}"),
                    (StepStatus::Failed, Some(error)) => format!("✗ {label}: {error}"),
                    (StepStatus::Failed, None) => format!("✗ {label}"),
                    (StepStatus::Canceled, _) => format!("■ {label}: canceled"),
                    (StepStatus::AwaitingApproval, _) => format!("‖ {label}: waiting for approval"),
                    (StepStatus::Rejected, _) => format!("✗ {label}: rejected"),
                    // Never reported: a step that began to wait has not finished.
                    (StepStatus::Waiting, _) => return None,
                };
                line(&at, text)
            }
            Event::StepLog { at, line: text } if nested(&at) => line(&at, format!("  {text}")),
            Event::RunLog {
                line: text,
                step: Some(step),
            } => Some(Event::StepLog {
                at: StepAt::top(root_position(&step).unwrap_or_default()),
                line: text,
            }),
            Event::StepProgress {
                at,
                processed,
                total,
            } if nested(&at) => Some(Event::StepProgress {
                at: at.root(),
                processed,
                total,
            }),
            Event::StepAgent {
                at,
                agent,
                activity,
            } if nested(&at) => Some(Event::StepAgent {
                at: at.root(),
                agent,
                activity,
            }),
            Event::StepDiff {
                at,
                path,
                diff,
                additions,
                deletions,
                diff_omitted,
            } if nested(&at) => Some(Event::StepDiff {
                at: at.root(),
                path,
                diff,
                additions,
                deletions,
                diff_omitted,
            }),
            event => Some(event),
        }
    }
}

/// The task a step runs (`forEachFile`, `approval`, ...), from its
/// recorded key: the canonical `[label, task]` JSON.
pub fn step_kind(key: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(key).ok()?;
    value.get(1)?.get("kind")?.as_str().map(str::to_string)
}

/// The most one `StepDiff` carries of a file's diff.
pub const STEP_DIFF_LIMIT: usize = 256 * 1024;

/// The most JSON `RunFinished.output` carries.
pub const OUTPUT_LIMIT: usize = 1024 * 1024;
/// The most JSON the step results in one `RunFinished` carry together.
pub const STEP_RESULTS_LIMIT: usize = 1024 * 1024;

/// A value left out of an event for its size.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Omitted {
    /// Its size as JSON.
    pub bytes: usize,
    /// What was left for it.
    pub limit: usize,
}

/// A step as `RunFinished` reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepResult {
    pub position: u64,
    pub label: String,
    pub status: StepStatus,
    pub result: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_omitted: Option<Omitted>,
}

/// `value` if it is at most `limit` bytes as JSON, else `null` and why.
pub fn bounded(value: &serde_json::Value, limit: usize) -> (serde_json::Value, Option<Omitted>) {
    let bytes = serde_json::to_string(value).map_or(0, |text| text.len());
    if bytes <= limit {
        (value.clone(), None)
    } else {
        (serde_json::Value::Null, Some(Omitted { bytes, limit }))
    }
}

impl StepResult {
    /// `steps` with their results, within [`STEP_RESULTS_LIMIT`] together.
    pub fn of(steps: &[StepRecord]) -> Vec<StepResult> {
        let mut left = STEP_RESULTS_LIMIT;
        steps
            .iter()
            .map(|step| {
                let (result, result_omitted) = bounded(
                    step.result.as_ref().unwrap_or(&serde_json::Value::Null),
                    left,
                );
                if result_omitted.is_none() {
                    left -= serde_json::to_string(&result).map_or(0, |text| text.len());
                }
                StepResult {
                    position: step.position,
                    label: step.label.clone(),
                    status: step.status,
                    result,
                    result_omitted,
                }
            })
            .collect()
    }
}

/// A step that finished with some of its files failed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PartialStep {
    pub position: u64,
    pub label: String,
    pub failed_files: usize,
    /// How many files the step ran on, when known.
    pub files: usize,
}

impl PartialStep {
    /// The partial steps among `steps`.
    pub fn of(steps: &[StepRecord]) -> Vec<PartialStep> {
        steps
            .iter()
            .filter(|step| step.status == StepStatus::Partial)
            .map(|step| PartialStep {
                position: step.position,
                label: step.label.clone(),
                failed_files: step.failed_files(),
                files: step.files.map_or(0, |files| files.total),
            })
            .collect()
    }
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

/// Discards every event.
pub struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _event: Event) {}
}

/// Keeps every event; for tests.
#[derive(Default)]
pub struct CollectEvents(pub Mutex<Vec<Event>>);

impl CollectEvents {
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl EventSink for CollectEvents {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(event);
    }
}
