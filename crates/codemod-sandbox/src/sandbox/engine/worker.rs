//! A long-lived JSSG runtime for hosts that call into JavaScript many times,
//! such as a workflow's `forEachFile` step running its callback per file.
//!
//! [`execute_codemod_with_quickjs`](super::execute_codemod_with_quickjs)
//! builds a fresh runtime for every file. A [`Worker`] builds one async
//! QuickJS context with the same modules (`codemod:ast-grep`,
//! `codemod:workflow`, `codemod:metrics`, `codemod:runtime`, `codemod:llm`
//! when fetch is granted, and the LLRT modules the capabilities allow), plus
//! in-memory modules and relative or package imports, and keeps it for as
//! many calls as the host makes.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{async_with, AsyncContext, AsyncRuntime};

use super::execution_engine::{
    install_console_bridge, DryRunExecutionFlag, ExecutionModeFlag, FileChange,
    JssgExecutionContext, JssgFileChanges, TargetDirectory,
};
use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::{AstGrepModule, DefaultParseLanguage};
use crate::llm::{LlmModule, LlmRequestHandler, LlmRuntimeContext};
use crate::metrics::{MetricsContext, MetricsModule};
use crate::sandbox::errors::{ExecutionError, RuntimeError};
use crate::sandbox::resolvers::ModuleResolver;
use crate::sandbox::runtime_module::{
    RuntimeEventCallback, RuntimeFailure, RuntimeHooksContext, RuntimeModule,
};
use crate::workflow_global::{SharedStateContext, WorkflowGlobalModule};

/// How to build a [`Worker`].
pub struct WorkerOptions<R> {
    /// `fs`, `fetch` and `child_process` are only available when granted.
    /// `fs` is not installed in a dry run.
    pub capabilities: HashSet<LlrtSupportedModules>,
    /// Resolves relative and package imports. Imports from in-memory modules
    /// resolve against the resolver's base directory.
    pub resolver: Arc<R>,
    /// In-memory modules, by name, as JavaScript source.
    pub modules: Vec<(String, String)>,
    pub metrics: MetricsContext,
    pub shared_state: SharedStateContext,
    /// Handles `codemod:llm` requests; the module exists only when fetch is granted.
    pub llm_request_handler: Option<LlmRequestHandler>,
    /// Receives `codemod:runtime` events and `console` output.
    pub runtime_event_callback: Option<RuntimeEventCallback>,
    /// Interrupts running JavaScript when set.
    pub cancel: Arc<AtomicBool>,
    pub dry_run: bool,
    pub target_directory: PathBuf,
    /// The language `parse(text)` uses when the call names none.
    pub default_language: Option<String>,
}

/// A JSSG runtime kept alive across calls. Run code in it with
/// `async_with!(worker.context => |ctx| { ... })`.
pub struct Worker {
    /// Keeps the runtime alive for as long as the context is used.
    pub runtime: AsyncRuntime,
    pub context: AsyncContext,
    hooks: RuntimeHooksContext,
    file_changes: JssgFileChanges,
}

fn init_error(message: String) -> ExecutionError {
    ExecutionError::Runtime {
        source: RuntimeError::InitializationFailed { message },
    }
}

impl Worker {
    pub async fn new<R: ModuleResolver + 'static>(
        options: WorkerOptions<R>,
    ) -> Result<Self, ExecutionError> {
        let runtime = AsyncRuntime::new()
            .map_err(|e| init_error(format!("Failed to create AsyncRuntime: {e}")))?;
        let interrupt = Arc::clone(&options.cancel);
        runtime
            .set_interrupt_handler(Some(Box::new(move || interrupt.load(Ordering::SeqCst))))
            .await;

        let fetch = options.capabilities.contains(&LlrtSupportedModules::Fetch);
        let mut module_builder = LlrtModuleBuilder::build();
        for capability in &options.capabilities {
            match capability {
                LlrtSupportedModules::Fetch => {
                    module_builder.enable_fetch();
                }
                LlrtSupportedModules::Fs if !options.dry_run => {
                    module_builder.enable_fs();
                }
                LlrtSupportedModules::ChildProcess => {
                    module_builder.enable_child_process();
                }
                _ => {}
            }
        }

        let (mut resolver, mut loader, globals) = module_builder.builder.build();
        resolver = resolver
            .add_name("codemod:ast-grep")
            .add_name("codemod:workflow")
            .add_name("codemod:metrics")
            .add_name("codemod:runtime");
        loader = loader
            .with_module("codemod:ast-grep", AstGrepModule)
            .with_module("codemod:workflow", WorkflowGlobalModule)
            .with_module("codemod:metrics", MetricsModule)
            .with_module("codemod:runtime", RuntimeModule);
        if fetch {
            resolver = resolver.add_name("codemod:llm");
            loader = loader.with_module("codemod:llm", LlmModule);
        }

        let mut memory_resolver = BuiltinResolver::default();
        let mut memory_loader = BuiltinLoader::default();
        for (name, source) in &options.modules {
            memory_resolver = memory_resolver.with_module(name.as_str());
            memory_loader = memory_loader.with_module(name.as_str(), source.as_str());
        }

        runtime
            .set_loader(
                (
                    resolver,
                    memory_resolver,
                    QuickJSResolver::new(options.resolver),
                ),
                (loader, memory_loader, QuickJSLoader),
            )
            .await;

        let context = AsyncContext::full(&runtime)
            .await
            .map_err(|e| init_error(format!("Failed to create AsyncContext: {e}")))?;

        let hooks = RuntimeHooksContext::new(
            options.runtime_event_callback.clone(),
            Some(Arc::clone(&options.cancel)),
        );
        let file_changes = JssgFileChanges::default();
        let target = options
            .target_directory
            .canonicalize()
            .unwrap_or_else(|_| options.target_directory.clone());
        let llm = LlmRuntimeContext::new(fetch.then_some(options.llm_request_handler).flatten());

        let hooks_for_ctx = hooks.clone();
        let file_changes_for_ctx = file_changes.clone();
        let installed: Result<(), String> = async_with!(context => |ctx| {
            let store = || -> rquickjs::Result<()> {
                let fail = |e: String| rquickjs::Error::new_from_js_message("userdata", "worker", e);
                ctx.store_userdata(ExecutionModeFlag { test_mode: false })
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(DryRunExecutionFlag(options.dry_run))
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(file_changes_for_ctx.clone())
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(JssgExecutionContext {
                    params: Default::default(),
                    matrix_values: None,
                })
                .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(TargetDirectory(target.clone()))
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(options.metrics.clone())
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(llm.clone())
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(options.shared_state.clone())
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(hooks_for_ctx.clone())
                    .map_err(|e| fail(format!("{e:?}")))?;
                if let Some(language) = &options.default_language {
                    ctx.store_userdata(DefaultParseLanguage(language.clone()))
                        .map_err(|e| fail(format!("{e:?}")))?;
                }
                globals.attach(&ctx)?;
                install_console_bridge(&ctx)
            };
            store().map_err(|e| e.to_string())
        })
        .await;
        installed.map_err(init_error)?;

        Ok(Self {
            runtime,
            context,
            hooks,
            file_changes,
        })
    }

    /// A failure reported with `codemod:runtime`'s `failFile` or `failStep`
    /// since the last call.
    pub fn take_pending_failure(&self) -> Option<RuntimeFailure> {
        self.hooks.take_pending_failure()
    }

    /// Files changed with `jssgTransform` since the last call.
    pub fn take_secondary_changes(&self) -> Vec<FileChange> {
        std::mem::take(&mut *self.file_changes.changes.lock().unwrap())
    }
}
