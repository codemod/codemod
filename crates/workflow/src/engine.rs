//! The public entry point: start, resume and cancel runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use butterflow_state::run_store::{
    MemoryStore, RunLease, RunRecord, RunSettings, RunStatus, RunStore, StepRecord, StoreError,
};
use chrono::Utc;
use serde_json::{json, Value};

use crate::build::{build, BuildError, Built};
use crate::events::{Event, EventSink};
use crate::interaction::Interaction;
use crate::run::{execute, CancelWatch, Ending, RunContext, RunError, TestPlan};
use crate::Services;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("{0}")]
    Compat(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("run '{0}' not found")]
    NotFound(String),
    #[error("run '{0}' already finished; start a new run instead")]
    AlreadyFinished(String),
    #[error("run '{0}' is being executed by another process; wait for it, or stop it with `codemod workflow cancel --id {0}`")]
    Busy(String),
    #[error("run '{run}' was started {started}, and can only be resumed there: {resuming} records steps differently")]
    OtherHost {
        run: String,
        started: &'static str,
        resuming: &'static str,
    },
}

/// The host a run's JavaScript runs in, as people know it.
fn host_name(host: &str) -> &'static str {
    match host {
        QUICKJS_HOST => "by the codemod CLI",
        _ => "with runWorkflow in Node.js",
    }
}

/// The engine's own JavaScript runtime, as recorded in a run's settings.
pub(crate) const QUICKJS_HOST: &str = "quickjs";

/// A workflow ready to run.
#[derive(Debug, Clone)]
pub struct LoadedWorkflow {
    pub path: PathBuf,
    pub built: Built,
}

/// Read and build a workflow file: TypeScript or JavaScript (`.ts`, `.mts`,
/// `.js`, `.mjs`), or `.yaml`/`.yml`/`.json` through the compatibility layer.
pub fn load_workflow(path: &Path) -> Result<LoadedWorkflow, EngineError> {
    let source = std::fs::read_to_string(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let file = path.display().to_string();
    let typescript = if is_typescript(path) {
        source
    } else {
        crate::compat::translate_file(path)
            .map_err(EngineError::Compat)?
            .source
    };
    let built = build(&file, &typescript)?;
    crate::build::check_imports(path, &typescript, &built)?;
    Ok(LoadedWorkflow {
        path: path.to_path_buf(),
        built,
    })
}

/// Whether a workflow file is code (TypeScript or JavaScript), not YAML.
pub fn is_typescript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts" | "mts" | "js" | "mjs")
    )
}

pub struct StartRequest {
    pub workflow: LoadedWorkflow,
    pub target: PathBuf,
    pub params: Value,
    pub name: Option<String>,
    /// Run every step but change nothing on disk and persist nothing.
    pub dry_run: bool,
    /// Run as a test: steps may be stopped at or stubbed, and nothing is
    /// persisted (changes on disk are made, in the target the test set up).
    pub test: Option<TestPlan>,
}

/// What a run ended as.
#[derive(Debug, Clone)]
pub struct RunReport {
    pub run: RunRecord,
    pub ending: Ending,
    /// Every recorded step, ordered by position.
    pub steps: Vec<StepRecord>,
}

impl RunReport {
    /// The report as the `codemod/testing` helpers and the Node.js host
    /// return it.
    pub fn to_json(&self) -> Value {
        let (output, error, awaiting, stopped_at) = match &self.ending {
            Ending::Succeeded(output) => (output.clone(), None, None, None),
            Ending::Failed(error) => (Value::Null, Some(error), None, None),
            Ending::Canceled => (Value::Null, None, None, None),
            Ending::AwaitingApproval(message) => (Value::Null, None, Some(message), None),
            Ending::Stopped(label) => (Value::Null, None, None, Some(label)),
        };
        let steps: Vec<Value> = self
            .steps
            .iter()
            .map(|step| {
                json!({
                    "position": step.position,
                    "label": step.label,
                    "status": step.status,
                    "result": step.result,
                    "error": step.error,
                    "given": step.given,
                })
            })
            .collect();
        json!({
            "runId": self.run.id,
            "status": self.ending.status(),
            "output": output,
            "error": error,
            "awaiting": awaiting,
            "stoppedAt": stopped_at,
            "steps": steps,
        })
    }
}

/// Stops the runs an engine executes: the ones in progress, and any it
/// starts afterwards (a Ctrl-C while a run is still being set up).
#[derive(Clone, Default)]
pub struct StopHandle {
    inner: Arc<StopState>,
}

#[derive(Default)]
struct StopState {
    requested: AtomicBool,
    runs: Mutex<Vec<Arc<AtomicBool>>>,
}

