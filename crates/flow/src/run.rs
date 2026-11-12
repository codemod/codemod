//! Runs a built workflow in one QuickJS context.
//!
//! The workflow body runs on the calling thread. Host tasks (today only
//! `forEachFile`) run on their own threads and report back over a channel; the
//! loop below settles their promises, so steps started together run at the
//! same time.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::promise::PromiseState;
use rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Object, Promise, Runtime, Value};
use serde::Deserialize;

use crate::build::{Built, FLOW_MODULE};
use crate::files::{self, ForEachFile, Selector};

const FLOW_JS: &str = include_str!("js/flow.js");
const WORKFLOW_MODULE: &str = "workflow.js";

pub struct RunOptions {
    /// Directory the workflow operates on.
    pub target: PathBuf,
    /// Passed to the workflow's default export.
    pub input: serde_json::Value,
    /// Steps recorded by an earlier run of the same workflow.
    pub history: Vec<serde_json::Value>,
    /// When set, the history is rewritten here after every step, so it
    /// survives a crash.
    pub history_path: Option<PathBuf>,
    /// Run every step but write nothing: no file changes, no history file.
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct RunOutcome {
    /// The workflow's return value, or the error that ended it.
    pub result: Result<serde_json::Value, String>,
    /// Every step's outcome, ordered by position.
    pub history: Vec<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("could not start the JavaScript runtime: {0}")]
    Runtime(String),
}

struct Host {
    target: PathBuf,
    callbacks: BTreeMap<String, String>,
    dry_run: bool,
    commit_lock: Mutex<()>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Task {
    #[serde(rename_all = "camelCase")]
    ForEachFile {
        selector: Selector,
        callback: String,
        captures: serde_json::Value,
    },
}

type Settlement = (u32, Result<String, String>);

pub fn run(built: &Built, options: RunOptions) -> Result<RunOutcome, RunError> {
    let runtime = Runtime::new().map_err(|e| RunError::Runtime(e.to_string()))?;
    let context = Context::full(&runtime).map_err(|e| RunError::Runtime(e.to_string()))?;
    runtime.set_loader(
        BuiltinResolver::default().with_module(FLOW_MODULE),
        BuiltinLoader::default().with_module(FLOW_MODULE, FLOW_JS),
    );

    let history = Arc::new(Mutex::new(options.history.clone()));
    let history_path = if options.dry_run {
        None
    } else {
        options.history_path.clone()
    };
    let host = Arc::new(Host {
        target: options.target.clone(),
        callbacks: built.callbacks.clone(),
        dry_run: options.dry_run,
        commit_lock: Mutex::new(()),
    });
    let in_flight = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel::<Settlement>();

    let result = context.with(|ctx| {
        let started = (|| -> rquickjs::Result<Value<'_>> {
            install_host(
                &ctx,
                &history,
                history_path.clone(),
                &host,
                &in_flight,
                &sender,
            )?;
            let declared = Module::declare(ctx.clone(), WORKFLOW_MODULE, built.main.as_str())?;
            let (module, promise) = declared.eval()?;
            promise.finish::<()>()?;
            let main: Function = module.get("default")?;
            let input =
                ctx.json_parse(serde_json::to_string(&options.input).unwrap_or_default())?;
            main.call((input,))
        })()
        .catch(&ctx)
        .map_err(|e| e.to_string());

        let value = match started {
            Ok(value) => value,
            Err(error) => return Err(error),
        };
        let Some(promise) = value.as_promise().cloned() else {
            return to_json(&ctx, value);
        };
        drive(&ctx, promise, &receiver, &in_flight)
    });

    let history = history.lock().map(|h| h.clone()).unwrap_or_default();
    Ok(RunOutcome { result, history })
}

