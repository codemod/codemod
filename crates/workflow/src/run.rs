//! Runs a built workflow in one QuickJS context.
//!
//! The workflow body runs on the calling thread, with the modules that are
//! deterministic (pure LLRT modules such as `path` and `url`,
//! `codemod/ast-grep`, packages and relative files) and replay-safe globals
//! (`js/body_globals.js`). Steps and `forEachFile` callbacks run in their own
//! isolates (see `tasks::isolate`). Host tasks run on their own
//! threads and report back over a channel; the loop below settles their
//! promises, so steps started together run at the same time. Every step's
//! outcome goes to the run store as soon as it is known.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use butterflow_state::run_store::{Outcome, RunStore, StepRecord, StoreResult};
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::quickjs_adapters::{
    CodemodModuleResolver, QuickJSLoader, QuickJSResolver,
};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::AstGrepModule;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::promise::PromiseState;
use rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Object, Promise, Runtime, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::build::{Built, FLOW_MODULE};
use crate::events::{Event, EventSink};
use crate::interaction::Interaction;
use crate::tasks::{self, TaskContext};
use crate::Services;

const RUNTIME_JS: &str = include_str!("js/workflow.js");
/// The orchestrator shared with the Node.js host.
pub const ORCHESTRATOR_JS: &str = include_str!("js/orchestrator.js");
/// Not under `codemod/` or `codemod:`: those names are the public modules.
pub(crate) const ORCHESTRATOR_MODULE: &str = "codemod-internal:orchestrator";
const BODY_GLOBALS_JS: &str = include_str!("js/body_globals.js");
const BODY_METRICS_JS: &str = include_str!("js/body_metrics.js");
const WORKFLOW_MODULE: &str = "workflow.js";

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

/// Everything one run needs, shared with host task threads.
pub(crate) struct RunContext {
    pub run_id: String,
    pub target: PathBuf,
    /// Directory of the workflow file; relative task paths resolve here.
    pub bundle_dir: PathBuf,
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
    /// Keeps `cancel` following the store's cancel requests; `None` for
    /// nested codemods, which share their parent's flag.
    pub _cancel_watch: Option<CancelWatch>,
}

/// Why a step could not finish.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Interruption {
    /// The run was canceled.
    Canceled,
    /// The step waits for an approval nobody could give now.
    AwaitingApproval(String),
}

/// Sets a run's cancel flag when the store holds a cancel request for it
/// (`codemod workflow cancel`, from any process). Stops when dropped.
pub(crate) struct CancelWatch {
    _stop: mpsc::Sender<()>,
}

