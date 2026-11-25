//! Function steps: `step(() => ...)` and `step(label, () => ...)`.
//!
//! The build extracts the function like a `forEachFile` callback. It runs
//! once, in its own isolate, where the clock, randomness, network and files
//! are real (as the run's capabilities allow); its result is recorded and
//! replayed on resume.

use std::collections::HashMap;
use std::path::PathBuf;

use butterflow_core::jssg_runner::JssgFileChange;
use codemod_sandbox::sandbox::engine::ExecutionResult;
use codemod_sandbox::Worker;
use rquickjs::{async_with, CatchResultExt, Function, Module, Promise};
use serde::Deserialize;
use serde_json::Value;

use super::isolate::{self, module_name, StepIsolates};
use super::{block_on, changes, TaskContext, TaskOutcome};

#[derive(Deserialize)]
struct Request {
    callback: String,
    captures: Value,
}

#[derive(Deserialize)]
struct Outcome {
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<String>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Value) -> Result<TaskOutcome, String> {
    let request: Request =
        serde_json::from_value(request).map_err(|e| format!("bad function step: {e}"))?;
    let called = call_with(task, &request.callback, &request.captures, None, true);
    if let Some(usage) = task.run.assess.take(task.position) {
        task.log(usage.describe());
    }
    task.run.assess.finish_step(task.position);
    let (result, changes) = called?;
    let planned = changes::plan(task, &changes)?;
    changes::commit(task, &planned, task.run.dry_run)?;
    Ok(TaskOutcome::new(result))
}

/// Run an extracted function once, in its own isolate: with no arguments
/// (a function step), or with `args`, a JSON array (a shard function).
/// Gives its JSON result and the files it changed with `jssgTransform`.
pub(crate) fn call(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    args: Option<&Value>,
) -> Result<(Value, Vec<JssgFileChange>), String> {
    call_with(task, callback, captures, args, false)
}

/// [`call`], where a function step (`assess`) may call `assess()`.
fn call_with(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    args: Option<&Value>,
    assess: bool,
) -> Result<(Value, Vec<JssgFileChange>), String> {
    let module = task
        .run
        .callbacks
        .get(callback)
        .ok_or_else(|| format!("unknown function '{callback}'"))?;
    let captures = serde_json::to_string(captures).map_err(|e| e.to_string())?;
    let args = args.map(Value::to_string);
    let isolates = StepIsolates::new(task)?;
    let options = isolates.worker_options(task, None);
    let name = module_name(task, callback);
    let workflow_path = task.run.workflow_path.clone();
    let registry = isolates.registry.clone();
    let assess = assess.then(|| crate::assess::AssessHandle::new(task));

    let (json, failure, changes) = block_on(async move {
        let worker = isolate::start(options, registry, assess).await?;
        let sandbox = &worker;
        let json: Result<String, String> = async_with!(sandbox.context => |ctx| {
            let run = async {
                let declared = Module::declare(ctx.clone(), name.as_str(), module.as_str())?;
                crate::run::set_import_meta(&declared, &workflow_path)?;
                let (evaluated, promise) = declared.eval()?;
                promise.into_future::<()>().await?;
                let init: Function = evaluated.get("__init")?;
                init.call::<_, ()>((captures.as_str(), None::<&str>))?;
                let promise: Promise = match &args {
                    None => evaluated.get::<_, Function>("__runStep")?.call(())?,
                    Some(args) => evaluated.get::<_, Function>("__runCall")?.call((args.as_str(),))?,
                };
                promise.into_future::<String>().await
            };
            run.await.catch(&ctx).map_err(|e| e.to_string())
        })
        .await;
        let (failure, changes) = side_effects(sandbox);
        // Clears timers the code left running, so the runtime can be freed.
        worker.shutdown(isolate::SHUTDOWN_GRACE).await;
        Ok::<_, String>((json?, failure, changes))
    })?;

    let outcome: Outcome = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    if let Some(message) = failure {
        return Err(message);
    }
    if !outcome.ok {
        return Err(outcome
            .error
            .unwrap_or_else(|| "the function failed".to_string()));
    }
    Ok((outcome.result, changes))
}

/// What a step's isolate did besides returning: a failure it reported with
/// `failStep`, and the files it changed with `jssgTransform`, renames
/// included. Shared by function steps and shard functions.
pub(crate) fn side_effects(sandbox: &Worker) -> (Option<String>, Vec<JssgFileChange>) {
    let effects = sandbox.take_side_effects();
    let failure = effects.failure.map(|failure| failure.message);
    let mut renames: HashMap<PathBuf, PathBuf> = effects
        .other_files
        .into_iter()
        .filter_map(|change| match change.result {
            ExecutionResult::Modified(modified) => {
                let path = change.path.canonicalize().unwrap_or(change.path);
                modified.rename_to.map(|to| (path, to))
            }
            _ => None,
        })
        .collect();
    let mut changes: Vec<JssgFileChange> = sandbox
        .changed_files()
        .into_iter()
        .map(|(path, original, content)| JssgFileChange {
            rename_to: renames.remove(&path),
            path,
            original,
            content,
        })
        .collect();
    // Files only renamed: their text as the step found it (in a dry run,
    // as earlier steps left it).
    for (path, to) in renames {
        let text = match sandbox.pending_files() {
            Some(pending) => pending.take(&path).map(|(text, _)| text).ok(),
            None => std::fs::read_to_string(&path).ok(),
        };
        if let Some(text) = text {
            changes.push(JssgFileChange {
                path,
                original: text.clone(),
                content: text,
                rename_to: Some(to),
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    (failure, changes)
}
