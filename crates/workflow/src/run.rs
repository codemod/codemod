//! Runs a built workflow in one QuickJS context.
//!
//! The workflow body runs on the calling thread, with the modules that are
//! deterministic (pure LLRT modules such as `path` and `url`,
//! `codemod:ast-grep`, packages and relative files) and replay-safe globals
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

use butterflow_state::run_store::{RunStore, StepRecord};
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::AstGrepModule;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::promise::PromiseState;
use rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Object, Promise, Runtime, Value};
use serde::Deserialize;
use serde_json::json;

use crate::build::{Built, FLOW_MODULE};
use crate::events::{Event, EventSink};
use crate::interaction::Interaction;
use crate::tasks::{self, TaskContext};
use crate::Services;

const RUNTIME_JS: &str = include_str!("js/workflow.js");
/// The orchestrator shared with the Node.js host.
pub const ORCHESTRATOR_JS: &str = include_str!("js/orchestrator.js");
const ORCHESTRATOR_MODULE: &str = "codemod:workflow/orchestrator";
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
    pub cancel: Arc<AtomicBool>,
    /// Serializes file commits from steps running at the same time.
    pub commit_lock: Mutex<()>,
    /// Codemod packages this run is nested in, for cycle detection.
    pub chain: Vec<String>,
    /// Steps to stop at or to stub, when the run is a test.
    pub test: Option<TestPlan>,
    /// Set once the test plan's `until` step has started.
    pub stopped: AtomicBool,
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
    pub fn canceled(&self) -> bool {
        if self.cancel.load(Ordering::SeqCst) {
            return true;
        }
        if self.store.cancel_requested(&self.run_id).unwrap_or(false) {
            self.cancel.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ending {
    Succeeded(serde_json::Value),
    Failed(String),
    Canceled,
    /// Stopped at a manual approval nobody could give.
    AwaitingApproval(String),
    /// A test run stopped after its `until` step (named here).
    Stopped(String),
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

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum HostEvent {
    StepStarted { position: u64, label: String },
    StepReplayed { position: u64, label: String },
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
            llrt_resolver.add_name("codemod:ast-grep"),
            BuiltinResolver::default()
                .with_module(FLOW_MODULE)
                .with_module(ORCHESTRATOR_MODULE)
                .with_module("codemod:metrics"),
            QuickJSResolver::new(Arc::new(packages)),
        ),
        (
            llrt_loader.with_module("codemod:ast-grep", AstGrepModule),
            BuiltinLoader::default()
                .with_module(FLOW_MODULE, RUNTIME_JS)
                .with_module(ORCHESTRATOR_MODULE, ORCHESTRATOR_JS)
                .with_module("codemod:metrics", BODY_METRICS_JS),
            QuickJSLoader,
        ),
    );
    let interrupt = Arc::clone(&run.cancel);
    runtime.set_interrupt_handler(Some(Box::new(move || interrupt.load(Ordering::SeqCst))));

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
            Err(error) if run.canceled() => {
                let _ = error;
                return Ending::Canceled;
            }
            Err(error) => return Ending::Failed(error),
        };
        let Some(promise) = value.as_promise().cloned() else {
            return match to_json(&ctx, value) {
                Ok(value) => Ending::Succeeded(value),
                Err(error) => Ending::Failed(error),
            };
        };
        drive(&ctx, &run, promise, &receiver, &in_flight)
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
                    .store
                    .put_step(&recorder.run_id, &step)
                    .map_err(|e| rquickjs::Error::Io(std::io::Error::other(e.to_string())))?;
                recorder.events.emit(Event::StepFinished {
                    position: step.position,
                    label: step.label.clone(),
                    status: step.status,
                    error: step.error.clone(),
                    duration_ms: duration_ms.max(0.0) as u64,
                });
                Ok(())
            },
        )?,
    )?;

    let emitter = Arc::clone(run);
    object.set(
        "event",
        Function::new(
            ctx.clone(),
            move |event: String| match serde_json::from_str::<HostEvent>(&event) {
                Ok(HostEvent::StepStarted { position, label }) => {
                    emitter.events.emit(Event::StepStarted { position, label })
                }
                Ok(HostEvent::StepReplayed { position, label }) => {
                    emitter.events.emit(Event::StepReplayed { position, label })
                }
                Err(_) => {}
            },
        )?,
    )?;

    let canceler = Arc::clone(run);
    object.set(
        "canceled",
        Function::new(ctx.clone(), move || canceler.canceled())?,
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
                let result = run_host_task(&run, &request);
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
        Err(error) => Err(error.to_payload()),
    }
}

