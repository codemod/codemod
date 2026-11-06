//! In-context JSSG batches. The bundle's dispatcher
//! (`packages/orchestration/src/host/quickjs/dispatch.ts`) drives one batch
//! per `jssg` command, one file at a time on the JS thread:
//!
//! - `jssgBegin(config, paths)`: the execution bridge's own
//!   [`BatchPlan`]: hash-verified artifact, canonical target root, compiled
//!   selector, params, every path resolved inside the root; semantic
//!   analysis is refused (`unsupported in runtime 'quickjs'`);
//! - `jssgPrepare(batch, index, content)`: `null` when the static selector
//!   skips the file, else `[root, options]` for the transform
//!   (`embedded::begin_file`);
//! - the dispatcher calls the transform (evaluated once per run) itself;
//! - `jssgFinish(batch, index, result)`: the awaited result as the
//!   `FileOutcome` JSON the bridge would have returned (`embedded::finish_file`
//!   then [`BatchPlan::convert`]);
//! - `jssgEnd(batch)`.

use std::rc::Rc;
use std::time::{Duration, Instant};

use butterflow_execution_bridge::jssg::{Batch, BatchPlan, SemanticSupport};
use butterflow_execution_bridge::{ArtifactRef, Selector, SemanticAnalysis};
use codemod_sandbox::sandbox::engine::embedded::{begin_file, finish_file, EmbeddedFile};
use rquickjs::{Array, Ctx, Function, Object, Value};
use serde::Deserialize;

use crate::native::{throw, HostState};

pub struct OpenBatch {
    plan: BatchPlan,
    paths: Vec<String>,
    current: Option<(u32, EmbeddedFile)>,
}

/// Time spent in Rust around transforms, for the timer breakdown.
#[derive(Default)]
pub struct Stats {
    /// `jssgPrepare`: selector check, parse, options.
    pub prepare: Duration,
    /// `jssgFinish`: result conversion and path validation.
    pub finish: Duration,
    pub files: u64,
    pub skipped: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchConfig {
    transform: ArtifactRef,
    source: Option<String>,
    language: String,
    #[serde(default)]
    selector: Option<Selector>,
    #[serde(default)]
    semantic_analysis: Option<SemanticAnalysis>,
    #[serde(default)]
    input: Option<serde_json::Value>,
    target_root: Option<String>,
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
                    let batch = Batch {
                        artifact: &config.transform,
                        source: config.source.as_deref(),
                        language: &config.language,
                        selector: config.selector.as_ref(),
                        target_root: config.target_root.as_deref(),
                        semantic_analysis: config.semantic_analysis.as_ref(),
                        input: config.input.as_ref(),
                        files: &[],
                    };
                    let plan = BatchPlan::for_paths(
                        &batch,
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
                            paths,
                            current: None,
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
            "jssgPrepare",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      id: u32,
                      index: u32,
                      content: String|
                      -> rquickjs::Result<Value<'js>> {
                    let started = Instant::now();
                    let mut batches = state.batches.borrow_mut();
                    let batch = batches
                        .get_mut(&id)
                        .ok_or_else(|| throw(&ctx, None, &format!("unknown JSSG batch {id}")))?;
                    let source = batch
                        .plan
                        .sources
                        .get(index as usize)
                        .ok_or_else(|| {
                            throw(&ctx, None, &format!("no file {index} in batch {id}"))
                        })?
                        .clone();
                    let mut stats = state.stats.borrow_mut();
                    if batch.plan.skips(&content) {
                        stats.skipped += 1;
                        stats.prepare += started.elapsed();
                        return Ok(Value::new_null(ctx.clone()));
                    }
                    let (file, root, options) = begin_file(
                        &ctx,
                        batch.plan.language,
                        &source,
                        &content,
                        batch.plan.params.clone().unwrap_or_default(),
                    )
                    .map_err(|error| {
                        throw(
                            &ctx,
                            None,
                            &format!("JSSG failed for '{}': {error}", batch.paths[index as usize]),
                        )
                    })?;
                    batch.current = Some((index, file));
                    stats.files += 1;
                    stats.prepare += started.elapsed();
                    let pair = Array::new(ctx.clone())?;
                    pair.set(0, root)?;
                    pair.set(1, options)?;
                    Ok(pair.into_value())
                },
            )?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "jssgFinish",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, id: u32, index: u32, result: Value<'js>| {
                    let started = Instant::now();
                    let mut batches = state.batches.borrow_mut();
                    let batch = batches
                        .get_mut(&id)
                        .ok_or_else(|| throw(&ctx, None, &format!("unknown JSSG batch {id}")))?;
                    let path = batch.paths.get(index as usize).cloned().ok_or_else(|| {
                        throw(&ctx, None, &format!("no file {index} in batch {id}"))
                    })?;
                    let file = match batch.current.take() {
                        Some((current, file)) if current == index => file,
                        _ => {
                            return Err(throw(
                                &ctx,
                                None,
                                &format!("file {index} of batch {id} was not prepared"),
                            ))
                        }
                    };
                    let outcome = finish_file(&ctx, file, result)
                        .map_err(|error| error.to_string())
                        .and_then(|output| batch.plan.convert(&path, output))
                        .map_err(|error| {
                            throw(&ctx, None, &format!("JSSG failed for '{path}': {error}"))
                        })?;
                    let text = serde_json::to_string(&outcome)
                        .map_err(|error| throw(&ctx, None, &error.to_string()))?;
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
