//! A long-lived JSSG runtime for hosts that call into JavaScript many times,
//! such as a workflow's `forEachFile` step running its callback per file.
//!
//! [`execute_codemod_with_quickjs`](super::execute_codemod_with_quickjs)
//! builds a fresh runtime for every file. A [`Worker`] builds one async
//! QuickJS context with the same modules (`codemod:ast-grep`,
//! `codemod:workflow`, `codemod:runtime`, `codemod:llm` when fetch is
//! granted, and the LLRT modules the capabilities allow), plus
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
    install_console_bridge, CallbackIsolate, DryRunExecutionFlag, ExecutionModeFlag, FileChange,
    JssgExecutionContext, JssgFileChanges, TargetDirectory,
};
use super::pending_files::PendingFiles;
use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::{DefaultParseLanguage, ParseSemanticProvider};
use crate::llm::{LlmRequestHandler, LlmRuntimeContext};
use crate::sandbox::errors::{ExecutionError, RuntimeError};
use crate::sandbox::resolvers::ModuleResolver;
use crate::sandbox::runtime_module::{RuntimeEventCallback, RuntimeFailure, RuntimeHooksContext};

/// Keeps the ids of the timers the code has running, so
/// [`Worker::shutdown`] can clear them: llrt holds their callbacks in a
/// table outside the runtime, which freeing the runtime does not empty.
const TRACK_TIMERS: &str = r#"(() => {
  const g = globalThis;
  if (typeof g.setTimeout !== "function") return;
  const live = new Set();
  const { setTimeout, setInterval, setImmediate, clearTimeout, clearInterval } = g;
  const once = (start) => function (callback, ...rest) {
    if (typeof callback !== "function") return start(callback, ...rest);
    let id;
    id = start(function (...args) {
      live.delete(id);
      return callback.apply(this, args);
    }, ...rest);
    live.add(id);
    return id;
  };
  g.setTimeout = once(setTimeout);
  g.setImmediate = once(setImmediate);
  g.setInterval = function (callback, ...rest) {
    const id = setInterval(callback, ...rest);
    live.add(id);
    return id;
  };
  g.clearTimeout = (id) => { live.delete(id); return clearTimeout(id); };
  g.clearInterval = (id) => { live.delete(id); return clearInterval(id); };
  Object.defineProperty(g, Symbol.for("codemod.clearTimers"), {
    value: () => {
      for (const id of live) clearTimeout(id);
      live.clear();
    },
  });
})();"#;

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
    /// The in-memory modules codemod modules resolve to, by codemod module
    /// name (`workflow` for `codemod/workflow`); importing a codemod module
    /// that is not here, and not offered to every transform, is an error.
    pub codemod_modules: Vec<(&'static str, String)>,
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
    /// Semantic analysis for roots `parse(file)` makes, so `definition()`
    /// and `references()` find something; none without it.
    pub semantic_provider: Option<Arc<dyn language_core::SemanticProvider>>,
    /// The step's unwritten files, when several workers share them;
    /// `jssgTransform` reads and changes files through it. Each worker has
    /// its own without it.
    pub pending_files: Option<PendingFiles>,
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

/// See [`Worker::take_side_effects`].
pub struct SideEffects {
    /// A failure reported with `codemod/runtime`'s `failFile` or `failStep`.
    pub failure: Option<RuntimeFailure>,
    /// Files changed with `jssgTransform`.
    pub other_files: Vec<FileChange>,
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
                // A dry run changes nothing: no file system, no commands.
                LlrtSupportedModules::Fs if !options.dry_run => {
                    module_builder.enable_fs();
                }
                LlrtSupportedModules::ChildProcess if !options.dry_run => {
                    module_builder.enable_child_process();
                }
                _ => {}
            }
        }

        // A dry run with `fs` granted: a curated fs that reads the disk and
        // keeps writes in memory, so code that writes reports still runs.
        let dry_fs = options.dry_run && options.capabilities.contains(&LlrtSupportedModules::Fs);
        let (mut resolver, loader, globals) = module_builder.builder.build();
        let (codemod_resolver, mut loader) = crate::with_codemod_modules!(loader, llm: fetch);
        if dry_fs {
            resolver = resolver.add_name("fs").add_name("fs/promises");
            loader = loader
                .with_module("fs", super::curated_fs::CuratedFsModule)
                .with_module("fs/promises", super::curated_fs::CuratedFsPromisesModule);
        }
        let codemod_resolver = codemod_resolver.providing(options.codemod_modules.clone());

        let mut memory_resolver = BuiltinResolver::default();
        let mut memory_loader = BuiltinLoader::default();
        for (name, source) in &options.modules {
            memory_resolver = memory_resolver.with_module(name.as_str());
            memory_loader = memory_loader.with_module(name.as_str(), source.as_str());
        }

        runtime
            .set_loader(
                (
                    codemod_resolver,
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
        // `jssgTransform` calls in one step build on each other's results.
        let file_changes = JssgFileChanges {
            pending: Some(options.pending_files.clone().unwrap_or_default()),
            ..JssgFileChanges::default()
        };
        // Not `\\?\C:\...` on Windows: code builds paths from this one by
        // appending `/` and relative paths, which such paths do not take.
        let target = dunce::canonicalize(&options.target_directory)
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
                ctx.store_userdata(CallbackIsolate)
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
                ctx.store_userdata(llm.clone())
                    .map_err(|e| fail(format!("{e:?}")))?;
                ctx.store_userdata(hooks_for_ctx.clone())
                    .map_err(|e| fail(format!("{e:?}")))?;
                if dry_fs {
                    let memory_root: vfs::VfsPath = vfs::MemoryFS::new().into();
                    let cfg = super::curated_fs::CuratedFsConfig::new(
                        target.to_string_lossy().into_owned(),
                        memory_root,
                    )
                    .with_fetcher(Arc::new(super::execution_engine::DryRunDiskFetcher {
                        target_directory: target.clone(),
                    }));
                    ctx.store_userdata(cfg).map_err(|e| fail(format!("{e:?}")))?;
                }
                if let Some(language) = &options.default_language {
                    ctx.store_userdata(DefaultParseLanguage(language.clone()))
                        .map_err(|e| fail(format!("{e:?}")))?;
                }
                if let Some(provider) = &options.semantic_provider {
                    ctx.store_userdata(ParseSemanticProvider(Arc::clone(provider)))
                        .map_err(|e| fail(format!("{e:?}")))?;
                }
                globals.attach(&ctx)?;
                ctx.eval::<(), _>(TRACK_TIMERS)?;
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

    /// Free the worker. Timers the code left (a pending `setTimeout`, an
    /// interval) are cleared: llrt keeps their callbacks outside the
    /// runtime, and QuickJS aborts the process when a runtime is freed
    /// while something outside it still holds its values. Other leftover
    /// work (an unawaited promise) gets up to `grace` to finish; whatever
    /// is still pending then is freed with the runtime, which drops pending
    /// jobs and futures before it frees itself.
    pub async fn shutdown(self, grace: std::time::Duration) {
        // Leftover work may start timers while it finishes: clear them as
        // it goes.
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            self.clear_timers().await;
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let slice = left.min(std::time::Duration::from_millis(50));
            if tokio::time::timeout(slice, self.runtime.idle())
                .await
                .is_ok()
                || left <= slice
            {
                break;
            }
        }
        self.clear_timers().await;
    }

    async fn clear_timers(&self) {
        async_with!(self.context => |ctx| {
            let clear: rquickjs::Result<()> = (|| {
                let symbol: rquickjs::Symbol = ctx.eval(r#"Symbol.for("codemod.clearTimers")"#)?;
                match ctx.globals().get::<_, Option<rquickjs::Function>>(symbol)? {
                    Some(clear) => clear.call(()),
                    None => Ok(()),
                }
            })();
            if clear.is_err() {
                let _ = ctx.catch();
            }
        })
        .await;
    }

    /// What the code did since the last call besides returning a value.
    /// Callers take both together after each call into the worker, so
    /// neither is left to be blamed on the next call.
    pub fn take_side_effects(&self) -> SideEffects {
        SideEffects {
            failure: self.hooks.take_pending_failure(),
            other_files: std::mem::take(&mut *self.file_changes.changes.lock().unwrap()),
        }
    }

    /// The files `jssgTransform` reads and changes through.
    pub fn pending_files(&self) -> Option<&PendingFiles> {
        self.file_changes.pending.as_ref()
    }

    /// What `jssgTransform` calls have made of the files they changed, so
    /// far: `(path, at the start of the step, now)`.
    pub fn changed_files(&self) -> Vec<(PathBuf, String, String)> {
        self.file_changes
            .pending
            .as_ref()
            .map(PendingFiles::changed)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::sandbox::resolvers::InMemoryResolver;

    async fn worker() -> Worker {
        Worker::new(WorkerOptions {
            capabilities: HashSet::new(),
            resolver: Arc::new(InMemoryResolver::new()),
            modules: Vec::new(),
            codemod_modules: Vec::new(),
            llm_request_handler: None,
            runtime_event_callback: None,
            cancel: Arc::new(AtomicBool::new(false)),
            dry_run: true,
            target_directory: std::env::temp_dir(),
            default_language: None,
            semantic_provider: None,
            pending_files: None,
        })
        .await
        .unwrap()
    }

    async fn eval<T>(worker: &Worker, code: &'static str) -> T
    where
        T: for<'js> rquickjs::FromJs<'js> + Send + 'static,
    {
        async_with!(worker.context => |ctx| {
            ctx.eval::<T, _>(code).map_err(|e| format!("{e}: {:?}", ctx.catch()))
        })
        .await
        .unwrap()
    }

    /// Timers left running (llrt keeps their callbacks outside the
    /// runtime) are cleared, so the runtime is freed without waiting out
    /// the grace period.
    #[tokio::test]
    async fn a_worker_with_timers_left_running_is_freed() {
        let worker = worker().await;
        eval::<()>(
            &worker,
            "setInterval(() => {}, 5); setTimeout(() => {}, 1e7); setImmediate(() => {});\
             new Promise((resolve) => setTimeout(resolve, 1e7)).then(() => setInterval(() => {}, 5));",
        )
        .await;
        let runtime = worker.runtime.weak();
        let started = Instant::now();
        worker.shutdown(Duration::from_secs(10)).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(runtime.try_ref().is_none(), "the runtime was kept");
    }

    /// Timers started while leftover work finishes are cleared too.
    #[tokio::test]
    async fn timers_started_by_leftover_work_are_cleared() {
        let worker = worker().await;
        eval::<()>(
            &worker,
            "Promise.resolve().then(() => setInterval(() => {}, 5)); setTimeout(() => setInterval(() => {}, 5), 1);",
        )
        .await;
        let runtime = worker.runtime.weak();
        let started = Instant::now();
        worker.shutdown(Duration::from_secs(10)).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(runtime.try_ref().is_none(), "the runtime was kept");
    }

    /// The tracking keeps timers working as before.
    #[tokio::test]
    async fn timers_still_fire_and_clear() {
        let worker = worker().await;
        eval::<()>(
            &worker,
            "globalThis.fired = [];\
             setTimeout(() => fired.push('timeout'), 1);\
             clearTimeout(setTimeout(() => fired.push('cleared'), 1));\
             const every = setInterval(() => { fired.push('interval'); clearInterval(every); }, 1);\
             setImmediate(() => fired.push('immediate'));",
        )
        .await;
        worker.runtime.idle().await;
        let fired: Vec<String> = eval(&worker, "fired").await;
        assert_eq!(fired, ["immediate", "timeout", "interval"]);
        let thrown: String = eval(
            &worker,
            "(() => { try { setTimeout('not a function', 1); return 'no error'; } catch (e) { return 'threw'; } })()",
        )
        .await;
        assert_eq!(thrown, "threw");
        worker.shutdown(Duration::from_secs(1)).await;
    }
}
