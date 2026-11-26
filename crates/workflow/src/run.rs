//! Runs a built workflow in one QuickJS context.
//!
//! The workflow body runs on the calling thread, with the modules that are
//! deterministic (pure LLRT modules such as `path` and `url`,
//! `codemod/ast-grep`, packages and relative files) and replay-safe globals
//! (`js/body_globals.ts`). Steps and `forEachFile` callbacks run in their own
//! isolates (see `tasks::isolate`). Host tasks run on their own
//! threads and report back over a channel; the loop below settles their
//! promises, so steps started together run at the same time. Every step's
//! outcome goes to the run store as soon as it is known.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use butterflow_state::run_store::{Outcome, RunStore, StepRecord, StepStatus, StoreResult};
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::quickjs_adapters::{
    CodemodModuleResolver, QuickJSLoader, QuickJSResolver,
};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::AstGrepModule;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver, Resolver};
use rquickjs::promise::PromiseState;
use rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Object, Promise, Runtime, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::build::{Built, ASSESS_MODULE, DEPS_MODULE, FLOW_MODULE, GIT_MODULE};
use crate::events::{step_kind, Event, EventSink, StepAt};
use crate::interaction::Interaction;
use crate::tasks::{self, TaskContext};
use crate::Services;

/// Not under `codemod/` or `codemod:`: those names are the public modules.
pub(crate) const ORCHESTRATOR_MODULE: &str = "codemod-internal:orchestrator";
/// `semver` for the body and isolates; see `js/semver.ts`.
pub(crate) const SEMVER_MODULE: &str = "codemod-internal:semver";
const WORKFLOW_MODULE: &str = "workflow.js";
const PATH_SHIM_MODULE: &str = "codemod-internal:path";

struct BodyPathResolver;

impl Resolver for BodyPathResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, base: &str, name: &str) -> rquickjs::Result<String> {
        if base != PATH_SHIM_MODULE && matches!(name, "path" | "node:path") {
            Ok(PATH_SHIM_MODULE.into())
        } else {
            // The shim itself imports the original LLRT module.
            Err(rquickjs::Error::new_resolving(base, name))
        }
    }
}

/// LLRT modules left out of the workflow body: they read the clock, the
/// environment or the machine, or schedule work. Steps and callbacks have them.
const BODY_EXCLUDED_MODULES: &[LlrtSupportedModules] = &[
    LlrtSupportedModules::Console,
    LlrtSupportedModules::Crypto,
    LlrtSupportedModules::Os,
    LlrtSupportedModules::PerfHooks,
    LlrtSupportedModules::Process,
    LlrtSupportedModules::Timers,
    LlrtSupportedModules::Tty,
];
const CANCEL_POLL: Duration = Duration::from_millis(250);