/// Run jobs and settle host tasks until the workflow's promise settles.
fn drive<'js>(
    ctx: &Ctx<'js>,
    run: &RunContext,
    promise: Promise<'js>,
    receiver: &mpsc::Receiver<Settlement>,
    in_flight: &AtomicUsize,
) -> Ending {
    let settle: Function = match ctx.globals().get("__workflowSettle") {
        Ok(settle) => settle,
        Err(error) => return Ending::Failed(error.to_string()),
    };
    loop {
        while ctx.execute_pending_job() {}
        match promise.state() {
            PromiseState::Resolved => {
                let value: Value = match promise.result() {
                    Some(Ok(value)) => value,
                    Some(Err(error)) => return Ending::Failed(error.to_string()),
                    None => Value::new_undefined(ctx.clone()),
                };
                if let Some(label) = stopped_at(run) {
                    return Ending::Stopped(label);
                }
                return match to_json(ctx, value) {
                    Ok(value) => Ending::Succeeded(value),
                    Err(error) => Ending::Failed(error),
                };
            }
            PromiseState::Rejected => return rejection(ctx, &promise, run),
            PromiseState::Pending => {}
        }
        if run.canceled() {
            return Ending::Canceled;
        }
        if in_flight.load(Ordering::SeqCst) == 0 {
            // Steps after a test's `until` step never settle.
            if let Some(label) = stopped_at(run) {
                return Ending::Stopped(label);
            }
            return Ending::Failed(
                "the workflow is waiting on a promise that no step will settle".to_string(),
            );
        }
        let (id, result) = match receiver.recv_timeout(CANCEL_POLL) {
            Ok(settlement) => settlement,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Ending::Failed("a host task disappeared".to_string())
            }
        };
        in_flight.fetch_sub(1, Ordering::SeqCst);
        let (ok, payload) = match result {
            Ok(json) => (true, json),
            Err(error) => (false, error),
        };
        if let Err(error) = settle.call::<_, ()>((id, ok, payload)).catch(ctx) {
            return Ending::Failed(error.to_string());
        }
    }
}

fn stopped_at(run: &RunContext) -> Option<String> {
    if !run.stopped.load(Ordering::SeqCst) {
        return None;
    }
    run.test.as_ref().and_then(|plan| plan.until.clone())
}

/// Classify a rejected workflow promise by the error's `name`.
fn rejection<'js>(ctx: &Ctx<'js>, promise: &Promise<'js>, run: &RunContext) -> Ending {
    let caught = match promise.result::<Value>() {
        Some(Err(error)) => Err::<(), _>(error).catch(ctx).err(),
        _ => None,
    };
    let Some(caught) = caught else {
        return Ending::Failed("the workflow failed".to_string());
    };
    let (name, message) = match &caught {
        rquickjs::CaughtError::Exception(exception) => (
            exception
                .as_object()
                .get::<_, Option<String>>("name")
                .ok()
                .flatten()
                .unwrap_or_default(),
            exception.message().unwrap_or_default(),
        ),
        rquickjs::CaughtError::Value(value) => (
            value
                .as_object()
                .and_then(|o| o.get::<_, Option<String>>("name").ok().flatten())
                .unwrap_or_default(),
            String::new(),
        ),
        rquickjs::CaughtError::Error(_) => (String::new(), String::new()),
    };
    match name.as_str() {
        "CanceledError" => Ending::Canceled,
        "AwaitingApprovalError" => Ending::AwaitingApproval(message),
        _ if run.canceled() => Ending::Canceled,
        _ => Ending::Failed(caught.to_string()),
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