impl StopHandle {
    pub fn stop(&self) {
        self.inner.requested.store(true, Ordering::SeqCst);
        for run in self.runs().iter() {
            run.store(true, Ordering::SeqCst);
        }
    }

    fn runs(&self) -> std::sync::MutexGuard<'_, Vec<Arc<AtomicBool>>> {
        self.inner.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A cancel flag for a run that starts now.
    fn enroll(&self) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(self.inner.requested.load(Ordering::SeqCst)));
        self.runs().push(Arc::clone(&cancel));
        cancel
    }

    fn release(&self, cancel: &Arc<AtomicBool>) {
        self.runs().retain(|run| !Arc::ptr_eq(run, cancel));
    }
}

pub struct Engine {
    store: Arc<dyn RunStore>,
    events: Arc<dyn EventSink>,
    interaction: Arc<dyn Interaction>,
    services: Arc<Services>,
    stop: StopHandle,
}

/// A run being executed: its record, and the hold on it in the store.
pub(crate) struct Execution {
    pub record: RunRecord,
    pub context: Arc<RunContext>,
    stop: StopHandle,
    _lease: Box<dyn RunLease>,
}

impl Execution {
    /// Record how the run ended, report it, and let go of the run.
    pub fn conclude(mut self, ending: Ending) -> Result<RunReport, EngineError> {
        let steps = self.save_ending(&ending)?;
        Ok(RunReport {
            run: self.record,
            ending,
            steps,
        })
    }

    /// Save how the run ended and report it. The execution stays with the
    /// caller, so a save that fails can be tried again.
    pub fn save_ending(&mut self, ending: &Ending) -> Result<Vec<StepRecord>, EngineError> {
        let context = &self.context;
        let mut record = self.record.clone();
        record.conclude(ending.outcome());
        context.store.put_run(&record)?;
        self.record = record;
        self.stop.release(&context.cancel);
        context.events.emit(Event::RunFinished {
            run_id: self.record.id.clone(),
            status: self.record.status,
            error: self.record.error.clone(),
        });
        Ok(context.store.load_steps(&self.record.id)?)
    }
}

impl Engine {
    pub fn new(
        store: Arc<dyn RunStore>,
        events: Arc<dyn EventSink>,
        interaction: Arc<dyn Interaction>,
        services: Services,
    ) -> Self {
        Self {
            store,
            events,
            interaction,
            services: Arc::new(services),
            stop: StopHandle::default(),
        }
    }

    pub fn store(&self) -> &Arc<dyn RunStore> {
        &self.store
    }

    /// Stops this engine's runs (Ctrl-C).
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    pub fn start(&self, request: StartRequest) -> Result<RunReport, EngineError> {
        let execution = self.begin_new(
            &request.workflow.path,
            &request.target,
            request.params,
            request.name,
            request.workflow.built.callbacks.clone(),
            request.dry_run,
            request.test,
            QUICKJS_HOST,
        )?;
        self.drive(execution, &request.workflow.built)
    }

    /// Continue a run that failed, was canceled or interrupted, or waits for
    /// an approval.
    pub fn resume(&self, run_id: &str) -> Result<RunReport, EngineError> {
        // Load the workflow first: one that no longer builds leaves the run
        // as it was.
        let record = self
            .store
            .get_run(run_id)?
            .ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
        let workflow = load_workflow(&record.workflow_path)?;
        let execution =
            self.begin_resumed(record, workflow.built.callbacks.clone(), QUICKJS_HOST)?;
        self.drive(execution, &workflow.built)
    }

    fn drive(&self, execution: Execution, built: &Built) -> Result<RunReport, EngineError> {
        let ending = execute(built, Arc::clone(&execution.context))
            // A run that could not execute still ends: as failed.
            .unwrap_or_else(|error| Ending::Failed(error.to_string()));
        execution.conclude(ending)
    }

