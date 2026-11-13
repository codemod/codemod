//! Function steps: `step(() => ...)` and `step(label, () => ...)`.
//!
//! The build extracts the function like a `forEachFile` callback. It runs
//! once, in its own isolate, where the clock, randomness, network and files
//! are real (as the run's capabilities allow); its result is recorded and
//! replayed on resume.

use rquickjs::{async_with, CatchResultExt, Function, Module, Promise};
use serde::Deserialize;
use serde_json::Value;

use super::isolate::{self, module_name, StepIsolates};
use super::{block_on, TaskContext, TaskOutcome};

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
    let module = task
        .run
        .callbacks
        .get(&request.callback)
        .ok_or_else(|| format!("unknown function step '{}'", request.callback))?;
    let captures = serde_json::to_string(&request.captures).map_err(|e| e.to_string())?;
    let isolates = StepIsolates::new(task)?;
    let options = isolates.worker_options(task, None);
    let name = module_name(task, &request.callback);
    let registry = isolates.registry.clone();

    let json = block_on(async move {
        let sandbox = isolate::start(options, registry).await?;
        let sandbox = &sandbox;
        async_with!(sandbox.context => |ctx| {
            let run = async {
                let declared = Module::declare(ctx.clone(), name.as_str(), module.as_str())?;
                let (evaluated, promise) = declared.eval()?;
                promise.into_future::<()>().await?;
                let init: Function = evaluated.get("__init")?;
                init.call::<_, ()>((captures.as_str(), None::<&str>))?;
                let run_step: Function = evaluated.get("__runStep")?;
                let promise: Promise = run_step.call(())?;
                promise.into_future::<String>().await
            };
            run.await.catch(&ctx).map_err(|e| e.to_string())
        })
        .await
    })?;

    let outcome: Outcome = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    isolates.metrics(task);
    if !outcome.ok {
        return Err(outcome
            .error
            .unwrap_or_else(|| "the step failed".to_string()));
    }
    Ok(TaskOutcome::new(outcome.result).with_state(isolates.state_changes()))
}
