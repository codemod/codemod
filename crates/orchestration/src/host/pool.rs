//! The transform pool: threads that each own a QuickJS runtime and context
//! (`codemod:ast-grep` and the other transform built-ins, no orchestration
//! primitives) and run whole chunks of a batch the bundle's context planned
//! (`jssgTransformRange`). A worker parses each file with the shared
//! [`BatchPlan`], calls the transform, and converts the result exactly as
//! the in-context loop does (`execution/jssg-batch.ts`), replying with JSON:
//! `{ "results": [{ "outcome": FileOutcome } | { "error": "JSSG failed for
//! '<path>': ..." }] }`, or an error when the chunk cannot run at all (the
//! module does not load, a file does not parse).
//!
//! Each worker evaluates an artifact once, so module-level state is per
//! worker.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::jssg_batch::BatchPlan;
use codemod_sandbox::sandbox::engine::embedded::{
    begin_call, configure_runtime, end_call, install_context, root_value,
};
use codemod_sandbox::sandbox::engine::FileChange;
use rquickjs::{
    async_with, AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Function, Object, Promise, Value,
};
use tokio::sync::oneshot;

use crate::host::jssg::finish_text;
use crate::host::modules::ModuleRegistry;
use crate::host::native::throw;

/// One chunk: files `start ..< start + contents.len()` of `plan`.
pub struct Job {
    pub plan: Arc<BatchPlan>,
    pub specifier: String,
    pub source: String,
    pub start: usize,
    pub paths: Vec<String>,
    pub contents: Vec<String>,
    pub reply: oneshot::Sender<Result<String, String>>,
}

pub struct Pool {
    jobs: Sender<Job>,
}

/// Workers per run: `CODEMOD_JSSG_WORKERS`, else the core count capped at 4.
pub fn size() -> usize {
    std::env::var("CODEMOD_JSSG_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&workers| workers >= 1)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|cores| cores.get().min(4))
                .unwrap_or(1)
        })
}

impl Pool {
    pub fn new(target: PathBuf) -> Self {
        let (jobs, queue) = channel::<Job>();
        let queue = Arc::new(Mutex::new(queue));
        for _ in 0..size() {
            let queue = Arc::clone(&queue);
            let target = target.clone();
            std::thread::spawn(move || work(target, queue));
        }
        Self { jobs }
    }

    pub fn run(&self, job: Job) {
        if let Err(error) = self.jobs.send(job) {
            let _ = error
                .0
                .reply
                .send(Err("the JSSG transform pool stopped".to_string()));
        }
    }
}

/// The chunk loop, evaluated once in every worker context.
const DRIVER: &str = r#"
const describe = (error) => {
  if (!(error instanceof Error)) return String(error);
  const head = `${error.name}: ${error.message}`;
  const stack = error.stack?.trimEnd();
  if (stack === undefined || stack === "") return head;
  return stack.startsWith(head) ? stack : `${head}\n${stack}`;
};
const modules = new Map();
globalThis.__codemodRunChunk = async (worker, specifier, start, paths, contents) => {
  let transform;
  try {
    let loaded = modules.get(specifier);
    if (loaded === undefined) {
      loaded = import(specifier).then((namespace) => namespace.default);
      modules.set(specifier, loaded);
    }
    transform = await loaded;
    if (typeof transform !== "function") {
      throw new Error(`jssg codemod modules must export a function. ${typeof transform} found.`);
    }
  } catch (error) {
    return JSON.stringify({ fatal: `JSSG failed for '${paths[0]}': ${describe(error)}` });
  }
  const results = [];
  for (let offset = 0; offset < contents.length; offset++) {
    const index = start + offset;
    const path = paths[offset];
    let root;
    try {
      root = worker.parse(index, contents[offset]);
    } catch (error) {
      return JSON.stringify({ fatal: error instanceof Error ? error.message : String(error) });
    }
    if (root === null) {
      results.push({ outcome: { path, edits: [] } });
      continue;
    }
    let result;
    try {
      const options = worker.begin();
      try {
        result = await transform(root, options);
      } finally {
        worker.end(index);
      }
    } catch (error) {
      results.push({ error: `JSSG failed for '${path}': ${describe(error)}` });
      continue;
    }
    try {
      const outcome = worker.finish(index, root, result);
      results.push({ outcome: outcome === undefined ? { path, edits: [] } : JSON.parse(outcome) });
    } catch (error) {
      results.push({ error: error instanceof Error ? error.message : String(error) });
    }
  }
  return JSON.stringify({ results });
};
"#;

/// The batch a worker is running a chunk of, and what each call staged.
#[derive(Default)]
struct Current {
    plan: Option<Arc<BatchPlan>>,
    staged: HashMap<u32, Vec<FileChange>>,
}