/// The directory a workflow file is in.
pub(crate) fn workflow_dir(workflow_path: &Path) -> PathBuf {
    workflow_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The package a workflow belongs to: the nearest directory with a
/// `codemod.yaml`, from the workflow's own up, without leaving the
/// repository it is in. The workflow's directory when there is none.
pub(crate) fn package_dir(workflow_path: &Path) -> PathBuf {
    let dir = workflow_dir(workflow_path);
    let start = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    for ancestor in start.ancestors() {
        if ancestor.join("codemod.yaml").is_file() {
            return ancestor.to_path_buf();
        }
        if ancestor.join(".git").exists() {
            break;
        }
    }
    dir
}

/// `import.meta.dirname`, `filename` and `url` for the workflow module and
/// its callbacks: the workflow file's, wherever the code runs.
pub(crate) fn set_import_meta<T>(
    module: &Module<'_, T>,
    workflow_path: &Path,
) -> rquickjs::Result<()> {
    let filename =
        std::fs::canonicalize(workflow_path).unwrap_or_else(|_| workflow_path.to_path_buf());
    let meta = module.meta()?;
    meta.set("filename", filename.to_string_lossy().into_owned())?;
    meta.set(
        "dirname",
        workflow_dir(&filename).to_string_lossy().into_owned(),
    )?;
    meta.set("url", file_url(&filename))?;
    Ok(())
}

/// A `file://` URL for an absolute path.
fn file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file://");
    if !path.starts_with('/') {
        url.push('/');
    }
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(byte as char)
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// Everything one run needs, shared with host task threads.
pub(crate) struct RunContext {
    pub run_id: String,
    pub target: PathBuf,
    /// Directory of the workflow file; relative task paths resolve here.
    pub bundle_dir: PathBuf,
    /// The workflow file: `import.meta.filename` in the workflow and its
    /// callbacks.
    pub workflow_path: PathBuf,
    /// The package the workflow belongs to, where its `codemod.yaml` is: the
    /// workflow's directory or one above it (`src/workflow.ts`). The
    /// package's languages and skill are read from here.
    pub package_dir: PathBuf,
    pub params: serde_json::Value,
    pub dry_run: bool,
    /// When the run started, in milliseconds since the epoch: where the
    /// workflow body's clock starts.
    pub started_at: i64,
    pub callbacks: BTreeMap<String, String>,
    pub store: Arc<dyn RunStore>,
    pub events: Arc<dyn EventSink>,
    pub interaction: Arc<dyn Interaction>,
    pub services: Arc<Services>,
    /// Set to stop the run: by the engine's stop handle (Ctrl-C), or by a
    /// cancel request in the store (`watch_cancel`). Shared with nested
    /// codemods.
    pub cancel: Arc<AtomicBool>,
    /// Serializes file commits from steps running at the same time, in this
    /// run and in the codemods nested in it.
    pub commit_lock: Arc<Mutex<()>>,
    /// In a dry run, what its steps would have changed so far, which later
    /// steps read instead of the disk; `None` otherwise. Shared with nested
    /// codemods.
    pub overlay: Option<Arc<crate::tasks::overlay::Overlay>>,
    /// For a nested codemod: the step running it, or one of its own parents,
    /// ran alongside other steps. Its managed git steps are refused then, as
    /// they share the checkout with those steps.
    pub alongside: bool,
    /// The codemod packages being run, outermost first, for cycle detection:
    /// the run's own workflow directory, then each nested codemod.
    pub chain: Vec<String>,
    /// Steps to stop at or to stub, when the run is a test.
    pub test: Option<TestPlan>,
    /// Set once the test plan's `until` step has started.
    pub stopped: AtomicBool,
    /// A step that could not finish, which decides how the run ends whatever
    /// the body does with the error.
    pub interruption: Mutex<Option<Interruption>>,
    /// The run's `assess()` calls: usage by step, and the import graph.
    pub assess: crate::assess::RunAssess,
    /// Checks for uncommitted changes before the run first changes its
    /// target; `None` when nothing is checked (dry runs, tests, resumes,
    /// `--allow-dirty`). Shared with nested codemods.
    pub write_gate: Option<Arc<crate::write_gate::WriteGate>>,
    /// For a nested codemod: the key of the step running it, which its own
    /// steps' keys start with. `None` for the run itself.
    pub key_prefix: Option<String>,
    /// Why the run is stopping, if it is: shared with nested codemods.
    pub halt: Arc<RunHalt>,
    /// Keeps `cancel` following the store's cancel requests; `None` for
    /// nested codemods, which share their parent's flag.
    pub _cancel_watch: Option<CancelWatch>,
}

/// Why a run stops before its body ends. Stronger reasons win: a run
/// paused and then canceled is canceled, and a rejection beats both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub(crate) enum StopReason {
    None = 0,
    /// Ctrl-C, `stop {mode: "pause"}`: the run can be resumed.
    Pause = 1,
    /// `workflow cancel`, `stop {mode: "cancel"}`: the run is over.
    Cancel = 2,
    /// An approval was rejected: the run is over.
    Reject = 3,
}

/// How a run is being stopped, shared by the run, its nested codemods and
/// the engine's stop handle. Also where the approval the run waits for is
/// noted, so the run's record can name it.
pub(crate) struct RunHalt {
    /// The run's own cancel flag (`RunContext::cancel` of the run itself):
    /// every reason to stop sets it.
    cancel: Arc<AtomicBool>,
    reason: AtomicU8,
    rejected: Mutex<Option<String>>,
    question: Mutex<Option<String>>,
}

impl RunHalt {
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            reason: AtomicU8::new(StopReason::None as u8),
            rejected: Mutex::new(None),
            question: Mutex::new(None),
        }
    }

    /// The run's own cancel flag.
    pub fn cancel(&self) -> &Arc<AtomicBool> {
        &self.cancel
    }

    pub fn stop(&self, reason: StopReason) {
        self.reason.fetch_max(reason as u8, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// End the run as rejected: `message` is the approval's. Steps running
    /// alongside stop as on a cancel.
    pub fn reject(&self, message: String) {
        self.rejected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(message);
        self.stop(StopReason::Reject);
    }

    pub fn reason(&self) -> StopReason {
        match self.reason.load(Ordering::SeqCst) {
            0 => StopReason::None,
            1 => StopReason::Pause,
            2 => StopReason::Cancel,
            _ => StopReason::Reject,
        }
    }

    pub fn rejected(&self) -> Option<String> {
        self.rejected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Note the question of an approval the run will wait for; the first
    /// one is the one its record names.
    pub fn wait_for(&self, question: String) {
        self.question
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(question);
    }

    pub fn question(&self) -> Option<String> {
        self.question
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Why a step could not finish.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Interruption {
    /// The run was canceled.
    Canceled,
    /// The step waits for an approval nobody could give now.
    AwaitingApproval(String),
}

/// Cancels a run when the store holds a cancel request for it (`codemod
/// workflow cancel`, from any process). Stops when dropped.
pub(crate) struct CancelWatch {
    _stop: mpsc::Sender<()>,
}

impl CancelWatch {
    pub fn start(store: Arc<dyn RunStore>, run_id: String, halt: Arc<RunHalt>) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(CANCEL_POLL) {
                if store.cancel_requested(&run_id).unwrap_or(false) {
                    halt.stop(StopReason::Cancel);
                }
            }
        });
        Self { _stop: stop }
    }
}

/// How a test runs a workflow: where to stop, and which steps to skip with a
/// given result. Steps are matched by label.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct TestPlan {
    /// Stop after this step: steps the workflow starts after it are not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Results for steps that should not run, by label.
    pub given: serde_json::Map<String, serde_json::Value>,
}

