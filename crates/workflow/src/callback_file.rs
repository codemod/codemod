//! Run a `forEachFile` callback kept in its own file (the file's default
//! export, `(file) => ...`) on one file, outside a workflow: what
//! `codemod jssg test --callback` tests. The callback runs as a
//! `forEachFile` step runs it, in an isolate with the same modules, the same
//! `file` object and the same `parse(file)`; what it did to the file is
//! returned, not committed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::sandbox::runtime_module::RuntimeEventCallback;
use codemod_sandbox::{SemanticProvider, WorkerOptions};
use rquickjs::{async_with, CatchResultExt, Function, Module, Promise};
use serde_json::Value;

use crate::tasks::for_each_file::{Effect, FileOutcome};
use crate::tasks::{changes, isolate};

/// One file to run a callback file on.
pub struct CallbackFileRequest {
    /// The callback's file.
    pub callback: PathBuf,
    /// Resolves the callback file's imports.
    pub resolver: Arc<OxcResolver>,
    /// The directory the file's path is relative to (a run's target).
    pub target: PathBuf,
    /// The file's path from `target`, with `/`: what `file.path` is.
    pub path: String,
    pub text: String,
    /// `file.language`, and the language `parse(file)` uses.
    pub language: Option<String>,
    /// Semantic analysis for roots `parse(file)` makes.
    pub semantic_provider: Option<Arc<dyn SemanticProvider>>,
    pub capabilities: HashSet<LlrtSupportedModules>,
    /// Receives `console` output and `codemod/runtime` events.
    pub runtime_event_callback: Option<RuntimeEventCallback>,
}

/// What the callback did to the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallbackFileRun {
    /// The text the file was last written with.
    pub written: Option<String>,
    /// Where the file was renamed to, from the target.
    pub renamed: Option<String>,
    pub deleted: bool,
    /// What the callback returned; `None` when it returned nothing.
    pub output: Option<Value>,
}

/// Run the callback file's default export on one file. An error is a
/// callback that could not be loaded, threw, or failed the file with
/// `codemod/runtime`.
pub async fn run_callback_file(request: CallbackFileRequest) -> Result<CallbackFileRun, String> {
    let CallbackFileRequest {
        callback,
        resolver,
        target,
        path,
        text,
        language,
        semantic_provider,
        capabilities,
        runtime_event_callback,
    } = request;
    // Imports resolve from the module's name: it must be absolute.
    let callback = dunce::canonicalize(&callback)
        .map_err(|e| format!("cannot read {}: {e}", callback.display()))?;
    let dir = callback.parent().unwrap_or(Path::new("."));
    let file_name = callback
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} is not a file", callback.display()))?;
    let module = callback_module(&file_name)?;
    let name = dir
        .join("__codemod_callback_file.js")
        .to_string_lossy()
        .into_owned();

    let options = WorkerOptions {
        capabilities,
        resolver,
        modules: isolate::isolate_modules(),
        codemod_modules: isolate::isolate_codemod_modules(),
        llm_request_handler: None,
        runtime_event_callback,
        cancel: Arc::new(AtomicBool::new(false)),
        dry_run: false,
        target_directory: target,
        default_language: language.clone(),
        semantic_provider,
        pending_files: None,
    };
    let worker = isolate::start(options, None).await?;
    let sandbox = &worker;
    let outcome: Result<FileOutcome, String> = async_with!(sandbox.context => |ctx| {
        let setup = async {
            let declared = Module::declare(ctx.clone(), name.as_str(), module.as_str())?;
            crate::run::set_import_meta(&declared, &callback)?;
            let (evaluated, promise) = declared.eval()?;
            promise.into_future::<()>().await?;
            let init: Function = evaluated.get("__init")?;
            init.call::<_, ()>(("{}", language.as_deref()))?;
            evaluated.get::<_, Function>("__runBatch")
        };
        let run_batch = setup
            .await
            .catch(&ctx)
            .map_err(|e| format!("could not load the callback: {e}"))?;
        let settled = async {
            let promise: Promise = run_batch.call((vec![path.as_str()], vec![text.as_str()]))?;
            promise.into_future::<String>().await
        };
        let json = settled.await.catch(&ctx).map_err(|e| e.to_string())?;
        let outcomes: Vec<FileOutcome> = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        outcomes
            .into_iter()
            .next()
            .ok_or_else(|| "the callback gave no outcome".to_string())
    })
    .await;
    let effects = worker.take_side_effects();
    worker.shutdown(SHUTDOWN_GRACE).await;

    let outcome = outcome?;
    if !outcome.ok {
        return Err(outcome
            .error
            .unwrap_or_else(|| "the callback failed".to_string()));
    }
    if let Some(failure) = effects.failure {
        return Err(failure.message);
    }
    if !effects.other_files.is_empty() {
        return Err("jssgTransform() is not supported in forEachFile callbacks yet; change each file from its own callback".to_string());
    }
    let mut run = CallbackFileRun {
        output: outcome.has_output.then_some(outcome.output),
        ..CallbackFileRun::default()
    };
    for effect in outcome.effects {
        match effect {
            Effect::Write { text } => run.written = Some(text),
            Effect::Rename { to } => {
                run.renamed = Some(changes::normalize(&to).ok_or_else(|| {
                    format!("cannot rename to '{to}': the path must be relative and stay inside the target")
                })?);
            }
            Effect::Delete => run.deleted = true,
        }
    }
    Ok(run)
}

/// How long the worker waits, once the callback is done, for work it left
/// running.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// The module the build extracts for an imported callback, importing
/// `file_name` from its own directory, with a clear error when the file's
/// default export is not a function.
fn callback_module(file_name: &str) -> Result<String, String> {
    let specifier = format!("./{file_name}");
    let module = format!(
        "import __imported from {specifier:?};\n\
         const __make = ({{  }}) => {{\n\
         \x20 if (typeof __imported !== \"function\") {{\n\
         \x20   throw new TypeError({message:?} + typeof __imported);\n\
         \x20 }}\n\
         \x20 return __imported;\n\
         }};\n{prelude}",
        message = format!(
            "{file_name} must default-export a forEachFile callback, (file) => ...; its default export is a "
        ),
        prelude = crate::js::CALLBACK_PRELUDE_TS,
    );
    crate::build::strip_types("callback.ts", &module, true)
}

#[cfg(test)]
mod tests {
    use super::callback_module;

    #[test]
    fn the_module_imports_the_callback_from_its_directory() {
        let module = callback_module("migrate.ts").unwrap();
        assert!(module.contains("from \"./migrate.ts\""), "{module}");
        assert!(module.contains("export function __runBatch"), "{module}");
    }
}
