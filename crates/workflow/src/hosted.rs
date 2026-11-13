//! Runs whose workflow body runs outside the engine: another JavaScript host
//! (Node.js or Bun, through the `codemod` npm package) runs the body, function
//! steps and `forEachFile` callbacks with the shared orchestrator
//! (`js/orchestrator.js`), and calls back here for what the engine owns: the
//! run store, events, host tasks, and selecting and committing files.
//!
//! There is no capability sandbox in such a host: the code it runs has
//! whatever the host gives it. Host tasks (`jssg`, `codemod`, ...) still run
//! in the engine as usual.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use butterflow_state::run_store::{RunRecord, StepRecord, StoreError};
use serde_json::{json, Value};

use crate::engine::{Engine, EngineError, RunReport};
use crate::events::Event;
use crate::run::{run_host_task, Ending, HostEvent, RunContext, TestPlan};
use crate::tasks::for_each_file::{self, FileEffects, Selector};
use crate::tasks::TaskContext;
use crate::Summary;

/// The language a `forEachFile` step's files are in, when known, and the
/// files as `(path, text)` pairs.
pub type SelectedFiles = (Option<String>, Vec<(String, String)>);

pub struct OpenRequest {
    /// The workflow file; relative task paths resolve against its directory.
    pub workflow: PathBuf,
    pub target: PathBuf,
    pub params: Value,
    pub name: Option<String>,
    pub dry_run: bool,
    pub test: Option<TestPlan>,
}

/// A run in progress whose body another host runs. Every method may be
/// called from any thread.
pub struct HostedRun {
    context: Arc<RunContext>,
    record: Mutex<Option<RunRecord>>,
    history: Vec<StepRecord>,
}

impl Engine {
    /// Start a run whose body the caller runs.
    pub fn open(&self, request: OpenRequest) -> Result<HostedRun, EngineError> {
        let record = Self::new_record(
            &request.workflow,
            &request.target,
            request.params,
            request.name,
        );
        let store = self.store_for(request.dry_run, request.test.is_some());
        store.create_run(&record)?;
        let context = self.begin(
            store,
            &record,
            Default::default(),
            request.dry_run,
            request.test,
            false,
        );
        Ok(HostedRun {
            context,
            record: Mutex::new(Some(record)),
            history: Vec::new(),
        })
    }

    /// Continue a run whose body the caller runs again.
    pub fn open_resumed(&self, run_id: &str) -> Result<HostedRun, EngineError> {
        let record = self.reopen(run_id)?;
        let context = self.begin(
            Arc::clone(self.store()),
            &record,
            Default::default(),
            false,
            None,
            true,
        );
        let history = context.store.load_steps(&record.id)?;
        Ok(HostedRun {
            context,
            record: Mutex::new(Some(record)),
            history,
        })
    }

    /// Record how a hosted run ended. A run can only finish once.
    pub fn finish(&self, run: &HostedRun, ending: Ending) -> Result<RunReport, EngineError> {
        let ending = match (&ending, run.stopped_at()) {
            (Ending::Succeeded(_) | Ending::Failed(_), Some(label)) => Ending::Stopped(label),
            _ => ending,
        };
        let record = run
            .record
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or_else(|| EngineError::AlreadyFinished(run.context.run_id.clone()))?;
        self.conclude(&run.context, record, ending)
    }
}

impl HostedRun {
    pub fn run_id(&self) -> &str {
        &self.context.run_id
    }

    pub fn target(&self) -> &Path {
        &self.context.target
    }

    pub fn workflow_dir(&self) -> &Path {
        &self.context.bundle_dir
    }

    pub fn params(&self) -> &Value {
        &self.context.params
    }

    pub fn dry_run(&self) -> bool {
        self.context.dry_run
    }

    /// When the run started, in milliseconds since the epoch.
    pub fn started_at(&self) -> i64 {
        self.context.started_at
    }

    /// Steps recorded by earlier executions of this run.
    pub fn history(&self) -> &[StepRecord] {
        &self.history
    }

    pub fn test_plan(&self) -> Option<&TestPlan> {
        self.context.test.as_ref()
    }