impl RunContext {
    /// The workflow is a workflow.yaml, so the package is a codemod 1.x
    /// package: its transforms get the 1.x APIs 2.0 removed (see
    /// `js/v1.ts`). Nested codemods decide for their own workflow.
    pub(crate) fn v1_compat(&self) -> bool {
        !crate::engine::is_typescript(&self.workflow_path)
    }

    /// The context of a codemod nested in this run's step at `position`: its
    /// own workflow, params and step history, everything else shared. It
    /// stops when that step does (`cancel`). `alongside`: that step ran
    /// alongside other steps.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn child(
        &self,
        position: u64,
        alongside: bool,
        workflow_path: PathBuf,
        package_dir: PathBuf,
        params: serde_json::Value,
        callbacks: BTreeMap<String, String>,
        events: Arc<dyn EventSink>,
        identity: String,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let mut chain = self.chain.clone();
        chain.push(identity);
        let key_prefix = Some(self.at(position).step);
        Self {
            run_id: format!("{}~{position}", self.run_id),
            target: self.target.clone(),
            bundle_dir: workflow_dir(&workflow_path),
            workflow_path,
            package_dir,
            params,
            dry_run: self.dry_run,
            started_at: self.started_at,
            callbacks,
            store: Arc::clone(&self.store),
            events,
            interaction: Arc::clone(&self.interaction),
            services: Arc::clone(&self.services),
            cancel,
            commit_lock: Arc::clone(&self.commit_lock),
            overlay: self.overlay.clone(),
            alongside: self.alongside || alongside,
            chain,
            test: None,
            stopped: AtomicBool::new(false),
            interruption: Mutex::new(None),
            assess: Default::default(),
            write_gate: self.write_gate.clone(),
            key_prefix,
            halt: Arc::clone(&self.halt),
            _cancel_watch: None,
        }
    }

    /// Which step an event at `position` is about.
    pub fn at(&self, position: u64) -> StepAt {
        match &self.key_prefix {
            None => StepAt::top(position),
            Some(prefix) => StepAt {
                position,
                step: format!("{prefix}/{position}"),
                parent: Some(prefix.clone()),
            },
        }
    }

    /// Call before changing the target: `Err` when it has uncommitted
    /// changes nobody agreed to write over (see `write_gate`).
    pub fn before_write(&self) -> Result<(), String> {
        match &self.write_gate {
            Some(gate) if !self.dry_run => gate.check(self.interaction.as_ref(), &self.cancel),
            _ => Ok(()),
        }
    }

    pub fn canceled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Note that a step could not finish. A cancel outranks a wait, and the
    /// first wait is the one reported.
    pub fn interrupt(&self, interruption: Interruption) {
        let mut slot = self.interruption.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() || interruption == Interruption::Canceled {
            *slot = Some(interruption);
        }
    }

    /// What the approval a step waits for asks, if one does: also when the
    /// run was paused while it waited.
    pub fn awaiting(&self) -> Option<String> {
        match &*self.interruption.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(Interruption::AwaitingApproval(message)) => Some(message.clone()),
            _ => None,
        }
    }

    /// Whether a step about to start must not, because the run was canceled.
    pub fn refuses_steps(&self) -> bool {
        let canceled = self.canceled();
        if canceled {
            self.interrupt(Interruption::Canceled);
        }
        canceled
    }

    /// The label of the test plan's `until` step, once it has started.
    pub fn stopped_at(&self) -> Option<String> {
        if !self.stopped.load(Ordering::SeqCst) {
            return None;
        }
        self.test.as_ref().and_then(|plan| plan.until.clone())
    }

    /// How the run ended, given how its body ended. A rejected approval, a
    /// step that was stopped or one that waits for an approval decides it
    /// even when the body caught the error and carried on; a test's `until`
    /// step turns success into stopped. Why the run was stopped decides
    /// whether it is paused, canceled or rejected.
    pub fn ending(&self, body: Ending) -> Ending {
        if let Some(message) = self.halt.rejected() {
            return Ending::Rejected(message);
        }
        let interruption = self
            .interruption
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let stopped = || match self.halt.reason() {
            StopReason::Cancel | StopReason::Reject => Ending::Canceled,
            StopReason::None | StopReason::Pause => Ending::Paused,
        };
        match (interruption, body) {
            (_, Ending::Rejected(message)) => Ending::Rejected(message),
            (Some(Interruption::Canceled), _) | (_, Ending::Canceled | Ending::Paused) => stopped(),
            // A run stopped at a question ends as stopped. A paused one
            // keeps the question (see `awaiting`); resuming it asks again.
            (Some(Interruption::AwaitingApproval(_)), _) | (_, Ending::AwaitingApproval(_))
                if self.halt.reason() != StopReason::None =>
            {
                stopped()
            }
            (Some(Interruption::AwaitingApproval(message)), _)
            | (_, Ending::AwaitingApproval(message)) => Ending::AwaitingApproval(message),
            // Stopping a run mid-step surfaces as that step's failure.
            (None, Ending::Failed(_)) if self.canceled() => stopped(),
            (None, Ending::Succeeded(output)) => match self.stopped_at() {
                Some(label) => Ending::Stopped(label),
                None => Ending::Succeeded(output),
            },
            (None, body) => body,
        }
    }

    /// Persist one step's outcome and report it. A step that only began to
    /// wait has not finished, so that is saved and not reported.
    pub fn record_step(&self, step: &StepRecord, duration_ms: u64) -> StoreResult<()> {
        let mut timed = step.clone();
        timed.duration_ms = Some(duration_ms);
        self.store.put_step(&self.run_id, &timed)?;
        if step.status == StepStatus::Waiting {
            return Ok(());
        }
        self.events.emit(Event::StepFinished {
            at: self.at(step.position),
            label: step.label.clone(),
            kind: step_kind(&step.key),
            status: step.status,
            error: step.error.clone(),
            duration_ms,
        });
        Ok(())
    }

    /// Report what the orchestrator says about a step:
    /// `{ type: "stepStarted" | "stepReplayed", position, label, kind }`, or
    /// `{ type: "stepRetrying", position, label, attempt, attempts, error }`.
    pub fn host_event(&self, event: &str) -> Result<(), String> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum HostEvent {
            #[serde(rename = "stepStarted")]
            Started {
                position: u64,
                label: String,
                #[serde(default)]
                kind: Option<String>,
            },
            #[serde(rename = "stepReplayed")]
            Replayed {
                position: u64,
                label: String,
                #[serde(default)]
                kind: Option<String>,
            },
            #[serde(rename = "stepRetrying")]
            Retrying {
                position: u64,
                label: String,
                attempt: u32,
                attempts: u32,
                error: String,
            },
        }
        let event = serde_json::from_str(event).map_err(|e| format!("bad step event: {e}"))?;
        self.events.emit(match event {
            HostEvent::Started {
                position,
                label,
                kind,
            } => Event::StepStarted {
                at: self.at(position),
                label,
                kind,
            },
            HostEvent::Replayed {
                position,
                label,
                kind,
            } => Event::StepReplayed {
                at: self.at(position),
                label,
                kind,
            },
            HostEvent::Retrying {
                position,
                label,
                attempt,
                attempts,
                error,
            } => Event::StepRetrying {
                at: self.at(position),
                label,
                attempt,
                attempts,
                error,
            },
        });
        Ok(())
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "kebab-case")]
pub enum Ending {
    Succeeded(serde_json::Value),
    Failed(String),
    /// The workflow called `fail()`: the run failed, and still has an output
    /// and the exit code the CLI ends with.
    #[serde(rename_all = "camelCase")]
    FailedWithOutput {
        error: String,
        output: serde_json::Value,
        exit_code: i32,
    },
    /// Canceled: the run is over (`workflow cancel`, `stop {mode: "cancel"}`).
    Canceled,
    /// Stopped on request, to be resumed (Ctrl-C, `stop {mode: "pause"}`).
    Paused,
    /// A manual approval was rejected (its message here): the run is over,
    /// and what earlier steps changed stays.
    Rejected(String),
    /// Stopped at a manual approval nobody could give.
    AwaitingApproval(String),
    /// A test run stopped after its `until` step (named here).
    Stopped(String),
}

