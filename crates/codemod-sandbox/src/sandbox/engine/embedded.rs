//! JSSG transforms in a QuickJS context the embedder owns.
//!
//! [`execute_codemod_with_loader`](super::execute_codemod_with_loader) builds
//! a fresh runtime and context for every file. An embedder that keeps ONE
//! context for a whole run (the orchestration QuickJS host, where the
//! workflow's control plane and every transform share the context) uses this
//! module instead:
//!
//! 1. [`configure_runtime`] installs the same built-in modules a transform
//!    can import under the per-file engine (`codemod:ast-grep`,
//!    `codemod:workflow`, `codemod:metrics`, `codemod:runtime`, the curated
//!    `fs`, and LLRT's safe modules), chained before the embedder's own
//!    resolver and loader;
//! 2. [`install_context`] stores the per-run userdata those modules read
//!    (target directory, staged writes, no dry run, no test mode) and
//!    attaches the globals;
//! 3. [`root_value`] hands a parsed [`CoreRoot`] (the embedder parses, e.g.
//!    with `jssg_batch::BatchPlan::parse`) to JavaScript as an `SgRoot`;
//! 4. around every transform call, [`begin_call`] clears the staged-changes
//!    accumulator, sets the params `jssgTransform` forwards, and builds the
//!    transform `options` exactly as the per-file engine does;
//!    [`end_call`] takes what the call staged; [`transform_output`] turns
//!    the transform's return value into the same [`CodemodOutput`]
//!    (string/null/`{ content, output }` handling, `rename()`, staged
//!    `write()`/`jssgTransform` changes).
//!
//! Calls must be serialized: the staged-changes accumulator and the
//! `jssgTransform` params are per context, so one transform call is in
//! flight at a time (between [`begin_call`] and [`end_call`]). Semantic
//! analysis is not wired here; roots get no semantic provider.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use rquickjs::loader::{Loader, Resolver};
use rquickjs::{AsyncRuntime, Class, Ctx, IntoJs, Value};

use super::codemod_lang::CodemodLang;
use super::curated_fs::{CuratedFsConfig, CuratedFsModule, CuratedFsPromisesModule};
use super::execution_engine::{
    CodemodOutput, DryRunExecutionFlag, ExecutionModeFlag, FileChange, JssgExecutionContext,
    JssgFileChanges, StageFileWrites, TargetDirectory,
};
use super::transform_helpers::{
    build_transform_options, extract_transform_output, process_transform_result, ModificationCheck,
};
use crate::ast_grep::sg_core::CoreRoot;
use crate::ast_grep::sg_node::SgRootRjs;
use crate::ast_grep::AstGrepModule;
use crate::llm::LlmRuntimeContext;
use crate::metrics::MetricsModule;
use crate::sandbox::errors::{ExecutionError, RuntimeError};
use crate::sandbox::runtime_module::{RuntimeHooksContext, RuntimeModule};
use crate::workflow_global::{SharedStateContext, WorkflowGlobalModule};

type AttachGlobals = Box<dyn for<'js> FnOnce(&Ctx<'js>) -> rquickjs::Result<()>>;

/// Globals (LLRT's safe set: `AbortController`, timers, `Buffer`, `URL`,
/// `console`, ...) to attach once the context exists ([`install_context`]).
pub struct EmbeddedGlobals(AttachGlobals);

