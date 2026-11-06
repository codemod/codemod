//! In-context JSSG batches. The portable batch loop
//! (`packages/orchestration/src/execution/jssg-batch.ts`, shared with the
//! `node` runtime) drives one batch per `jssg` command through the bundle's
//! engine (`packages/orchestration/src/host/quickjs/dispatch.ts`), one
//! transform call at a time on the JS thread. Every batch rule is the
//! shared [`jssg_batch::BatchPlan`]:
//!
//! - `jssgBegin(config, paths)`: [`BatchPlan`]: hash-verified artifact,
//!   canonical target root, compiled selector, params, every path resolved
//!   inside the root; semantic analysis is refused (`unsupported in runtime
//!   'quickjs'`);
//! - `jssgParse(batch, start, contents)`: [`BatchPlan::parse`] for each
//!   file, an `SgRoot` or `null` where the static selector skips it;
//! - `jssgCall(batch)`: the transform `options`, staging cleared
//!   (`embedded::begin_call`); the loop then calls the transform (evaluated
//!   once per run) itself;
//! - `jssgCallEnd(batch, index)`: the changes the call staged, kept for
//!   `jssgFinish` (`embedded::end_call`);
//! - `jssgFinish(batch, index, root, result)`: the awaited result as the
//!   `FileOutcome` JSON the bridge would have returned
//!   (`embedded::transform_output` then [`BatchPlan::finish`]);
//! - `jssgEnd(batch)`.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use codemod_sandbox::sandbox::engine::embedded::{
    begin_call, core_root, end_call, root_value, transform_output,
};
use codemod_sandbox::sandbox::engine::FileChange;
use jssg_batch::{BatchConfig, BatchPlan, SemanticSupport};
use rquickjs::{Ctx, Function, Object, Value};

use crate::native::{throw, HostState};

pub struct OpenBatch {
    plan: BatchPlan,
    /// What each finished transform call staged, until `jssgFinish`.
    staged: HashMap<u32, Vec<FileChange>>,
}

/// Time spent in Rust around transforms, for the timer breakdown.
#[derive(Default)]
pub struct Stats {
    /// `jssgParse`: selector check and parse.
    pub prepare: Duration,
    /// `jssgFinish`: result conversion and path validation.
    pub finish: Duration,
    pub files: u64,
    pub skipped: u64,
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
                        SemanticSupport::Unsupported("quickjs"),
                    )
                    .map_err(|error| throw(&ctx, None, &error))?;
                    let id = state.next_id();
                    let module = plan.module.clone();
                    state.batches.borrow_mut().insert(
                        id,
                        OpenBatch {
                            plan,
                            staged: HashMap::new(),
                        },
                    );
                    let handle = Object::new(ctx.clone())?;
                    handle.set("id", id)?;
                    handle.set("module", module)?;
                    Ok::<_, rquickjs::Error>(handle)
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
                    let started = Instant::now();
                    let roots = with_batch(&ctx, &state, id, |batch| {
                        let mut stats = state.stats.borrow_mut();
                        let mut roots = Vec::with_capacity(contents.len());
                        for (offset, content) in contents.iter().enumerate() {
                            let root = batch
                                .plan
                                .parse(start as usize + offset, content)
                                .map_err(|error| throw(&ctx, None, &error))?;
                            roots.push(match root {
                                None => {
                                    stats.skipped += 1;
                                    Value::new_null(ctx.clone())
                                }
                                Some(root) => {
                                    stats.files += 1;
                                    root_value(&ctx, root)
                                        .map_err(|error| throw(&ctx, None, &error.to_string()))?
                                }
                            });
                        }
                        Ok(roots)
                    })?;
                    state.stats.borrow_mut().prepare += started.elapsed();
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
                    let started = Instant::now();
                    let text = with_batch(&ctx, &state, id, |batch| {
                        let core = core_root(&root).ok_or_else(|| {
                            throw(&ctx, None, &format!("file {index} has no SgRoot"))
                        })?;
                        let staged = batch.staged.remove(&index).unwrap_or_default();
                        let output = transform_output(&ctx, &core, result, staged)
                            .map_err(|error| error.to_string());
                        let outcome = batch
                            .plan
                            .finish(index as usize, output)
                            .map_err(|error| throw(&ctx, None, &error))?;
                        serde_json::to_string(&outcome)
                            .map_err(|error| throw(&ctx, None, &error.to_string()))
                    })?;
                    state.stats.borrow_mut().finish += started.elapsed();
                    Ok::<_, rquickjs::Error>(text)
                },
            )?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgEnd",
            Function::new(ctx.clone(), move |id: u32| {
                state.batches.borrow_mut().remove(&id);
            })?,
        )?;
    }
    native.set(
        "jssgStats",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            let stats = state.stats.borrow();
            let object = Object::new(ctx.clone())?;
            object.set("prepareMs", stats.prepare.as_secs_f64() * 1000.0)?;
            object.set("finishMs", stats.finish.as_secs_f64() * 1000.0)?;
            object.set("files", stats.files as f64)?;
            object.set("skipped", stats.skipped as f64)?;
            Ok::<_, rquickjs::Error>(object)
        })?,
    )?;
    Ok(())
}