impl Ending {
    /// The one-word form used in reports and by `codemod/testing`.
    pub fn status(&self) -> &'static str {
        match self {
            Self::Succeeded(_) => "succeeded",
            Self::Failed(_) | Self::FailedWithOutput { .. } => "failed",
            Self::Canceled => "canceled",
            Self::Paused => "paused",
            Self::Rejected(_) => "rejected",
            Self::AwaitingApproval(_) => "awaiting-approval",
            Self::Stopped(_) => "stopped",
        }
    }

    /// What the run's record keeps of it. Only test runs stop early, and
    /// they are never resumed, so stopped is stored as succeeded.
    /// What the workflow returned, or gave `fail()`; `Null` otherwise.
    pub fn output(&self) -> &serde_json::Value {
        match self {
            Self::Succeeded(output) | Self::FailedWithOutput { output, .. } => output,
            _ => &serde_json::Value::Null,
        }
    }

    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Succeeded(output) => Outcome::Succeeded(output.clone()),
            Self::Failed(error) => Outcome::Failed(error.clone()),
            Self::FailedWithOutput {
                error,
                output,
                exit_code,
            } => Outcome::FailedWithOutput {
                error: error.clone(),
                output: output.clone(),
                exit_code: *exit_code,
            },
            Self::Canceled => Outcome::Canceled,
            Self::Paused => Outcome::Paused,
            Self::Rejected(message) => Outcome::Rejected(message.clone()),
            Self::AwaitingApproval(message) => Outcome::AwaitingApproval(message.clone()),
            Self::Stopped(_) => Outcome::Succeeded(serde_json::Value::Null),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("could not start the JavaScript runtime: {0}")]
    Runtime(String),
    #[error(transparent)]
    Store(#[from] butterflow_state::run_store::StoreError),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostRequest {
    position: u64,
    label: String,
    task: serde_json::Value,
    /// How long the task may run, from the step's `timeout`.
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// When the step began to wait (`sleep`, `waitForEvent`).
    #[serde(default)]
    since: Option<i64>,
    /// Other steps were still running when the step was issued.
    #[serde(default)]
    alongside: bool,
}

