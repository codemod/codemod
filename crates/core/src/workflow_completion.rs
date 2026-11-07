//! In-process completion signals for workflow executors.
//!
//! The engine drives each workflow run on a dedicated thread
//! (`Engine::spawn_workflow_executor`). Callers in the same process (the CLI's
//! `workflow run` / `workflow resume` wait loops) used to learn about the end
//! of a run only by polling the state adapter on a fixed sleep. This module
//! lets them wake as soon as an executor exits instead, while the state
//! adapter stays the source of truth for the run's status.
//!
//! A run can have several executor episodes (start, then one per resume or
//! trigger), so the signal tracks a monotonically increasing exit count
//! rather than a one-shot flag.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use uuid::Uuid;

/// Executor activity for one workflow run, as seen by this process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkflowExecutorActivity {
    /// Executors for this run currently running in this process.
    pub active: usize,
    /// Number of executors for this run that have exited, with any outcome.
    pub exits: u64,
    /// Error from the most recently exited executor, if it failed or panicked.
    pub last_error: Option<String>,
}

impl WorkflowExecutorActivity {
    /// True when no executor is running and the last one to exit failed.
    pub fn failed_without_executor(&self) -> bool {
        self.active == 0 && self.exits > 0 && self.last_error.is_some()
    }
}

/// Per-run executor activity channels, shared by every clone of an engine.
#[derive(Clone, Default)]
pub(crate) struct WorkflowExecutorRegistry {
    runs: Arc<Mutex<HashMap<Uuid, Arc<watch::Sender<WorkflowExecutorActivity>>>>>,
}

impl WorkflowExecutorRegistry {
    fn sender(&self, workflow_run_id: Uuid) -> Arc<watch::Sender<WorkflowExecutorActivity>> {
        let mut runs = self.runs.lock().unwrap_or_else(|error| error.into_inner());
        Arc::clone(
            runs.entry(workflow_run_id)
                .or_insert_with(|| Arc::new(watch::channel(Default::default()).0)),
        )
    }

    /// Record that an executor for `workflow_run_id` is starting. The returned
    /// guard reports the exit when dropped, including during a panic unwind.
    pub(crate) fn begin(&self, workflow_run_id: Uuid) -> WorkflowExecutorGuard {
        let sender = self.sender(workflow_run_id);
        sender.send_modify(|activity| activity.active += 1);
        WorkflowExecutorGuard {
            sender,
            error: None,
        }
    }

    /// Subscribe to executor exits for a run. `None` when no executor for the
    /// run was ever started in this process (e.g. the run is driven by another
    /// process through shared state), in which case callers must poll.
    pub(crate) fn subscribe(&self, workflow_run_id: Uuid) -> Option<WorkflowCompletionSignal> {
        let runs = self.runs.lock().unwrap_or_else(|error| error.into_inner());
        let receiver = runs.get(&workflow_run_id)?.subscribe();
        let seen_exits = receiver.borrow().exits;
        Some(WorkflowCompletionSignal {
            receiver,
            seen_exits,
        })
    }
}

/// Reports an executor exit to subscribers when dropped.
pub(crate) struct WorkflowExecutorGuard {
    sender: Arc<watch::Sender<WorkflowExecutorActivity>>,
    error: Option<String>,
}

impl WorkflowExecutorGuard {
    pub(crate) fn fail(&mut self, error: String) {
        self.error = Some(error);
    }
}

impl Drop for WorkflowExecutorGuard {
    fn drop(&mut self) {
        let error = self.error.take().or_else(|| {
            std::thread::panicking().then(|| "Workflow executor panicked".to_string())
        });
        self.sender.send_modify(|activity| {
            activity.active = activity.active.saturating_sub(1);
            activity.exits += 1;
            activity.last_error = error;
        });
    }
}

/// Wakes when a workflow executor for one run exits in this process.
///
/// This is a wake-up hint only: after it fires, re-read the run status from
/// the state adapter, which remains authoritative.
pub struct WorkflowCompletionSignal {
    receiver: watch::Receiver<WorkflowExecutorActivity>,
    seen_exits: u64,
}

impl WorkflowCompletionSignal {
    /// Current executor activity for the run.
    pub fn activity(&self) -> WorkflowExecutorActivity {
        self.receiver.borrow().clone()
    }

    /// Resolve once an executor exit happens that this signal has not reported
    /// yet. Exits that happened after subscribing but before this call resolve
    /// immediately; each exit is reported once. Never resolves if no further
    /// executor exits, so pair it with a fallback timer.
    pub async fn executor_exited(&mut self) -> WorkflowExecutorActivity {
        let seen = self.seen_exits;
        match self.receiver.wait_for(|activity| activity.exits > seen).await {
            Ok(activity) => {
                let activity = activity.clone();
                self.seen_exits = activity.exits;
                activity
            }
            // The engine that owned the channel is gone; nothing will fire.
            Err(_) => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn subscribe_is_none_for_unknown_run() {
        let registry = WorkflowExecutorRegistry::default();
        assert!(registry.subscribe(Uuid::new_v4()).is_none());
    }

    #[tokio::test]
    async fn fires_once_per_exit_and_reports_errors() {
        let registry = WorkflowExecutorRegistry::default();
        let run_id = Uuid::new_v4();

        let guard = registry.begin(run_id);
        let mut signal = registry.subscribe(run_id).expect("run was started here");
        assert_eq!(signal.activity().active, 1);

        drop(guard);
        let activity = tokio::time::timeout(Duration::from_secs(1), signal.executor_exited())
            .await
            .expect("exit should be reported");
        assert_eq!(activity.active, 0);
        assert_eq!(activity.exits, 1);
        assert!(!activity.failed_without_executor());

        // Already reported: must not fire again until another executor exits.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), signal.executor_exited())
                .await
                .is_err()
        );

        let mut guard = registry.begin(run_id);
        guard.fail("boom".to_string());
        drop(guard);
        let activity = tokio::time::timeout(Duration::from_secs(1), signal.executor_exited())
            .await
            .expect("second exit should be reported");
        assert_eq!(activity.exits, 2);
        assert_eq!(activity.last_error.as_deref(), Some("boom"));
        assert!(activity.failed_without_executor());
    }

    #[tokio::test]
    async fn records_panics_as_errors() {
        let registry = WorkflowExecutorRegistry::default();
        let run_id = Uuid::new_v4();
        let guard = registry.begin(run_id);
        let mut signal = registry.subscribe(run_id).unwrap();

        let handle = std::thread::spawn(move || {
            let _guard = guard;
            panic!("executor blew up");
        });
        assert!(handle.join().is_err());

        let activity = tokio::time::timeout(Duration::from_secs(1), signal.executor_exited())
            .await
            .expect("panic exit should be reported");
        assert_eq!(
            activity.last_error.as_deref(),
            Some("Workflow executor panicked")
        );
    }
}
