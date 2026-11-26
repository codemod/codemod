//! The public entry point: start, resume and cancel runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use butterflow_state::run_store::{
    MemoryStore, RunLease, RunRecord, RunSettings, RunStatus, RunStore, StepRecord, StepStatus,
    StoreError,
};
use chrono::Utc;
use serde_json::{json, Value};

use crate::build::{build, BuildError, Built};
use crate::events::{bounded, Event, EventSink, PartialStep, StepResult, OUTPUT_LIMIT};
use crate::interaction::Interaction;
use crate::run::{
    execute, CancelWatch, Ending, RunContext, RunError, RunHalt, StopReason, TestPlan,
};
use crate::Services;
use butterflow_state::run_store::NotResumable;

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
    #[error("run '{run}' was {}: it is over. Resume it anyway with --force", status.as_str())]
    Ended { run: String, status: RunStatus },
    #[error("a run with id '{0}' already exists")]
    Exists(String),
    #[error("run '{0}' is being executed by another process; wait for it, or stop it with `codemod workflow cancel --id {0}`")]
    Busy(String),
    #[error("'{0}' cannot name an event: use 1 to 100 letters, digits, '.', '_' or '-'")]
    EventName(String),
    #[error("{}", other_version(run, made_by.as_deref(), stale))]
    OtherVersion {
        run: String,
        /// The version that made the run's steps; `None` for runs from
        /// before versions were recorded.
        made_by: Option<String>,
        /// The labels of the finished steps that would run again.
        stale: Vec<String>,
    },
    #[error("run '{run}' was started {started}, and can only be resumed there: {resuming} records steps differently")]
    OtherHost {
        run: String,
        started: &'static str,
        resuming: &'static str,
    },
}

/// Why a run another codemod version made is not resumed.
fn other_version(run: &str, made_by: Option<&str>, stale: &[String]) -> String {
    let (steps, them) = match stale.len() {
        1 => ("step", "it"),
        _ => ("steps", "them"),
    };
    let made = match made_by {
        Some(version) => format!("codemod {version}"),
        None => "an earlier codemod (2.0.0-alpha.9 or before)".to_string(),
    };
    let same = match made_by {
        Some(version) => format!(
            "Resume it with that version (npx codemod@{version} workflow resume --id {run}), or pass"
        ),
        None => "Pass".to_string(),
    };
    format!(
        "run '{run}' was made by {made}, and codemod {CODEMOD_VERSION} builds the workflow's functions differently: {} finished {steps} ({}) would run again. {same} --force to run {them} again",
        stale.len(),
        stale.join(", ")
    )
}

/// The host a run's JavaScript runs in, as people know it.
fn host_name(host: &str) -> &'static str {
    match host {
        QUICKJS_HOST => "by the codemod CLI",
        _ => "with runWorkflow in Node.js",
    }
}

/// The codemod version, recorded on each run: its step keys depend on it.
pub const CODEMOD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The callback ids (`cb_<16 hex digits>`) a step key names.
fn callback_ids(key: &str) -> impl Iterator<Item = &str> {
    key.match_indices("cb_").filter_map(|(at, _)| {
        let id = key.get(at..at + 19)?;
        id[3..].bytes().all(|b| b.is_ascii_hexdigit()).then_some(id)
    })
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
    /// The new run's id; one is made up when `None`. A host that announces
    /// the run before it starts (the CLI's control channel) picks it.
    pub run_id: Option<String>,
    pub workflow: LoadedWorkflow,
    pub target: PathBuf,
    pub params: Value,
    pub name: Option<String>,
    /// Run every step but change nothing on disk and persist nothing.
    pub dry_run: bool,
    /// Run as a test: steps may be stopped at or stubbed, and nothing is
    /// persisted (changes on disk are made, in the target the test set up).
    pub test: Option<TestPlan>,
    /// Before the run first changes the target, check it has no uncommitted
    /// changes (and is in a git repository), and ask the [`Interaction`]
    /// (`Approval::UncommittedChanges`) when it does. A run that never
    /// changes anything is never checked. Ignored for dry runs and tests.
    pub check_uncommitted: bool,
}