/// Stops one step's task when its time is up, or when the run is canceled.
pub(crate) struct StepClock {
    /// What the task watches instead of the run's flag.
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) expired: Arc<AtomicBool>,
    _stop: mpsc::Sender<()>,
}

impl StepClock {
    const POLL: Duration = Duration::from_millis(50);

    pub(crate) fn start(run: Arc<AtomicBool>, timeout: Duration) -> Self {
        let cancel = Arc::new(AtomicBool::new(run.load(Ordering::SeqCst)));
        let expired = Arc::new(AtomicBool::new(false));
        let (stop, stopped) = mpsc::channel::<()>();
        let (flag, late) = (Arc::clone(&cancel), Arc::clone(&expired));
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                match stopped.recv_timeout(left.min(Self::POLL)) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => return,
                }
                if run.load(Ordering::SeqCst) {
                    flag.store(true, Ordering::SeqCst);
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    late.store(true, Ordering::SeqCst);
                    flag.store(true, Ordering::SeqCst);
                    return;
                }
            }
        });
        Self {
            cancel,
            expired,
            _stop: stop,
        }
    }
}

type Settlement = (u32, Result<String, String>);

pub(crate) fn execute(built: &Built, run: Arc<RunContext>) -> Result<Ending, RunError> {
    let recorded = run.store.load_steps(&run.run_id)?;

    let runtime = Runtime::new().map_err(|e| RunError::Runtime(e.to_string()))?;
    let context = Context::full(&runtime).map_err(|e| RunError::Runtime(e.to_string()))?;
    let (llrt_resolver, llrt_loader, llrt_globals) =
        LlrtModuleBuilder::build_with_exclusions(BODY_EXCLUDED_MODULES)
            .builder
            .build();
    let packages = OxcResolver::new(run.bundle_dir.clone(), None)
        .map_err(|e| RunError::Runtime(format!("cannot resolve imports: {e}")))?;
    runtime.set_loader(
        (
            codemod_sandbox::sandbox::engine::target_files::WorkflowFsResolver(false),
            // The body's codemod modules: `codemod/<name>`, or `codemod:<name>`.
            CodemodModuleResolver::new(&["ast-grep", "workflow", "deps", "git", "assess"]),
            BodyPathResolver,
            llrt_resolver,
            BuiltinResolver::default()
                .with_module(ORCHESTRATOR_MODULE)
                .with_module(SEMVER_MODULE),
            QuickJSResolver::new(Arc::new(packages)),
        ),
        (
            llrt_loader.with_module("codemod/ast-grep", AstGrepModule),
            BuiltinLoader::default()
                .with_module(FLOW_MODULE, crate::js::workflow_module())
                .with_module(DEPS_MODULE, crate::js::deps_module())
                .with_module(GIT_MODULE, crate::js::git_module())
                .with_module(ASSESS_MODULE, crate::js::assess_module())
                .with_module(ORCHESTRATOR_MODULE, crate::js::orchestrator())
                .with_module(SEMVER_MODULE, crate::js::semver_module())
                .with_module(PATH_SHIM_MODULE, include_str!("js/path.js")),
            QuickJSLoader,
        ),
    );
    // Canceling interrupts the body's JavaScript, until the run's outcome is
    // decided: recording what the remaining steps did must not be cut short.
    let interruptible = Arc::new(AtomicBool::new(true));
    let (interrupt, armed) = (Arc::clone(&run.cancel), Arc::clone(&interruptible));
    runtime.set_interrupt_handler(Some(Box::new(move || {
        armed.load(Ordering::SeqCst) && interrupt.load(Ordering::SeqCst)
    })));

    let in_flight = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel::<Settlement>();

    let ending = context.with(|ctx| {
        let started = (|| -> rquickjs::Result<Value<'_>> {
            llrt_globals.attach(&ctx)?;
            ctx.store_userdata(codemod_sandbox::sandbox::engine::target_files::WorkflowBody)
                .map_err(|e| {
                    rquickjs::Error::new_from_js_message("userdata", "workflow", format!("{e:?}"))
                })?;
            install_host(&ctx, &run, &recorded, &in_flight, &sender)?;
            ctx.eval::<(), _>(crate::js::body_globals())?;
            // Load the runtime even when the workflow never imports it.
            Module::import(&ctx, FLOW_MODULE)?.finish::<()>()?;
            let declared = Module::declare(ctx.clone(), WORKFLOW_MODULE, built.main.as_str())?;
            set_import_meta(&declared, &run.workflow_path)?;
            let (module, promise) = declared.eval()?;
            promise.finish::<()>()?;
            let main: Function = module.get("default")?;
            let input = json!({
                "params": run.params,
                "target": run.target,
                "dryRun": run.dry_run,
                // Use Node.js platform names on both hosts.
                "platform": match std::env::consts::OS {
                    "windows" => "win32",
                    "macos" => "darwin",
                    "solaris" | "illumos" => "sunos",
                    os => os,
                },
            });
            let input = ctx.json_parse(input.to_string())?;
            main.call((input,))
        })()
        .catch(&ctx)
        .map_err(|e| e.to_string());

        let value = match started {
            Ok(value) => value,
            Err(error) => return run.ending(Ending::Failed(error)),
        };
        let Some(promise) = value.as_promise().cloned() else {
            return run.ending(match to_json(&ctx, value) {
                Ok(value) => Ending::Succeeded(value),
                Err(error) => Ending::Failed(error),
            });
        };
        drive(&ctx, &run, promise, &receiver, &in_flight, &interruptible)
    });
    Ok(ending)
}

