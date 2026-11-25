//! Runs whose workflow body runs outside the engine: another JavaScript host
//! (Node.js or Bun, through the `codemod` npm package) runs the body, function
//! steps and `forEachFile` callbacks with the shared orchestrator
//! (`js/orchestrator.ts`), and calls back here for what the engine owns: the
//! run store, events, host tasks, and selecting and committing files.
//!
//! There is no capability sandbox in such a host: the code it runs has
//! whatever the host gives it. Host tasks (`shard`, `codemod`, ...) still run
//! in the engine as usual.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use butterflow_state::run_store::StepRecord;
use serde_json::{json, Value};

use crate::engine::{Engine, EngineError, Execution, RunReport};
use crate::events::Event;
use crate::run::{run_host_task, Ending, RunContext, TestPlan};
use crate::tasks::for_each_file::{self, FileEffects, Selector};
use crate::tasks::TaskContext;

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
    /// The functions the build extracted for the engine to run in isolates
    /// (`shard`'s function and filter), by id; see [`crate::Built::callbacks`].
    pub callbacks: std::collections::BTreeMap<String, String>,
}

/// The host of runs opened here, as recorded in a run's settings.
const NODE_HOST: &str = "node";

/// A run in progress whose body another host runs. Every method may be
/// called from any thread.
pub struct HostedRun {
    context: Arc<RunContext>,
    /// The execution while the run is open; `None` once it has finished.
    execution: Mutex<Option<Execution>>,
    history: Vec<StepRecord>,
}

impl Engine {
    /// Start a run whose body the caller runs.
    pub fn open(&self, request: OpenRequest) -> Result<HostedRun, EngineError> {
        let execution = self.begin_new(
            None,
            &request.workflow,
            &request.target,
            request.params,
            request.name,
            request.callbacks,
            request.dry_run,
            request.test,
            // `runWorkflow` runs on a target the test or script chose.
            false,
            NODE_HOST,
        )?;
        Ok(HostedRun::new(execution, Vec::new()))
    }

    /// Continue a run whose body the caller runs again.
    pub fn open_resumed(&self, run_id: &str) -> Result<HostedRun, EngineError> {
        let record = self
            .store()
            .get_run(run_id)?
            .ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
        // The workflow is built again, for the functions the engine runs.
        let callbacks = crate::engine::load_workflow(&record.workflow_path)
            .map(|loaded| loaded.built.callbacks)
            .unwrap_or_default();
        let execution = self.begin_resumed(record, callbacks, NODE_HOST, false)?;
        let history = execution.context.store.load_steps(run_id)?;
        Ok(HostedRun::new(execution, history))
    }
}

impl HostedRun {
    fn new(execution: Execution, history: Vec<StepRecord>) -> Self {
        Self {
            context: Arc::clone(&execution.context),
            execution: Mutex::new(Some(execution)),
            history,
        }
    }

    /// Record how the run ended, given how its body ended; `None` when the
    /// body cannot end any more, because the steps it waits for will never
    /// start. A step that was canceled or waits for an approval decides the
    /// ending whatever the body did. A run finishes once; if its record
    /// cannot be saved it stays open, so finishing can be tried again.
    pub fn finish(&self, body: Option<Ending>) -> Result<RunReport, EngineError> {
        let mut slot = self.execution.lock().unwrap_or_else(|e| e.into_inner());
        let mut execution = slot.take().ok_or_else(|| self.finished())?;
        let body = body.unwrap_or_else(|| crate::run::stalled(&self.context));
        let ending = self.context.ending(body);
        match execution.save_ending(&ending) {
            Ok(steps) => Ok(RunReport {
                run: execution.record.clone(),
                ending,
                steps,
            }),
            Err(error) => {
                *slot = Some(execution);
                Err(error)
            }
        }
    }

    fn finished(&self) -> EngineError {
        EngineError::AlreadyFinished(self.context.run_id.clone())
    }