/// How to resume a run.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResumeOptions {
    /// Skip the check for uncommitted changes that a run which has not
    /// written yet makes before its first write (`--allow-dirty`).
    pub allow_dirty: bool,
    /// Continue a run that was canceled or rejected, or one another codemod
    /// version made whose finished function steps would run again (`--force`).
    pub force: bool,
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
    /// How many files failed in the run's partial steps.
    pub fn failed_files(&self) -> usize {
        self.steps.iter().map(StepRecord::failed_files).sum()
    }

    /// The report as the `codemod/testing` helpers and the Node.js host
    /// return it.
    pub fn to_json(&self) -> Value {
        let (output, error, awaiting, stopped_at) = match &self.ending {
            Ending::Succeeded(output) => (output.clone(), None, None, None),
            Ending::Failed(error) => (Value::Null, Some(error), None, None),
            Ending::FailedWithOutput { error, output, .. } => {
                (output.clone(), Some(error), None, None)
            }
            Ending::Canceled => (Value::Null, None, None, None),
            // Paused at a question: the run still waits for it.
            Ending::Paused => (Value::Null, None, self.run.awaiting.as_ref(), None),
            Ending::Rejected(message) => (Value::Null, Some(message), None, None),
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
            "exitCode": match &self.ending {
                Ending::FailedWithOutput { exit_code, .. } => Some(*exit_code),
                _ => None,
            },
            "steps": steps,
        })
    }
}

/// How a run asked to stop ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopMode {
    /// Paused: resuming it continues where it stopped.
    Pause,
    /// Canceled: it is over (resuming it takes `--force`).
    Cancel,
}

impl StopMode {
    fn reason(self) -> StopReason {
        match self {
            StopMode::Pause => StopReason::Pause,
            StopMode::Cancel => StopReason::Cancel,
        }
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
    /// The strongest stop asked for so far (`StopReason` as a number).
    requested: AtomicU8,
    runs: Mutex<Vec<Arc<RunHalt>>>,
}

impl StopHandle {
    /// Pause the runs (Ctrl-C).
    pub fn stop(&self) {
        self.stop_with(StopMode::Pause);
    }

    pub fn stop_with(&self, mode: StopMode) {
        let reason = mode.reason();
        self.inner
            .requested
            .fetch_max(reason as u8, Ordering::SeqCst);
        for run in self.runs().iter() {
            run.stop(reason);
        }
    }

    fn runs(&self) -> std::sync::MutexGuard<'_, Vec<Arc<RunHalt>>> {
        self.inner.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The halt of a run that starts now, stopped already if a stop came.
    fn enroll(&self) -> Arc<RunHalt> {
        let halt = Arc::new(RunHalt::new(Arc::new(AtomicBool::new(false))));
        match self.inner.requested.load(Ordering::SeqCst) {
            0 => {}
            1 => halt.stop(StopReason::Pause),
            _ => halt.stop(StopReason::Cancel),
        }
        self.runs().push(Arc::clone(&halt));
        halt
    }

    fn release(&self, halt: &Arc<RunHalt>) {
        self.runs().retain(|run| !Arc::ptr_eq(run, halt));
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
        match ending {
            Ending::AwaitingApproval(_) => record.question = context.halt.question(),
            // Paused while a question was open: the run still waits for it,
            // and `workflow resume --approve <id>` gives it.
            Ending::Paused => {
                if let Some(question) = context.halt.question() {
                    record.awaiting = context.awaiting().or_else(|| {
                        context
                            .store
                            .load_steps(&record.id)
                            .ok()?
                            .into_iter()
                            .find(|step| step.status == StepStatus::AwaitingApproval)
                            .and_then(|step| step.error)
                    });
                    record.question = Some(question);
                }
            }
            _ => {}
        }
        // Still to check when the run never got past the check.
        record.settings.check_uncommitted = context
            .write_gate
            .as_ref()
            .is_some_and(|gate| !gate.passed());
        context.store.put_run(&record)?;
        self.record = record;
        self.stop.release(&context.halt);
        if let Some(overlay) = &context.overlay {
            for diff in overlay.diffs() {
                context.events.emit(Event::RunDiff {
                    path: diff.path,
                    diff: diff.diff_text,
                    additions: diff.additions,
                    deletions: diff.deletions,
                });
            }
        }
        let steps = context.store.load_steps(&self.record.id)?;
        let partial_steps = PartialStep::of(&steps);
        let (output, output_omitted) = bounded(ending.output(), OUTPUT_LIMIT);
        context.events.emit(Event::RunFinished {
            run_id: self.record.id.clone(),
            status: self.record.status,
            error: self.record.error.clone(),
            awaiting: self.record.awaiting.clone(),
            question: self.record.question.clone(),
            failed_files: partial_steps.iter().map(|step| step.failed_files).sum(),
            partial_steps,
            output,
            output_omitted,
            steps: StepResult::of(&steps),
        });
        Ok(steps)
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
            request.run_id,
            &request.workflow.path,
            &request.target,
            request.params,
            request.name,
            request.workflow.built.callbacks.clone(),
            request.dry_run,
            request.test,
            request.check_uncommitted,
            QUICKJS_HOST,
        )?;
        self.drive(execution, &request.workflow.built)
    }