fn install_host<'js>(
    ctx: &Ctx<'js>,
    run: &Arc<RunContext>,
    recorded: &[StepRecord],
    in_flight: &Arc<AtomicUsize>,
    sender: &mpsc::Sender<Settlement>,
) -> rquickjs::Result<()> {
    let object = Object::new(ctx.clone())?;

    let history = serde_json::to_string(recorded).unwrap_or_else(|_| "[]".to_string());
    object.set(
        "history",
        Function::new(ctx.clone(), move || history.clone())?,
    )?;

    let recorder = Arc::clone(run);
    object.set(
        "record",
        Function::new(
            ctx.clone(),
            move |entry: String, duration_ms: f64| -> rquickjs::Result<()> {
                let step: StepRecord = serde_json::from_str(&entry).map_err(|e| {
                    rquickjs::Error::new_from_js_message("step", "StepRecord", e.to_string())
                })?;
                recorder
                    .record_step(&step, duration_ms.max(0.0) as u64)
                    .map_err(|e| rquickjs::Error::Io(std::io::Error::other(e.to_string())))
            },
        )?,
    )?;

    let emitter = Arc::clone(run);
    object.set(
        "event",
        Function::new(ctx.clone(), move |event: String| {
            emitter
                .host_event(&event)
                .map_err(|e| rquickjs::Error::new_from_js_message("event", "step event", e))
        })?,
    )?;

    let canceler = Arc::clone(run);
    object.set(
        "canceled",
        Function::new(ctx.clone(), move || canceler.refuses_steps())?,
    )?;

    object.set(
        "now",
        Function::new(ctx.clone(), || chrono::Utc::now().timestamp_millis() as f64)?,
    )?;
    let clock = Arc::new(AtomicI64::new(run.started_at));
    let reader = Arc::clone(&clock);
    object.set(
        "clock",
        Function::new(ctx.clone(), move || reader.load(Ordering::SeqCst) as f64)?,
    )?;
    object.set(
        "advanceClock",
        Function::new(ctx.clone(), move |ms: f64| {
            clock.fetch_max(ms as i64, Ordering::SeqCst);
        })?,
    )?;
    let seed = run_seed(&run.run_id);
    object.set("seed", Function::new(ctx.clone(), move || seed)?)?;
    let logger = Arc::clone(run);
    object.set(
        "log",
        Function::new(ctx.clone(), move |line: String| {
            logger.events.emit(Event::RunLog {
                line,
                step: logger.key_prefix.clone(),
            });
        })?,
    )?;

    let plan = match &run.test {
        Some(plan) => serde_json::to_string(plan).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };
    object.set(
        "testPlan",
        Function::new(ctx.clone(), move || plan.clone())?,
    )?;
    let stopper = Arc::clone(run);
    object.set(
        "stop",
        Function::new(ctx.clone(), move || {
            stopper.stopped.store(true, Ordering::SeqCst);
        })?,
    )?;

    let evaluator = Arc::clone(run);
    object.set(
        "evaluate",
        Function::new(
            ctx.clone(),
            move |kind: String, expression: String, scope: String| {
                crate::expr::evaluate(&evaluator, &kind, &expression, &scope).map_err(|message| {
                    rquickjs::Error::new_from_js_message("expression", "value", message)
                })
            },
        )?,
    )?;

    object.set(
        "semver",
        Function::new(ctx.clone(), crate::deps_host::semver)?,
    )?;

    let starter = Arc::clone(run);
    let in_flight = Arc::clone(in_flight);
    let sender = sender.clone();
    let next_id = Arc::new(AtomicU32::new(1));
    object.set(
        "start",
        Function::new(ctx.clone(), move |request: String| -> u32 {
            let id = next_id.fetch_add(1, Ordering::SeqCst);
            in_flight.fetch_add(1, Ordering::SeqCst);
            let run = Arc::clone(&starter);
            let sender = sender.clone();
            std::thread::spawn(move || {
                // A task that panics still settles its step, as a failure.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_host_task(&run, &request)
                }))
                .unwrap_or_else(|panic| Err(panic_payload(panic)));
                let _ = sender.send((id, result));
            });
            id
        })?,
    )?;

    ctx.globals().set("__workflowHost", object)
}