    /// Create a run and take it for this engine.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_new(
        &self,
        workflow_path: &Path,
        target: &Path,
        params: Value,
        name: Option<String>,
        callbacks: std::collections::BTreeMap<String, String>,
        dry_run: bool,
        test: Option<TestPlan>,
        host: &str,
    ) -> Result<Execution, EngineError> {
        let record = RunRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            workflow_path: absolute(workflow_path),
            target: absolute(target),
            params,
            status: RunStatus::Running,
            started_at: Utc::now(),
            ended_at: None,
            awaiting: None,
            error: None,
            output: None,
            settings: self.settings(host),
        };
        // Dry runs and tests leave no run behind.
        let store: Arc<dyn RunStore> = if dry_run || test.is_some() {
            Arc::new(MemoryStore::default())
        } else {
            Arc::clone(&self.store)
        };
        store.put_run(&record)?;
        let lease = store
            .acquire(&record.id)?
            .ok_or_else(|| EngineError::Busy(record.id.clone()))?;
        Ok(self.begin(store, record, lease, callbacks, dry_run, test, false))
    }

    /// Take a stored run to continue it, marked running again.
    pub(crate) fn begin_resumed(
        &self,
        mut record: RunRecord,
        callbacks: std::collections::BTreeMap<String, String>,
        host: &str,
    ) -> Result<Execution, EngineError> {
        // Step keys identify callbacks the way the host that ran them does.
        if let Some(started) = record.settings.host.as_deref().filter(|h| *h != host) {
            return Err(EngineError::OtherHost {
                run: record.id.clone(),
                started: host_name(started),
                resuming: if host == QUICKJS_HOST {
                    "the codemod CLI"
                } else {
                    "runWorkflow"
                },
            });
        }
        // The lease comes first: while another process executes the run, its
        // record and cancel request are not ours to change.
        let lease = self
            .store
            .acquire(&record.id)?
            .ok_or_else(|| EngineError::Busy(record.id.clone()))?;
        record
            .reopen()
            .map_err(|_| EngineError::AlreadyFinished(record.id.clone()))?;
        self.store.clear_cancel(&record.id)?;
        self.store.put_run(&record)?;
        let store = Arc::clone(&self.store);
        Ok(self.begin(store, record, lease, callbacks, false, None, true))
    }

    /// Announce a run and set up what its steps share.
    #[allow(clippy::too_many_arguments)]
    fn begin(
        &self,
        store: Arc<dyn RunStore>,
        record: RunRecord,
        lease: Box<dyn RunLease>,
        callbacks: std::collections::BTreeMap<String, String>,
        dry_run: bool,
        test: Option<TestPlan>,
        resumed: bool,
    ) -> Execution {
        self.events.emit(Event::RunStarted {
            run_id: record.id.clone(),
            workflow: record.workflow_path.clone(),
            resumed,
        });
        let bundle_dir = record
            .workflow_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let cancel = self.stop.enroll();
        let context = Arc::new(RunContext {
            run_id: record.id.clone(),
            target: record.target.clone(),
            chain: vec![absolute(&bundle_dir).to_string_lossy().into_owned()],
            bundle_dir,
            params: record.params.clone(),
            dry_run,
            started_at: record.started_at.timestamp_millis(),
            callbacks,
            _cancel_watch: Some(CancelWatch::start(
                Arc::clone(&store),
                record.id.clone(),
                Arc::clone(&cancel),
            )),
            store,
            events: Arc::clone(&self.events),
            interaction: Arc::clone(&self.interaction),
            services: Arc::clone(&self.services),
            cancel,
            commit_lock: Arc::new(Mutex::new(())),
            test,
            stopped: AtomicBool::new(false),
            interruption: Mutex::new(None),
        });
        Execution {
            record,
            context,
            stop: self.stop.clone(),
            _lease: lease,
        }
    }

    /// The settings a run's steps depend on, for its record.
    fn settings(&self, host: &str) -> RunSettings {
        let mut capabilities: Vec<String> = self
            .services
            .capabilities
            .iter()
            .map(|capability| capability.as_str().to_string())
            .collect();
        capabilities.sort();
        RunSettings {
            capabilities,
            managed_git: self.services.managed_git,
            agent: self.services.agent.clone(),
            host: Some(host.to_string()),
        }
    }
}

/// What asking a run to stop did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Canceled {
    /// Something is executing the run: it was asked to stop, and stops after
    /// the step it is on.
    Requested,
    /// Nothing was executing it: it is marked canceled.
    Marked,
    /// It had already succeeded or been canceled.
    AlreadyFinished(RunStatus),
}

/// Ask a run to stop, whichever process executes it.
pub fn cancel_run(store: &dyn RunStore, run_id: &str) -> Result<Canceled, EngineError> {
    let mut record = store
        .get_run(run_id)?
        .ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
    if store.is_held(run_id)? {
        store.request_cancel(run_id)?;
        return Ok(Canceled::Requested);
    }
    if matches!(record.status, RunStatus::Succeeded | RunStatus::Canceled) {
        return Ok(Canceled::AlreadyFinished(record.status));
    }
    // Failed, waiting for an approval, or interrupted (its record says
    // running, but its process is gone).
    record.conclude(butterflow_state::run_store::Outcome::Canceled);
    store.put_run(&record)?;
    Ok(Canceled::Marked)
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