    /// Continue a run that failed, was paused or interrupted, or waits for
    /// an approval.
    pub fn resume(&self, run_id: &str) -> Result<RunReport, EngineError> {
        self.resume_with(run_id, ResumeOptions::default())
    }

    /// [`Self::resume`], with options. A run that was started with the
    /// check for uncommitted changes and never passed it (it wrote nothing)
    /// checks before its first write, unless `options.allow_dirty`. A run
    /// that was canceled or rejected is continued only with `options.force`.
    pub fn resume_with(
        &self,
        run_id: &str,
        options: ResumeOptions,
    ) -> Result<RunReport, EngineError> {
        // Load the workflow first: one that no longer builds leaves the run
        // as it was.
        let record = self
            .store
            .get_run(run_id)?
            .ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
        let workflow = load_workflow(&record.workflow_path)?;
        let execution = self.begin_resumed(
            record,
            workflow.built.callbacks.clone(),
            QUICKJS_HOST,
            options,
        )?;
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
        run_id: Option<String>,
        workflow_path: &Path,
        target: &Path,
        params: Value,
        name: Option<String>,
        callbacks: std::collections::BTreeMap<String, String>,
        dry_run: bool,
        test: Option<TestPlan>,
        check_uncommitted: bool,
        host: &str,
    ) -> Result<Execution, EngineError> {
        let check_uncommitted = check_uncommitted && !dry_run && test.is_none();
        let mut record = RunRecord {
            id: run_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name,
            workflow_path: absolute(workflow_path),
            target: absolute(target),
            params,
            status: RunStatus::Running,
            started_at: Utc::now(),
            ended_at: None,
            awaiting: None,
            question: None,
            error: None,
            output: None,
            exit_code: None,
            settings: self.settings(host),
            codemod_version: Some(CODEMOD_VERSION.to_string()),
        };
        record.settings.check_uncommitted = check_uncommitted;
        // Dry runs and tests leave no run behind.
        let store: Arc<dyn RunStore> = if dry_run || test.is_some() {
            Arc::new(MemoryStore::default())
        } else {
            Arc::clone(&self.store)
        };
        if store.get_run(&record.id)?.is_some() {
            return Err(EngineError::Exists(record.id));
        }
        store.put_run(&record)?;
        let lease = store
            .acquire(&record.id)?
            .ok_or_else(|| EngineError::Busy(record.id.clone()))?;
        let gate = check_uncommitted.then(|| {
            Arc::new(crate::write_gate::WriteGate::new(
                &record.target,
                &record.id,
                Arc::clone(&store),
            ))
        });
        Ok(self.begin(store, record, lease, callbacks, dry_run, test, false, gate))
    }