fn install_host<'js>(
    ctx: &Ctx<'js>,
    history: &Arc<Mutex<Vec<serde_json::Value>>>,
    history_path: Option<PathBuf>,
    host: &Arc<Host>,
    in_flight: &Arc<AtomicUsize>,
    sender: &mpsc::Sender<Settlement>,
) -> rquickjs::Result<()> {
    let object = Object::new(ctx.clone())?;

    let recorded = serde_json::to_string(&*history.lock().unwrap_or_else(|e| e.into_inner()))
        .unwrap_or_else(|_| "[]".to_string());
    object.set(
        "history",
        Function::new(ctx.clone(), move || recorded.clone())?,
    )?;

    let history = Arc::clone(history);
    object.set(
        "record",
        Function::new(ctx.clone(), move |entry: String| -> rquickjs::Result<()> {
            record(&history, history_path.as_deref(), &entry).map_err(rquickjs::Error::Io)
        })?,
    )?;

    let host = Arc::clone(host);
    let in_flight = Arc::clone(in_flight);
    let sender = sender.clone();
    let next_id = Arc::new(AtomicU32::new(1));
    object.set(
        "start",
        Function::new(ctx.clone(), move |task: String| -> u32 {
            let id = next_id.fetch_add(1, Ordering::SeqCst);
            in_flight.fetch_add(1, Ordering::SeqCst);
            let host = Arc::clone(&host);
            let sender = sender.clone();
            std::thread::spawn(move || {
                let result = run_task(&host, &task);
                let _ = sender.send((id, result));
            });
            id
        })?,
    )?;

    ctx.globals().set("__flowHost", object)
}

/// Run jobs and settle host tasks until the workflow's promise settles.
fn drive<'js>(
    ctx: &Ctx<'js>,
    promise: Promise<'js>,
    receiver: &mpsc::Receiver<Settlement>,
    in_flight: &AtomicUsize,
) -> Result<serde_json::Value, String> {
    let settle: Function = ctx
        .globals()
        .get("__flowSettle")
        .map_err(|e| e.to_string())?;
    loop {
        while ctx.execute_pending_job() {}
        match promise.state() {
            PromiseState::Resolved => {
                let value: Value = promise
                    .result()
                    .unwrap_or(Ok(Value::new_undefined(ctx.clone())))
                    .map_err(|e| e.to_string())?;
                return to_json(ctx, value);
            }
            PromiseState::Rejected => {
                let error = promise
                    .result::<Value>()
                    .and_then(|r| r.catch(ctx).err().map(|e| e.to_string()))
                    .unwrap_or_else(|| "the workflow failed".to_string());
                return Err(error);
            }
            PromiseState::Pending => {}
        }
        if in_flight.load(Ordering::SeqCst) == 0 {
            return Err(
                "the workflow is waiting on a promise that no step will settle".to_string(),
            );
        }
        let (id, result) = receiver
            .recv()
            .map_err(|_| "a host task disappeared".to_string())?;
        in_flight.fetch_sub(1, Ordering::SeqCst);
        let (ok, payload) = match result {
            Ok(json) => (true, json),
            Err(error) => (false, error),
        };
        settle
            .call::<_, ()>((id, ok, payload))
            .catch(ctx)
            .map_err(|e| e.to_string())?;
    }
}

fn run_task(host: &Host, task: &str) -> Result<String, String> {
    let task: Task = serde_json::from_str(task).map_err(|e| format!("bad task: {e}"))?;
    match task {
        Task::ForEachFile {
            selector,
            callback,
            captures,
        } => {
            let module = host
                .callbacks
                .get(&callback)
                .ok_or_else(|| format!("unknown callback '{callback}'"))?;
            let captures = serde_json::to_string(&captures).map_err(|e| e.to_string())?;
            let result = files::run(ForEachFile {
                target: &host.target,
                module,
                selector: &selector,
                captures: &captures,
                dry_run: host.dry_run,
                commit_lock: &host.commit_lock,
            })?;
            serde_json::to_string(&result).map_err(|e| e.to_string())
        }
    }
}

fn record(
    history: &Mutex<Vec<serde_json::Value>>,
    path: Option<&Path>,
    entry: &str,
) -> std::io::Result<()> {
    let entry: serde_json::Value = serde_json::from_str(entry)?;
    let position = entry.get("position").cloned();
    let mut history = history.lock().unwrap_or_else(|e| e.into_inner());
    match history
        .iter_mut()
        .find(|e| e.get("position") == position.as_ref())
    {
        Some(existing) => *existing = entry,
        None => history.push(entry),
    }
    history.sort_by_key(|e| e.get("position").and_then(|p| p.as_u64()).unwrap_or(0));
    if let Some(path) = path {
        fs::write(path, serde_json::to_string_pretty(&*history)?)?;
    }
    Ok(())
}

fn to_json<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<serde_json::Value, String> {
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
