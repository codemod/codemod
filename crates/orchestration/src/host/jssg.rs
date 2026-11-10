//! In-context JSSG batches. The portable batch loop
//! (`packages/orchestration/src/execution/jssg-batch.ts`) drives one batch per `jssg` command through the bundle's
//! engine (`packages/orchestration/src/host/quickjs/dispatch.ts`), one
//! transform call at a time on the JS thread. Every batch rule is the
//! shared [`crate::jssg_batch::BatchPlan`]:
//!
//! - `jssgBegin(config, paths)`: [`BatchPlan`]: hash-verified artifact,
//!   canonical target root, compiled selector, params, every path resolved
//!   inside the root, and the batch's semantic provider (`semanticAnalysis`);
//! - `jssgIndex(batch, start, contents)`: [`BatchPlan::index`], the
//!   workspace index (workspace semantic analysis, before the first
//!   transform);
//! - `jssgParse(batch, start, contents)`: [`BatchPlan::parse`] for each
//!   file, an `SgRoot` or `null` where the static selector skips it;
//! - `jssgCall(batch)`: the transform `options`, staging cleared
//!   (`embedded::begin_call`); the loop then calls the transform (evaluated
//!   once per run) itself;
//! - `jssgCallEnd(batch, index)`: the changes the call staged, kept for
//!   `jssgFinish` (`embedded::end_call`);
//! - `jssgFinish(batch, index, root, result)`: the awaited result as the
//!   `FileOutcome` JSON of the shared rules (`undefined` when it
//!   has no edits and no output)
//!   (`embedded::transform_output` then [`BatchPlan::finish`]);
//! - `jssgEnd(batch)`;
//! - `jssgPoolSize()` / `jssgTransformRange(batch, specifier, source, start,
//!   paths, contents)`: whole chunks parsed, transformed, and converted on
//!   the transform pool ([`crate::host::pool`]), each worker with its own context,
//!   as JSON (`{ outcome } | { error }` per file).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::jssg_batch::{BatchConfig, BatchPlan, Containment};
use codemod_sandbox::sandbox::engine::embedded::{
    begin_call, core_root, end_call, root_value, transform_output,
};
use codemod_sandbox::sandbox::engine::FileChange;
use rquickjs::{Ctx, Function, Object, Promise, Value};

use crate::host::native::{error_value, throw, HostState};
use crate::host::pool::{self, Job};

pub struct OpenBatch {
    plan: Arc<BatchPlan>,
    /// What each finished transform call staged, until `jssgFinish`.
    staged: HashMap<u32, Vec<FileChange>>,
}

fn with_batch<'js, T>(
    ctx: &Ctx<'js>,
    state: &HostState,
    id: u32,
    f: impl FnOnce(&mut OpenBatch) -> rquickjs::Result<T>,
) -> rquickjs::Result<T> {
    let mut batches = state.batches.borrow_mut();
    let batch = batches
        .get_mut(&id)
        .ok_or_else(|| throw(ctx, None, &format!("unknown JSSG batch {id}")))?;
    f(batch)
}