    /// Fails once the run has finished: nothing more can be done in it.
    fn open(&self) -> Result<(), EngineError> {
        let open = self
            .execution
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if open {
            Ok(())
        } else {
            Err(self.finished())
        }
    }

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

    /// Persist one step's outcome and report it.
    pub fn record(&self, step: &StepRecord, duration_ms: u64) -> Result<(), EngineError> {
        self.open()?;
        Ok(self.context.record_step(step, duration_ms)?)
    }

    /// Report a step started or replayed: `{ type, position, label }`.
    pub fn event(&self, event: &str) -> Result<(), String> {
        self.context.host_event(event)
    }

    /// A line the body or a step wrote; `position` is the step's.
    pub fn log(&self, position: Option<u64>, line: String) {
        self.context.events.emit(match position {
            Some(position) => Event::StepLog {
                at: self.context.at(position),
                line,
            },
            None => Event::RunLog { line, step: None },
        });
    }

    pub fn canceled(&self) -> bool {
        self.context.canceled()
    }

    /// Whether a step about to start must not, because the run was canceled.
    pub fn refuses_steps(&self) -> bool {
        self.context.refuses_steps()
    }

    /// Cancel this run.
    pub fn cancel(&self) {
        self.context.halt.stop(crate::run::StopReason::Cancel);
    }

    /// The test plan's `until` step has started.
    pub fn stop(&self) {
        self.context.stopped.store(true, Ordering::SeqCst);
    }

    /// Evaluate a workflow.yaml `${{ }}` expression; returns JSON.
    pub fn evaluate(&self, kind: &str, expression: &str, scope: &str) -> Result<String, String> {
        crate::expr::evaluate(&self.context, kind, expression, scope)
    }

    /// Run a host task (`{ position, label, task }`, as the
    /// orchestrator sends it). Blocks until it finishes. Returns the outcome
    /// as JSON, or the error payload (`{ name, message }`) as JSON.
    pub fn run_task(&self, request: &str) -> Result<String, String> {
        self.open().map_err(|e| e.to_string())?;
        // A task that panics still settles its step, as a failure.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_host_task(&self.context, request)
        }))
        .unwrap_or_else(|panic| Err(crate::run::panic_payload(panic)))
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
        self.open().map_err(|e| e.to_string())?;
        let files: Vec<FileEffects> =
            serde_json::from_value(files).map_err(|e| format!("bad file effects: {e}"))?;
        let task = TaskContext {
            run: &self.context,
            position,
            label,
            cancel: Arc::clone(&self.context.cancel),
            since: None,
            alongside: false,
        };
        let (changes, failures) = for_each_file::commit_effects(&task, files)?;
        let mut committed = json!({ "changes": changes, "failures": failures });
        // What the step's `assess()` calls used, as the CLI reports it.
        if let Some(usage) = self.context.assess.take(position) {
            task.log(usage.describe());
            committed["assessed"] = json!(usage);
        }
        self.context.assess.finish_step(position);
        Ok(committed)
    }

    /// Answer an `assess()` call of the step at `position` (the request as
    /// `codemod/assess` sends it). Blocks until the model answers. Returns
    /// `{ ok: true, value }` or `{ ok: false, error }` as JSON.
    pub fn assess(&self, position: u64, request: &str) -> String {
        if let Err(error) = self.open() {
            return json!({ "ok": false, "error": error.to_string() }).to_string();
        }
        match crate::assess::ask(&self.context, position, &self.context.cancel, request) {
            Ok(value) => format!(r#"{{"ok":true,"value":{value}}}"#),
            Err(error) => json!({ "ok": false, "error": error }).to_string(),
        }
    }

    /// Report a step's progress through its files.
    pub fn progress(&self, position: u64, processed: usize, total: usize) {
        self.context.events.emit(Event::StepProgress {
            at: self.context.at(position),
            processed,
            total,
        });
    }
}
