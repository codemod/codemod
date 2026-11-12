//! Events the engine reports while a run progresses. The engine never writes
//! to the terminal; the CLI decides how to render these (text, JSONL, ...).

use std::path::PathBuf;
use std::sync::Mutex;

use butterflow_state::run_store::{RunStatus, StepStatus};
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
    StepFinished {
        position: u64,
        label: String,
        status: StepStatus,
        error: Option<String>,
        duration_ms: u64,
    },
    RunFinished {
        run_id: String,
        status: RunStatus,
        error: Option<String>,
    },
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
