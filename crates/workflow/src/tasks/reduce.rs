//! `jssg(path, { reduce, initial })`: each file's `data` is folded into one
//! result as the file finishes, in an isolate of its own, so the step never
//! holds every file's data at once.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use codemod_deps::Registry;
use rquickjs::{async_with, CatchResultExt, Function, Module, Promise};
use serde::Deserialize;
use serde_json::Value;

use super::isolate::{self, module_name, StepIsolates};
use super::{block_on, TaskContext};

#[derive(Deserialize, Clone)]
pub(crate) struct Reduce {
    callback: String,
    #[serde(default)]
    captures: Value,
    initial: Value,
}

/// What the reducer's thread needs, taken from the task before it starts.
pub(crate) struct Fold {
    options: codemod_sandbox::WorkerOptions<codemod_sandbox::sandbox::resolvers::OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
    name: String,
    module: String,
    captures: String,
    initial: String,
    workflow_path: std::path::PathBuf,
}

impl Fold {
    pub fn new(task: &TaskContext<'_>, reduce: &Reduce) -> Result<Self, String> {
        let module = task
            .run
            .callbacks
            .get(&reduce.callback)
            .ok_or_else(|| format!("unknown reducer '{}'", reduce.callback))?
            .clone();
        let isolates = StepIsolates::new(task)?;
        Ok(Self {
            options: isolates.worker_options(task, None),
            registry: isolates.registry.clone(),
            name: module_name(task, &reduce.callback),
            module,
            captures: serde_json::to_string(&reduce.captures).map_err(|e| e.to_string())?,
            initial: serde_json::to_string(&reduce.initial).map_err(|e| e.to_string())?,
            workflow_path: task.run.workflow_path.clone(),
        })
    }

    /// Fold every `{ file, data }` (as JSON) that arrives until the sender
    /// is dropped; the result, or why the reducer failed.
    pub fn run(self, entries: Receiver<String>) -> Result<Value, String> {
        let Fold {
            options,
            registry,
            name,
            module,
            captures,
            initial,
            workflow_path,
        } = self;
        let json = block_on(async move {
            let sandbox = isolate::start(options, registry).await?;
            let json = async_with!(sandbox.context => |ctx| {
                let run = async {
                    let declared = Module::declare(ctx.clone(), name.as_str(), module.as_str())?;
                    crate::run::set_import_meta(&declared, &workflow_path)?;
                    let (evaluated, promise) = declared.eval()?;
                    promise.into_future::<()>().await?;
                    let init: Function = evaluated.get("__init")?;
                    init.call::<_, ()>((captures.as_str(), None::<&str>))?;
                    let start: Function = evaluated.get("__reduceStart")?;
                    start.call::<_, ()>((initial.as_str(),))?;
                    let one: Function = evaluated.get("__reduceOne")?;
                    while let Ok(entry) = entries.recv() {
                        let promise: Promise = one.call((entry.as_str(),))?;
                        promise.into_future::<()>().await?;
                    }
                    let end: Function = evaluated.get("__reduceEnd")?;
                    end.call::<_, String>(())
                };
                run.await.catch(&ctx).map_err(|e| format!("the reducer failed: {e}"))
            })
            .await;
            // A `failStep` the reducer caught still fails the step.
            if let Some(failure) = sandbox.take_side_effects().failure {
                return Err(failure.message);
            }
            json
        })?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}