pub fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    state: Rc<HostState>,
) -> rquickjs::Result<()> {
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgBegin",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, config: String, paths: Vec<String>| {
                    let config: BatchConfig = serde_json::from_str(&config).map_err(|error| {
                        throw(&ctx, None, &format!("invalid JSSG batch: {error}"))
                    })?;
                    let plan = BatchPlan::for_paths(
                        &config.batch(),
                        paths.iter().map(String::as_str),
                        Containment::PerFile,
                    )
                    .map_err(|error| throw(&ctx, None, &error))?;
                    let id = state.next_id();
                    let module = plan.module.clone();
                    let indexes_workspace = plan.indexes_workspace();
                    state.batches.borrow_mut().insert(
                        id,
                        OpenBatch {
                            plan: Arc::new(plan),
                            staged: HashMap::new(),
                        },
                    );
                    let handle = Object::new(ctx.clone())?;
                    handle.set("id", id)?;
                    handle.set("module", module)?;
                    handle.set("indexesWorkspace", indexes_workspace)?;
                    Ok::<_, rquickjs::Error>(handle)
                },
            )?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgIndex",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, id: u32, start: u32, contents: Vec<String>| {
                    // On this thread: the index is the batch's, like every root.
                    with_batch(&ctx, &state, id, |batch| {
                        for (offset, content) in contents.iter().enumerate() {
                            batch
                                .plan
                                .index(start as usize + offset, content)
                                .map_err(|error| throw(&ctx, None, &error))?;
                        }
                        Ok(())
                    })
                },
            )?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgParse",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      id: u32,
                      start: u32,
                      contents: Vec<String>|
                      -> rquickjs::Result<Vec<Value<'js>>> {
                    let roots = with_batch(&ctx, &state, id, |batch| {
                        let mut roots = Vec::with_capacity(contents.len());
                        for (offset, content) in contents.iter().enumerate() {
                            let root = batch
                                .plan
                                .parse(start as usize + offset, content)
                                .map_err(|error| throw(&ctx, None, &error))?;
                            roots.push(match root {
                                None => Value::new_null(ctx.clone()),
                                Some(root) => root_value(&ctx, root)
                                    .map_err(|error| throw(&ctx, None, &error.to_string()))?,
                            });
                        }
                        Ok(roots)
                    })?;
                    Ok(roots)
                },
            )?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgCall",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: u32| {
                let (language, params) = with_batch(&ctx, &state, id, |batch| {
                    Ok((
                        batch.plan.language,
                        batch.plan.params.clone().unwrap_or_default(),
                    ))
                })?;
                begin_call(&ctx, language, params)
                    .map_err(|error| throw(&ctx, None, &error.to_string()))
            })?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgCallEnd",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: u32, index: u32| {
                let staged = end_call(&ctx);
                with_batch(&ctx, &state, id, |batch| {
                    batch.staged.insert(index, staged);
                    Ok(())
                })
            })?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgFinish",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, id: u32, index: u32, root: Value<'js>, result: Value<'js>| {
                    let text = with_batch(&ctx, &state, id, |batch| {
                        let staged = batch.staged.remove(&index).unwrap_or_default();
                        finish_text(&ctx, &batch.plan, index, &root, result, staged)
                            .map_err(|error| throw(&ctx, None, &error))
                    })?;
                    Ok::<_, rquickjs::Error>(text)
                },
            )?,
        )?;
    }
    native.set("jssgPoolSize", Function::new(ctx.clone(), pool::size)?)?;
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgTransformRange",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      id: u32,
                      specifier: String,
                      source: String,
                      start: u32,
                      paths: Vec<String>,
                      contents: Vec<String>|
                      -> rquickjs::Result<Promise<'js>> {
                    let plan = with_batch(&ctx, &state, id, |batch| Ok(Arc::clone(&batch.plan)))?;
                    let (reply, replied) = tokio::sync::oneshot::channel();
                    state.pool().run(Job {
                        plan,
                        specifier,
                        source,
                        start: start as usize,
                        paths,
                        contents,
                        reply,
                    });
                    let (promise, resolve, reject) = Promise::new(&ctx)?;
                    let ctx2 = ctx.clone();
                    ctx.spawn(async move {
                        let settled = match replied.await {
                            Ok(Ok(text)) => resolve.call::<_, ()>((text,)),
                            Ok(Err(message)) => error_value(&ctx2, None, &message)
                                .and_then(|error| reject.call::<_, ()>((error,))),
                            Err(_) => error_value(&ctx2, None, "a JSSG transform worker stopped")
                                .and_then(|error| reject.call::<_, ()>((error,))),
                        };
                        if settled.is_err() {
                            let _ = ctx2.catch();
                        }
                    });
                    Ok(promise)
                },
            )?,
        )?;
    }
    native.set(
        "jssgEnd",
        Function::new(ctx.clone(), move |id: u32| {
            state.batches.borrow_mut().remove(&id);
        })?,
    )?;
    Ok(())
}

/// The awaited `result` of file `index`'s transform on `root`, with what the
/// call staged, as `FileOutcome` JSON (`None`: no edits and no output; the
/// caller builds `{ path, edits: [] }`); the error names the file.
pub(crate) fn finish_text<'js>(
    ctx: &Ctx<'js>,
    plan: &BatchPlan,
    index: u32,
    root: &Value<'js>,
    result: Value<'js>,
    staged: Vec<FileChange>,
) -> Result<Option<String>, String> {
    let core = core_root(root).ok_or_else(|| format!("file {index} has no SgRoot"))?;
    let output = transform_output(ctx, &core, result, staged).map_err(|error| error.to_string());
    let outcome = plan.finish(index as usize, output)?;
    if outcome.edits.is_empty() && outcome.output.is_none() {
        return Ok(None);
    }
    serde_json::to_string(&outcome)
        .map(Some)
        .map_err(|error| error.to_string())
}