    pub fn summary(&self) -> &Summary {
        &self.context.services.summary
    }

    /// Persist one step's outcome and report it.
    pub fn record(&self, step: &StepRecord, duration_ms: u64) -> Result<(), StoreError> {
        self.context.store.put_step(&self.context.run_id, step)?;
        self.context.events.emit(Event::StepFinished {
            position: step.position,
            label: step.label.clone(),
            status: step.status,
            error: step.error.clone(),
            duration_ms,
        });
        Ok(())
    }

    /// Report a step started or replayed: `{ type, position, label }`.
    pub fn event(&self, event: &str) {
        match serde_json::from_str::<HostEvent>(event) {
            Ok(HostEvent::StepStarted { position, label }) => self
                .context
                .events
                .emit(Event::StepStarted { position, label }),
            Ok(HostEvent::StepReplayed { position, label }) => self
                .context
                .events
                .emit(Event::StepReplayed { position, label }),
            Err(_) => {}
        }
    }

    /// A line the body or a step wrote; `position` is the step's.
    pub fn log(&self, position: Option<u64>, line: String) {
        self.context.events.emit(match position {
            Some(position) => Event::StepLog { position, line },
            None => Event::RunLog { line },
        });
    }

    pub fn canceled(&self) -> bool {
        self.context.canceled()
    }

    pub fn cancel(&self) {
        self.context.cancel.store(true, Ordering::SeqCst);
    }

    /// The test plan's `until` step has started.
    pub fn stop(&self) {
        self.context.stopped.store(true, Ordering::SeqCst);
    }

    fn stopped_at(&self) -> Option<String> {
        if !self.context.stopped.load(Ordering::SeqCst) {
            return None;
        }
        self.context
            .test
            .as_ref()
            .and_then(|plan| plan.until.clone())
    }

    /// Evaluate a workflow.yaml `${{ }}` expression; returns JSON.
    pub fn evaluate(&self, kind: &str, expression: &str, scope: &str) -> Result<String, String> {
        crate::expr::evaluate(&self.context, kind, expression, scope)
    }

    /// Run a host task (`{ position, label, task, state }`, as the
    /// orchestrator sends it). Blocks until it finishes. Returns the outcome
    /// as JSON, or the error payload (`{ name, message }`) as JSON.
    pub fn run_task(&self, request: &str) -> Result<String, String> {
        run_host_task(&self.context, request)
    }

    /// The files a `forEachFile` selector picks, as `[path, text]` pairs, and
    /// the language the callback's files are in when the selector or the
    /// package names exactly one.
    pub fn select_files(&self, selector: Value) -> Result<SelectedFiles, String> {
        let selector: Selector =
            serde_json::from_value(selector).map_err(|e| format!("bad selector: {e}"))?;
        let (selector, files) = for_each_file::select_files(&self.context, selector)?;
        Ok((selector.language, files))
    }

    /// Commit what a `forEachFile` step's callbacks did to its files:
    /// `[{ path, original, effects: [{ op: "write", text } | { op: "rename", to } | { op: "delete" }] }]`.
    /// Returns `{ changes, failures }`; files whose effects are invalid are
    /// failures.
    pub fn commit_files(
        &self,
        position: u64,
        label: String,
        files: Value,
    ) -> Result<Value, String> {
        let files: Vec<FileEffects> =
            serde_json::from_value(files).map_err(|e| format!("bad file effects: {e}"))?;
        let task = TaskContext {
            run: &self.context,
            position,
            label,
            state: Default::default(),
        };
        let (changes, failures) = for_each_file::commit_effects(&task, files)?;
        Ok(json!({ "changes": changes, "failures": failures }))
    }

    /// Report a step's progress through its files.
    pub fn progress(&self, position: u64, processed: usize, total: usize) {
        self.context.events.emit(Event::StepProgress {
            position,
            processed,
            total,
        });
    }

    /// Add metrics a step recorded to the run summary.
    pub fn add_metrics(&self, metrics: Value) -> Result<(), String> {
        let metrics = serde_json::from_value(metrics).map_err(|e| format!("bad metrics: {e}"))?;
        self.context.services.summary.merge_metrics(metrics);
        Ok(())
    }
}