/// A seed for the workflow body's random numbers: the same for every
/// execution of one run, different between runs.
fn run_seed(run_id: &str) -> u32 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(run_id.as_bytes());
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// Run the task of one step. `Ok` is the outcome as JSON; `Err` the error
/// payload (`{ name, message }`) as JSON.
pub(crate) fn run_host_task(run: &Arc<RunContext>, request: &str) -> Result<String, String> {
    let request: HostRequest =
        serde_json::from_str(request).map_err(|e| format!("bad task request: {e}"))?;
    let clock = request
        .timeout_ms
        .map(|ms| StepClock::start(Arc::clone(&run.cancel), Duration::from_millis(ms)));
    let task = TaskContext {
        run,
        position: request.position,
        label: request.label,
        cancel: match &clock {
            Some(clock) => Arc::clone(&clock.cancel),
            None => Arc::clone(&run.cancel),
        },
        since: request.since,
        alongside: request.alongside,
    };
    let result = tasks::dispatch(&task, request.task).map_err(|error| {
        // Stopped by its clock, the task reports a cancel; the run goes on.
        let timed_out = clock
            .as_ref()
            .is_some_and(|clock| clock.expired.load(Ordering::SeqCst));
        if timed_out && !run.canceled() {
            tasks::TaskError::Failed(format!(
                "timed out after {}",
                tasks::describe_duration(request.timeout_ms.unwrap_or_default())
            ))
        } else {
            error
        }
    });
    match result {
        Ok(outcome) => serde_json::to_string(&outcome).map_err(|e| e.to_string()),
        Err(error) => {
            match &error {
                tasks::TaskError::Canceled => run.interrupt(Interruption::Canceled),
                tasks::TaskError::AwaitingApproval(message) => {
                    run.interrupt(Interruption::AwaitingApproval(message.clone()))
                }
                // The run's halt already says so.
                tasks::TaskError::Rejected(_) | tasks::TaskError::Failed(_) => {}
            }
            Err(error.to_payload())
        }
    }
}

/// The error payload for a task that panicked.
pub(crate) fn panic_payload(panic: Box<dyn std::any::Any + Send>) -> String {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|message| message.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_string());
    tasks::TaskError::Failed(format!("the step's task panicked: {detail}")).to_payload()
}

/// Run jobs and settle host tasks until the run is over: the body's promise
/// has settled (or cannot any more) and no task is left running.
fn drive<'js>(
    ctx: &Ctx<'js>,
    run: &RunContext,
    promise: Promise<'js>,
    receiver: &mpsc::Receiver<Settlement>,
    in_flight: &AtomicUsize,
    interruptible: &AtomicBool,
) -> Ending {
    let function = |name: &str| ctx.globals().get::<_, Function>(name);
    let (settle, replay_next, close) = match (
        function("__workflowSettle"),
        function("__workflowReplayNext"),
        function("__workflowClose"),
    ) {
        (Ok(settle), Ok(replay_next), Ok(close)) => (settle, replay_next, close),
        _ => return Ending::Failed("the workflow runtime did not load".to_string()),
    };
    // How the body ended, once that is known. Tasks still running then are
    // waited for, so their outcomes are recorded before the run reports.
    let mut body: Option<Ending> = None;
    loop {
        while ctx.execute_pending_job() {}
        if body.is_none() {
            if run.canceled() {
                body = Some(Ending::Canceled);
            } else {
                // Replayed steps settle one at a time, each after the body
                // has reacted to the one before.
                match replay_next.call::<_, bool>(()).catch(ctx) {
                    Ok(true) => continue,
                    Ok(false) => body = settled(ctx, &promise),
                    Err(error) => body = Some(Ending::Failed(error.to_string())),
                }
            }
            if body.is_some() {
                interruptible.store(false, Ordering::SeqCst);
                // No step starts from here on.
                let _ = close.call::<_, ()>(()).catch(ctx);
                continue;
            }
        }
        if in_flight.load(Ordering::SeqCst) == 0 {
            return run.ending(body.unwrap_or_else(|| stalled(run)));
        }
        let (id, result) = match receiver.recv_timeout(CANCEL_POLL) {
            Ok(settlement) => settlement,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return run.ending(Ending::Failed("a host task disappeared".to_string()))
            }
        };
        in_flight.fetch_sub(1, Ordering::SeqCst);
        let (ok, payload) = match result {
            Ok(json) => (true, json),
            Err(error) => (false, error),
        };
        if let Err(error) = settle.call::<_, ()>((id, ok, payload)).catch(ctx) {
            body.get_or_insert(Ending::Failed(error.to_string()));
        }
    }
}

