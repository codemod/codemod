//! Runs a built workflow in one QuickJS context.
//!
//! The workflow body runs on the calling thread. Host tasks run on their own
//! threads and report back over a channel; the loop below settles their
//! promises, so steps started together run at the same time. Every step's
//! outcome goes to the run store as soon as it is known.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use butterflow_state::run_store::{RunStore, StepRecord};
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
const WORKFLOW_MODULE: &str = "workflow.js";
const CANCEL_POLL: Duration = Duration::from_millis(250);

/// Everything one run needs, shared with host task threads.
pub(crate) struct RunContext {
    pub run_id: String,
    pub target: PathBuf,
    /// Directory of the workflow file; relative task paths resolve here.
    pub bundle_dir: PathBuf,
    pub params: serde_json::Value,
    pub dry_run: bool,
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
enum HostEvent {
    StepStarted { position: u64, label: String },
    StepReplayed { position: u64, label: String },
}

type Settlement = (u32, Result<String, String>);

pub(crate) fn execute(built: &Built, run: Arc<RunContext>) -> Result<Ending, RunError> {
    let recorded = run.store.load_steps(&run.run_id)?;

    let runtime = Runtime::new().map_err(|e| RunError::Runtime(e.to_string()))?;
    let context = Context::full(&runtime).map_err(|e| RunError::Runtime(e.to_string()))?;
    runtime.set_loader(
        BuiltinResolver::default().with_module(FLOW_MODULE),
        BuiltinLoader::default().with_module(FLOW_MODULE, RUNTIME_JS),
    );
    let interrupt = Arc::clone(&run.cancel);
    runtime.set_interrupt_handler(Some(Box::new(move || interrupt.load(Ordering::SeqCst))));

    let in_flight = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel::<Settlement>();

    let ending = context.with(|ctx| {
        let started = (|| -> rquickjs::Result<Value<'_>> {
            install_host(&ctx, &run, &recorded, &in_flight, &sender)?;
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

    let evaluator = Arc::clone(run);
    object.set(
        "evaluate",
        Function::new(
            ctx.clone(),
            move |kind: String, expression: String, scope: String| {
                crate::expr::evaluate(&evaluator, &kind, &expression, &scope)
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

fn run_host_task(run: &Arc<RunContext>, request: &str) -> Result<String, String> {
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