fn work(target: PathBuf, queue: Arc<Mutex<Receiver<Job>>>) {
    let next = || queue.lock().ok().and_then(|queue| queue.recv().ok());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            while let Some(job) = next() {
                let _ = job
                    .reply
                    .send(Err(format!("JSSG transform worker: {error}")));
            }
            return;
        }
    };
    runtime.block_on(async move {
        let started: Result<(AsyncRuntime, AsyncContext, ModuleRegistry), String> = async {
            let runtime = AsyncRuntime::new().map_err(|error| format!("QuickJS runtime: {error}"))?;
            let registry = ModuleRegistry::default();
            let globals = configure_runtime(&runtime, registry.resolver(), registry.loader()).await;
            let context = AsyncContext::full(&runtime)
                .await
                .map_err(|error| format!("QuickJS context: {error}"))?;
            let installed: Result<(), String> = async_with!(context => |ctx| {
                install_context(&ctx, globals, &target).map_err(|error| error.to_string())?;
                ctx.eval::<(), _>(DRIVER).catch(&ctx).map_err(|error| error.to_string())
            })
            .await;
            installed.map(|()| (runtime, context, registry))
        }
        .await;
        let (_runtime, context, registry) = match started {
            Ok(started) => started,
            Err(error) => {
                while let Some(job) = next() {
                    let _ = job.reply.send(Err(format!("JSSG transform worker: {error}")));
                }
                return;
            }
        };
        let current = Rc::new(RefCell::new(Current::default()));
        while let Some(job) = next() {
            registry.register(job.specifier.clone(), job.source);
            current.borrow_mut().plan = Some(job.plan);
            let chunk_current = Rc::clone(&current);
            let (specifier, start, paths, contents) =
                (job.specifier, job.start, job.paths, job.contents);
            let reply: Result<String, String> = async_with!(context => |ctx| {
                let run = async {
                    let worker = worker_object(&ctx, chunk_current)?;
                    let chunk: Function = ctx.globals().get("__codemodRunChunk")?;
                    let pending: Promise = chunk.call((worker, specifier, start as u32, paths, contents))?;
                    pending.into_future::<String>().await
                };
                run.await.catch(&ctx).map_err(|error| error.to_string())
            })
            .await;
            {
                let mut current = current.borrow_mut();
                current.plan = None;
                current.staged.clear();
            }
            let reply = reply.and_then(|text| match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Object(reply)) if reply.contains_key("fatal") => Err(reply
                    .get("fatal")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("JSSG transform worker failed")
                    .to_string()),
                _ => Ok(text),
            });
            let _ = job.reply.send(reply);
        }
        drop(context);
    });
}

/// `{ parse, begin, end, finish }` over the worker's current batch.
fn worker_object<'js>(
    ctx: &Ctx<'js>,
    current: Rc<RefCell<Current>>,
) -> rquickjs::Result<Object<'js>> {
    let plan = |ctx: &Ctx<'js>, current: &RefCell<Current>| {
        current
            .borrow()
            .plan
            .clone()
            .ok_or_else(|| throw(ctx, None, "no JSSG batch on this worker"))
    };
    let worker = Object::new(ctx.clone())?;
    {
        let current = Rc::clone(&current);
        worker.set(
            "parse",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, index: u32, content: String| {
                    let plan = plan(&ctx, &current)?;
                    match plan
                        .parse(index as usize, &content)
                        .map_err(|error| throw(&ctx, None, &error))?
                    {
                        None => Ok(Value::new_null(ctx.clone())),
                        Some(root) => root_value(&ctx, root)
                            .map_err(|error| throw(&ctx, None, &error.to_string())),
                    }
                },
            )?,
        )?;
    }
    {
        let current = Rc::clone(&current);
        worker.set(
            "begin",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                let plan = plan(&ctx, &current)?;
                begin_call(&ctx, plan.language, plan.params.clone().unwrap_or_default())
                    .map_err(|error| throw(&ctx, None, &error.to_string()))
            })?,
        )?;
    }
    {
        let current = Rc::clone(&current);
        worker.set(
            "end",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, index: u32| {
                let staged = end_call(&ctx);
                current.borrow_mut().staged.insert(index, staged);
            })?,
        )?;
    }
    worker.set(
        "finish",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, index: u32, root: Value<'js>, result: Value<'js>| {
                let plan = plan(&ctx, &current)?;
                let staged = current
                    .borrow_mut()
                    .staged
                    .remove(&index)
                    .unwrap_or_default();
                finish_text(&ctx, &plan, index, &root, result, staged)
                    .map_err(|error| throw(&ctx, None, &error))
            },
        )?,
    )?;
    Ok(worker)
}