    /// Take a stored run to continue it, marked running again.
    pub(crate) fn begin_resumed(
        &self,
        mut record: RunRecord,
        callbacks: std::collections::BTreeMap<String, String>,
        host: &str,
        options: ResumeOptions,
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
        // Function steps are keyed by a hash of the code the build generates,
        // which another version may generate differently. Then finished
        // steps would silently run again: refused, unless forced.
        if host == QUICKJS_HOST
            && !options.force
            && record.codemod_version.as_deref() != Some(CODEMOD_VERSION)
        {
            let stale: Vec<String> = self
                .store
                .load_steps(&record.id)?
                .into_iter()
                .filter(|step| matches!(step.status, StepStatus::Succeeded | StepStatus::Partial))
                .filter(|step| callback_ids(&step.key).any(|id| !callbacks.contains_key(id)))
                .map(|step| step.label)
                .collect();
            if !stale.is_empty() {
                return Err(EngineError::OtherVersion {
                    run: record.id.clone(),
                    made_by: record.codemod_version.clone(),
                    stale,
                });
            }
        }
        // The lease comes first: while another process executes the run, its
        // record and cancel request are not ours to change.
        let lease = self
            .store
            .acquire(&record.id)?
            .ok_or_else(|| EngineError::Busy(record.id.clone()))?;
        record
            .reopen(options.force)
            .map_err(|refused| match refused {
                NotResumable::AlreadySucceeded => EngineError::AlreadyFinished(record.id.clone()),
                NotResumable::Ended(status) => EngineError::Ended {
                    run: record.id.clone(),
                    status,
                },
            })?;
        self.store.clear_cancel(&record.id)?;
        // Steps recorded from here on are this version's.
        record.codemod_version = Some(CODEMOD_VERSION.to_string());
        // A run that already wrote is not checked again: it made the changes
        // there may be. One that never got past the check checks again.
        if options.allow_dirty {
            record.settings.check_uncommitted = false;
        }
        self.store.put_run(&record)?;
        let store = Arc::clone(&self.store);
        let gate = record.settings.check_uncommitted.then(|| {
            Arc::new(crate::write_gate::WriteGate::new(
                &record.target,
                &record.id,
                Arc::clone(&store),
            ))
        });
        Ok(self.begin(store, record, lease, callbacks, false, None, true, gate))
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
        write_gate: Option<Arc<crate::write_gate::WriteGate>>,
    ) -> Execution {
        self.events.emit(Event::RunStarted {
            run_id: record.id.clone(),
            workflow: record.workflow_path.clone(),
            resumed,
            target: record.target.clone(),
            params: record.params.clone(),
            dry_run,
            settings: (&record.settings).into(),
        });
        let bundle_dir = crate::run::workflow_dir(&record.workflow_path);
        let halt = self.stop.enroll();
        let cancel = Arc::clone(halt.cancel());
        let context = Arc::new(RunContext {
            run_id: record.id.clone(),
            target: record.target.clone(),
            chain: vec![absolute(&bundle_dir).to_string_lossy().into_owned()],
            bundle_dir,
            workflow_path: record.workflow_path.clone(),
            package_dir: crate::run::package_dir(&record.workflow_path),
            params: record.params.clone(),
            dry_run,
            started_at: record.started_at.timestamp_millis(),
            callbacks,
            _cancel_watch: Some(CancelWatch::start(
                Arc::clone(&store),
                record.id.clone(),
                Arc::clone(&halt),
            )),
            key_prefix: None,
            halt,
            store,
            events: Arc::clone(&self.events),
            interaction: Arc::clone(&self.interaction),
            services: Arc::clone(&self.services),
            cancel,
            commit_lock: Arc::new(Mutex::new(())),
            overlay: dry_run.then(|| Arc::new(crate::tasks::overlay::Overlay::new(&record.target))),
            alongside: false,
            test,
            stopped: AtomicBool::new(false),
            interruption: Mutex::new(None),
            assess: Default::default(),
            write_gate,
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
            check_uncommitted: false,
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
    /// It had already succeeded, or been canceled or rejected.
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
    if record.status.resumable() != butterflow_state::run_store::Resumable::Yes {
        return Ok(Canceled::AlreadyFinished(record.status));
    }
    // Failed, paused, waiting for an approval, or interrupted (its record
    // says running, but its process is gone).
    record.conclude(butterflow_state::run_store::Outcome::Canceled);
    store.put_run(&record)?;
    Ok(Canceled::Marked)
}

/// Leave an event for a run: a `waitForEvent(name)` step in it takes the
/// event, now if one is waiting, or when the run gets there or is resumed.
/// `data` is what that step resolves to.
pub fn send_event(
    store: &dyn RunStore,
    run_id: &str,
    name: &str,
    data: &serde_json::Value,
) -> Result<(), EngineError> {
    if !butterflow_state::run_store::valid_event_name(name) {
        return Err(EngineError::EventName(name.to_string()));
    }
    let record = store
        .get_run(run_id)?
        .ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
    match record.status.resumable() {
        butterflow_state::run_store::Resumable::Yes => {}
        butterflow_state::run_store::Resumable::No => {
            return Err(EngineError::AlreadyFinished(run_id.to_string()))
        }
        butterflow_state::run_store::Resumable::WithForce => {
            return Err(EngineError::Ended {
                run: run_id.to_string(),
                status: record.status,
            })
        }
    }
    Ok(store.send_event(run_id, name, data)?)
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