/// The body's promise is pending and nothing can settle it. After a test's
/// `until` step, and after a step that was canceled or waits for an approval,
/// that is expected: steps issued afterwards never start.
pub(crate) fn stalled(run: &RunContext) -> Ending {
    if run.stopped_at().is_some() {
        return Ending::Succeeded(serde_json::Value::Null);
    }
    Ending::Failed("the workflow is waiting on a promise that no step will settle".to_string())
}

/// How the body ended, if its promise has settled.
fn settled<'js>(ctx: &Ctx<'js>, promise: &Promise<'js>) -> Option<Ending> {
    match promise.state() {
        PromiseState::Pending => None,
        PromiseState::Resolved => Some(match promise.result::<Value>() {
            Some(Ok(value)) => match to_json(ctx, value) {
                Ok(value) => Ending::Succeeded(value),
                Err(error) => Ending::Failed(error),
            },
            Some(Err(error)) => Ending::Failed(error.to_string()),
            None => Ending::Succeeded(serde_json::Value::Null),
        }),
        PromiseState::Rejected => Some(rejection(ctx, promise)),
    }
}

/// A rejected body, classified by the error's `name`. The message is
/// `Name: message`, as the Node.js host reports it.
fn rejection<'js>(ctx: &Ctx<'js>, promise: &Promise<'js>) -> Ending {
    let caught = match promise.result::<Value>() {
        Some(Err(error)) => Err::<(), _>(error).catch(ctx).err(),
        _ => None,
    };
    let Some(caught) = caught else {
        return Ending::Failed("the workflow failed".to_string());
    };
    let named = |object: &Object<'js>| -> (String, String) {
        let text = |key: &str| object.get::<_, Option<String>>(key).ok().flatten();
        (
            text("name").unwrap_or_else(|| "Error".to_string()),
            text("message").unwrap_or_default(),
        )
    };
    let object = match &caught {
        rquickjs::CaughtError::Exception(exception) => exception.as_object().clone(),
        rquickjs::CaughtError::Value(value) => match value.as_object() {
            Some(object) => object.clone(),
            None => return Ending::Failed(caught.to_string()),
        },
        rquickjs::CaughtError::Error(_) => return Ending::Failed(caught.to_string()),
    };
    let (name, message) = named(&object);
    match name.as_str() {
        "CanceledError" => Ending::Canceled,
        "AwaitingApprovalError" => Ending::AwaitingApproval(message),
        "RejectedError" => Ending::Rejected(message),
        "WorkflowFailure" => workflow_failure(ctx, &object, message),
        _ => Ending::Failed(format!("{name}: {message}")),
    }
}

/// The ending of a body that `fail()` stopped: its message, output and exit
/// code (`fail()` checked both).
fn workflow_failure<'js>(ctx: &Ctx<'js>, error: &Object<'js>, message: String) -> Ending {
    let output = error
        .get::<_, Value>("output")
        .map_err(|e| e.to_string())
        .and_then(|value| to_json(ctx, value));
    let exit_code = error.get::<_, f64>("exitCode").ok();
    match (output, exit_code) {
        (Ok(output), Some(code)) if code.fract() == 0.0 && (1.0..=125.0).contains(&code) => {
            Ending::FailedWithOutput {
                error: message,
                output,
                exit_code: code as i32,
            }
        }
        _ => Ending::Failed(format!("WorkflowFailure: {message}")),
    }
}

pub(crate) fn to_json<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<serde_json::Value, String> {
    if value.is_undefined() {
        return Ok(serde_json::Value::Null);
    }
    let text = ctx
        .json_stringify(value)
        .catch(ctx)
        .map_err(|e| e.to_string())?
        .map(|s| s.to_string())
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "null".to_string());
    serde_json::from_str(&text).map_err(|e| e.to_string())
}