/// Chain the transform built-ins before `resolver`/`loader` (the embedder's
/// own modules: its bundle, transform artifacts) and set them on `runtime`.
pub async fn configure_runtime<R, L>(
    runtime: &AsyncRuntime,
    resolver: R,
    loader: L,
) -> EmbeddedGlobals
where
    R: Resolver + 'static,
    L: Loader + 'static,
{
    let (builtin_resolver, builtin_loader, globals) = LlrtModuleBuilder::build().builder.build();
    let builtin_resolver = builtin_resolver
        .add_name("codemod:ast-grep")
        .add_name("codemod:workflow")
        .add_name("codemod:metrics")
        .add_name("codemod:runtime")
        .add_name("fs")
        .add_name("fs/promises");
    let builtin_loader = builtin_loader
        .with_module("codemod:ast-grep", AstGrepModule)
        .with_module("codemod:workflow", WorkflowGlobalModule)
        .with_module("codemod:metrics", MetricsModule)
        .with_module("codemod:runtime", RuntimeModule)
        .with_module("fs", CuratedFsModule)
        .with_module("fs/promises", CuratedFsPromisesModule);
    runtime
        .set_loader((builtin_resolver, resolver), (builtin_loader, loader))
        .await;
    EmbeddedGlobals(Box::new(move |ctx: &Ctx<'_>| globals.attach(ctx)))
}

fn init_error(what: &str, error: impl std::fmt::Debug) -> ExecutionError {
    ExecutionError::Runtime {
        source: RuntimeError::InitializationFailed {
            message: format!("{what}: {error:?}"),
        },
    }
}

/// Per-run userdata and globals: the canonical target directory every
/// `rename()`/`jssgTransform()` path must stay inside, staged writes (the
/// embedder commits), no dry run, no test mode, no LLM, no metrics sink.
pub fn install_context(
    ctx: &Ctx<'_>,
    globals: EmbeddedGlobals,
    target_directory: &Path,
) -> Result<(), ExecutionError> {
    let target = target_directory
        .canonicalize()
        .unwrap_or_else(|_| target_directory.to_path_buf());
    ctx.store_userdata(ExecutionModeFlag { test_mode: false })
        .map_err(|e| init_error("ExecutionModeFlag", e))?;
    ctx.store_userdata(DryRunExecutionFlag(false))
        .map_err(|e| init_error("DryRunExecutionFlag", e))?;
    ctx.store_userdata(StageFileWrites(true))
        .map_err(|e| init_error("StageFileWrites", e))?;
    ctx.store_userdata(JssgFileChanges::default())
        .map_err(|e| init_error("JssgFileChanges", e))?;
    ctx.store_userdata(JssgExecutionContext {
        params: HashMap::new(),
        matrix_values: None,
    })
    .map_err(|e| init_error("JssgExecutionContext", e))?;
    ctx.store_userdata(TargetDirectory(target.clone()))
        .map_err(|e| init_error("TargetDirectory", e))?;
    ctx.store_userdata(LlmRuntimeContext::new(None))
        .map_err(|e| init_error("LlmRuntimeContext", e))?;
    ctx.store_userdata(SharedStateContext::default())
        .map_err(|e| init_error("SharedStateContext", e))?;
    ctx.store_userdata(RuntimeHooksContext::new(None, None))
        .map_err(|e| init_error("RuntimeHooksContext", e))?;
    let target_text = target.to_string_lossy().into_owned();
    let physical_root: vfs::VfsPath = vfs::PhysicalFS::new(PathBuf::from("/")).into();
    ctx.store_userdata(
        CuratedFsConfig::new(target_text.clone(), physical_root)
            .with_physical_target_dir(PathBuf::from(&target_text)),
    )
    .map_err(|e| init_error("CuratedFsConfig", e))?;
    (globals.0)(ctx).map_err(|e| init_error("global modules", e))?;
    Ok(())
}

/// `root` as the transform's `SgRoot` argument.
pub fn root_value<'js>(ctx: &Ctx<'js>, root: CoreRoot) -> Result<Value<'js>, ExecutionError> {
    SgRootRjs::from_core(root)
        .into_js(ctx)
        .map_err(|e| init_error("SgRoot", e))
}

/// The core root behind an `SgRoot` value (as made by [`root_value`]).
pub fn core_root(value: &Value<'_>) -> Option<CoreRoot> {
    Class::<SgRootRjs>::from_value(value)
        .ok()
        .map(|root| root.borrow().core.clone())
}

/// Start one transform call: clear the staged-changes accumulator, set the
/// params `jssgTransform` forwards, and build the transform `options` as the
/// per-file engine does: `{ params, language, matches: undefined,
/// matrixValues: undefined, dryRun: false, targetDir }`.
pub fn begin_call<'js>(
    ctx: &Ctx<'js>,
    language: CodemodLang,
    params: HashMap<String, serde_json::Value>,
) -> Result<Value<'js>, ExecutionError> {
    let target = ctx
        .userdata::<TargetDirectory>()
        .map(|target| target.0.clone())
        .ok_or_else(|| init_error("install_context was not called", ()))?;
    end_call(ctx);
    ctx.store_userdata(JssgExecutionContext {
        params: params.clone(),
        matrix_values: None,
    })
    .map_err(|e| init_error("JssgExecutionContext", e))?;
    build_transform_options(
        ctx,
        params,
        &language.to_string(),
        None,
        None,
        false,
        &target.to_string_lossy(),
    )
}

/// End the transform call: the `write()` / `jssgTransform()` changes it
/// staged, in order.
pub fn end_call(ctx: &Ctx<'_>) -> Vec<FileChange> {
    ctx.userdata::<JssgFileChanges>()
        .and_then(|changes| {
            changes
                .changes
                .lock()
                .map(|mut staged| std::mem::take(&mut *staged))
                .ok()
        })
        .unwrap_or_default()
}

/// The transform's (awaited) return value on `root`, with the changes the
/// call staged, as a [`CodemodOutput`], exactly as the per-file engine
/// converts it.
pub fn transform_output<'js>(
    ctx: &Ctx<'js>,
    root: &CoreRoot,
    result: Value<'js>,
    secondary: Vec<FileChange>,
) -> Result<CodemodOutput, ExecutionError> {
    let (content, output) = extract_transform_output(ctx, result)?;
    let original = root.source();
    let primary = process_transform_result(
        &content,
        root,
        ModificationCheck::StringEquality {
            original_content: &original,
        },
    )?;
    Ok(CodemodOutput {
        primary,
        secondary,
        output,
    })
}