impl CancelWatch {
    pub fn start(store: Arc<dyn RunStore>, run_id: String, cancel: Arc<AtomicBool>) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(CANCEL_POLL) {
                if store.cancel_requested(&run_id).unwrap_or(false) {
                    cancel.store(true, Ordering::SeqCst);
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
    /// The context of a codemod nested in this run's step at `position`: its
    /// own workflow, params and step history, everything else shared.
    pub(crate) fn child(
        &self,
        position: u64,
        bundle_dir: PathBuf,
        params: serde_json::Value,
        callbacks: BTreeMap<String, String>,
        events: Arc<dyn EventSink>,
        identity: String,
    ) -> Self {
        let mut chain = self.chain.clone();
        chain.push(identity);
        Self {
            run_id: format!("{}~{position}", self.run_id),
            target: self.target.clone(),
            bundle_dir,
            params,
            dry_run: self.dry_run,
            started_at: self.started_at,
            callbacks,
            store: Arc::clone(&self.store),
            events,
            interaction: Arc::clone(&self.interaction),
            services: Arc::clone(&self.services),
            cancel: Arc::clone(&self.cancel),
            commit_lock: Arc::clone(&self.commit_lock),
            chain,
            test: None,
            stopped: AtomicBool::new(false),
            interruption: Mutex::new(None),
            _cancel_watch: None,
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

    /// How the run ended, given how its body ended. A step that was canceled
    /// or waits for an approval decides it even when the body caught the
    /// error and carried on; a test's `until` step turns success into
    /// stopped.
    pub fn ending(&self, body: Ending) -> Ending {
        let interruption = self
            .interruption
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match (interruption, body) {
            (Some(Interruption::Canceled), _) | (_, Ending::Canceled) => Ending::Canceled,
            (Some(Interruption::AwaitingApproval(message)), _)
            | (_, Ending::AwaitingApproval(message)) => Ending::AwaitingApproval(message),
            // Stopping a run mid-step surfaces as that step's failure.
            (None, Ending::Failed(_)) if self.canceled() => Ending::Canceled,
            (None, Ending::Succeeded(output)) => match self.stopped_at() {
                Some(label) => Ending::Stopped(label),
                None => Ending::Succeeded(output),
            },
            (None, body) => body,
        }
    }

    /// Persist one step's outcome and report it.
    pub fn record_step(&self, step: &StepRecord, duration_ms: u64) -> StoreResult<()> {
        self.store.put_step(&self.run_id, step)?;
        self.events.emit(Event::StepFinished {
            position: step.position,
            label: step.label.clone(),
            status: step.status,
            error: step.error.clone(),
            duration_ms,
        });
        Ok(())
    }

    /// Report what the orchestrator says about a step:
    /// `{ type: "stepStarted" | "stepReplayed", position, label }`.
    pub fn host_event(&self, event: &str) -> Result<(), String> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "camelCase")]
        enum HostEvent {
            StepStarted { position: u64, label: String },
            StepReplayed { position: u64, label: String },
        }
        let event = serde_json::from_str(event).map_err(|e| format!("bad step event: {e}"))?;
        self.events.emit(match event {
            HostEvent::StepStarted { position, label } => Event::StepStarted { position, label },
            HostEvent::StepReplayed { position, label } => Event::StepReplayed { position, label },
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
    Canceled,
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
            Self::Failed(_) => "failed",
            Self::Canceled => "canceled",
            Self::AwaitingApproval(_) => "awaiting-approval",
            Self::Stopped(_) => "stopped",
        }
    }

    /// What the run's record keeps of it. Only test runs stop early, and
    /// they are never resumed, so stopped is stored as succeeded.
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Succeeded(output) => Outcome::Succeeded(output.clone()),
            Self::Failed(error) => Outcome::Failed(error.clone()),
            Self::Canceled => Outcome::Canceled,
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
struct HostRequest {
    position: u64,
    label: String,
    task: serde_json::Value,
    #[serde(default)]
    state: serde_json::Map<String, serde_json::Value>,
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
            // The body's codemod modules: `codemod/<name>`, or `codemod:<name>`.
            CodemodModuleResolver::new(&["ast-grep", "workflow", "metrics"]),
            llrt_resolver,
            BuiltinResolver::default().with_module(ORCHESTRATOR_MODULE),
            QuickJSResolver::new(Arc::new(packages)),
        ),
        (
            llrt_loader.with_module("codemod/ast-grep", AstGrepModule),
            BuiltinLoader::default()
                .with_module(FLOW_MODULE, RUNTIME_JS)
                .with_module(ORCHESTRATOR_MODULE, ORCHESTRATOR_JS)
                .with_module("codemod/metrics", BODY_METRICS_JS),
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
            install_host(&ctx, &run, &recorded, &in_flight, &sender)?;
            ctx.eval::<(), _>(BODY_GLOBALS_JS)?;
            // Load the runtime even when the workflow never imports it.
            Module::import(&ctx, FLOW_MODULE)?.finish::<()>()?;
            let declared = Module::declare(ctx.clone(), WORKFLOW_MODULE, built.main.as_str())?;
            let (module, promise) = declared.eval()?;
            promise.finish::<()>()?;
            let main: Function = module.get("default")?;
            let input = json!({
                "params": run.params,
                "target": run.target,
                "dryRun": run.dry_run,
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
            logger.events.emit(Event::RunLog { line });
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
    let task = TaskContext {
        run,
        position: request.position,
        label: request.label,
        state: request.state,
    };
    match tasks::dispatch(&task, request.task) {
        Ok(outcome) => serde_json::to_string(&outcome).map_err(|e| e.to_string()),
        Err(error) => {
            match &error {
                tasks::TaskError::Canceled => run.interrupt(Interruption::Canceled),
                tasks::TaskError::AwaitingApproval(message) => {
                    run.interrupt(Interruption::AwaitingApproval(message.clone()))
                }
                tasks::TaskError::Failed(_) => {}
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
    let (name, message) = match &caught {
        rquickjs::CaughtError::Exception(exception) => named(exception.as_object()),
        rquickjs::CaughtError::Value(value) => match value.as_object() {
            Some(object) => named(object),
            None => return Ending::Failed(caught.to_string()),
        },
        rquickjs::CaughtError::Error(_) => return Ending::Failed(caught.to_string()),
    };
    match name.as_str() {
        "CanceledError" => Ending::Canceled,
        "AwaitingApprovalError" => Ending::AwaitingApproval(message),
        _ => Ending::Failed(format!("{name}: {message}")),
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
