//! Events the engine reports while a run progresses. The engine never writes
//! to the terminal; the CLI decides how to render these (text, JSONL, ...).

use std::path::PathBuf;
use std::sync::Mutex;

use butterflow_state::run_store::{RunStatus, StepRecord, StepStatus};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        run_id: String,
        workflow: PathBuf,
        resumed: bool,
    },
    StepStarted {
        position: u64,
        label: String,
    },
    /// A finished step whose recorded result was reused.
    StepReplayed {
        position: u64,
        label: String,
    },
    StepProgress {
        position: u64,
        processed: usize,
        total: usize,
    },
    StepLog {
        position: u64,
        line: String,
    },
    /// `console` output from the workflow body.
    RunLog {
        line: String,
    },
    /// What the coding agent running an `agent(...)` step did.
    StepAgent {
        position: u64,
        /// The agent's id (`"claude-code"`, `"codex"`, ...).
        agent: String,
        activity: codemod_agents::AgentEvent,
    },
    /// A step failed and runs again, as its `retries` allow.
    StepRetrying {
        position: u64,
        label: String,
        /// The attempt that failed, counting from 1.
        attempt: u32,
        /// How many attempts the step has in all.
        attempts: u32,
        error: String,
    },
    StepFinished {
        position: u64,
        label: String,
        status: StepStatus,
        error: Option<String>,
        duration_ms: u64,
    },
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
        /// How many files failed in the run's partial steps, counting steps a
        /// resume reused. A run that succeeded with failed files did not
        /// change those files.
        failed_files: usize,
        /// The steps some files failed in, by position.
        partial_steps: Vec<PartialStep>,
    },
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
