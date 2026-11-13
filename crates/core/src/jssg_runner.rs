//! Run a JSSG transform over a target without the workflow engine.
//!
//! The runner never writes: it returns every change it would make, and the
//! caller decides whether and how to apply them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::{
    codemod_lang::CodemodLang, execution_engine::execute_codemod_with_quickjs,
    extract_selector_with_quickjs, CodemodOutput, ExecutionResult, JssgExecutionOptions,
    PendingFiles, SelectorEngineOptions,
};
use codemod_sandbox::sandbox::errors::{ExecutionError, RuntimeError};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::sandbox::runtime_module::{
    RuntimeEvent, RuntimeEventCallback, RuntimeEventKind,
};
use codemod_sandbox::utils::project_discovery::find_tsconfig;
use codemod_sandbox::{MetricsContext, MetricsData, SharedStateContext};
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;
use serde_json::Value;

use crate::execution::{CodemodExecutionConfig, ProgressCallback};

/// Semantic analysis available to the transform.
#[derive(Debug, Clone, Default)]
pub enum SemanticMode {
    #[default]
    None,
    File,
    Workspace(PathBuf),
}

pub struct JssgRunRequest {
    /// The transform file.
    pub script_path: PathBuf,
    pub target: PathBuf,
    /// Directory under `target` to restrict the run to.
    pub base_path: Option<PathBuf>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    /// Process exactly these files instead of walking.
    pub explicit_files: Option<Vec<PathBuf>>,
    pub language: String,
    pub params: HashMap<String, Value>,
    pub matrix: Option<HashMap<String, Value>>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    pub semantic: SemanticMode,
    pub threads: Option<usize>,
    /// Passed to the transform as `options.dryRun`.
    pub dry_run: bool,
    /// Seeds `getState` in the transform.
    pub initial_state: HashMap<String, Value>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub progress: Option<ProgressCallback>,
    /// Receives what the transform logged (`console`, `codemod/runtime`)
    /// for each file, once the file is done, so files never interleave.
    pub on_log: Option<JssgLogCallback>,
}

/// What a transform logged while processing one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JssgLog {
    /// `<file> :: <unit>`: the file relative to the target, and the unit the
    /// transform named with `setCurrentUnit` (the file itself until then).
    pub unit: String,
    /// The message: console output as written, or `[warn] …` / `[progress] …`
    /// with the event's metadata.
    pub line: String,
    pub warning: bool,
}

pub type JssgLogCallback = Arc<dyn Fn(Vec<JssgLog>) + Send + Sync>;

/// One file the transform changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JssgFileChange {
    pub path: PathBuf,
    pub original: String,
    pub content: String,
    pub rename_to: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct JssgRunOutcome {
    pub changes: Vec<JssgFileChange>,
    pub failures: Vec<(PathBuf, String)>,
    pub unmodified: usize,
    pub skipped: usize,
    /// Files the transform's selector matched.
    pub matched: Vec<PathBuf>,
    /// State the transform set; `Value::Null` marks a removed key.
    pub state: BTreeMap<String, Value>,
    pub metrics: MetricsData,
    pub warnings: Vec<String>,
}

thread_local! {
    static RUNTIME: tokio::runtime::Runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.with(|runtime| tokio::task::LocalSet::new().block_on(runtime, future))
}

pub fn run_jssg(request: JssgRunRequest) -> Result<JssgRunOutcome, String> {
    if !request.script_path.is_file() {
        return Err(format!(
            "transform '{}' does not exist",
            request.script_path.display()
        ));
    }
    let language: CodemodLang = request
        .language
        .parse()
        .map_err(|e| format!("invalid language '{}': {e}", request.language))?;
    let script_dir = request
        .script_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let resolver = Arc::new(
        OxcResolver::new(script_dir.clone(), find_tsconfig(&script_dir))
            .map_err(|e| format!("cannot set up module resolution: {e}"))?,
    );
    let target = request.target.clone();

    let mut outcome = JssgRunOutcome::default();
    let selector = match block_on(extract_selector_with_quickjs(SelectorEngineOptions {
        script_path: &request.script_path,
        language,
        resolver: Arc::clone(&resolver),
        capabilities: request.capabilities.clone(),
        target_directory: Some(&target),
    })) {
        Ok(selector) => selector.map(Arc::from),
        Err(error) if is_initialization_failure(&error) => {
            return Err(format!("cannot initialize the transform: {error}"));
        }
        Err(error) => {
            outcome
                .warnings
                .push(format!("cannot read the transform's selector: {error}"));
            None
        }
    };

    let config = CodemodExecutionConfig {
        pre_run_callback: None,
        progress_callback: Arc::new(request.progress.clone()),
        target_path: Some(target.clone()),
        base_path: request.base_path.clone(),
        include_globs: request.include.clone(),
        explicit_files: request.explicit_files.clone(),
        exclude_globs: request.exclude.clone(),
        dry_run: true,
        languages: Some(vec![request.language.clone()]),
        threads: request.threads,
        capabilities: request.capabilities.clone(),
    };

    let semantic: Option<Arc<dyn SemanticProvider>> = match &request.semantic {
        SemanticMode::None => None,
        SemanticMode::File => Some(Arc::new(LazySemanticProvider::file_scope())),
        SemanticMode::Workspace(root) => {
            let provider = Arc::new(LazySemanticProvider::workspace_scope(root.clone()));
            for file in config.collect_files() {
                if let Ok(content) = std::fs::read_to_string(&file) {
                    let _ = provider.notify_file_processed(&file, &content);
                }
            }
            Some(provider)
        }
    };

    let metrics = MetricsContext::new();
    let shared_state = SharedStateContext::with_initial_state(request.initial_state.clone());
    // Every file's text goes through here, so a file changed more than once
    // (by its own transform, and by other files' `jssgTransform` calls) ends
    // up with all of the changes.
    let pending = PendingFiles::default();
    let renames = Mutex::new(BTreeMap::<PathBuf, PathBuf>::new());
    let failures = Mutex::new(Vec::<(PathBuf, String)>::new());
    let matched = Mutex::new(Vec::<PathBuf>::new());
    let unmodified = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);

    let walk = config.execute(|file_path, _| {
        if !file_path.is_file() || abort.load(Ordering::SeqCst) {
            return;
        }
        if request
            .cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            return;
        }
        let Ok((content, taken)) = pending.take(file_path) else {
            skipped.fetch_add(1, Ordering::SeqCst);
            return;
        };
        let relative = file_path
            .strip_prefix(&target)
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");
        let logs = Arc::new(Mutex::new(Vec::<JssgLog>::new()));
        let runtime_event_callback = request.on_log.as_ref().map(|_| {
            let logs = Arc::clone(&logs);
            let unit = Mutex::new(relative.clone());
            let relative = relative.clone();
            let callback: RuntimeEventCallback = Arc::new(move |event: RuntimeEvent| {
                if event.kind == RuntimeEventKind::SetCurrentUnit {
                    *unit.lock().unwrap() = format!("{relative} :: {}", event.message);
                    return;
                }
                let current = unit.lock().unwrap().clone();
                logs.lock().unwrap().push(JssgLog {
                    unit: current,
                    line: format_runtime_event(&event),
                    warning: event.kind == RuntimeEventKind::Warn,
                });
            });
            callback
        });
        let result = block_on(execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &request.script_path,
            resolver: Arc::clone(&resolver),
            language,
            file_path,
            content: &content,
            selector_config: selector.clone(),
            params: Some(request.params.clone()),
            matrix_values: request.matrix.clone(),
            capabilities: request.capabilities.clone(),
            semantic_provider: semantic.clone(),
            metrics_context: Some(metrics.clone()),
            llm_request_handler: None,
            shared_state_context: Some(shared_state.clone()),
            runtime_event_callback,
            cancellation_flag: request.cancel.clone(),
            test_mode: false,
            dry_run: request.dry_run,
            target_directory: &target,
            pending_files: Some(pending.clone()),
        }));
        if let Some(on_log) = &request.on_log {
            let logs = std::mem::take(&mut *logs.lock().unwrap());
            if !logs.is_empty() {
                on_log(logs);
            }
        }
        match result {
            Ok(CodemodOutput { primary, secondary }) => {
                match primary {
                    ExecutionResult::Modified(modified) => {
                        matched.lock().unwrap().push(file_path.to_path_buf());
                        taken.set(modified.content);
                        if let Some(to) = modified.rename_to {
                            renames
                                .lock()
                                .unwrap()
                                .insert(taken.path().to_path_buf(), to);
                        }
                    }
                    ExecutionResult::Unmodified => {
                        if selector.is_some() {
                            matched.lock().unwrap().push(file_path.to_path_buf());
                        }
                        unmodified.fetch_add(1, Ordering::SeqCst);
                    }
                    ExecutionResult::Skipped => {
                        skipped.fetch_add(1, Ordering::SeqCst);
                    }
                }
                // What `jssgTransform` made of other files is in `pending`;
                // only their renames are left to note.
                for change in secondary {
                    if let ExecutionResult::Modified(modified) = change.result {
                        if let Some(to) = modified.rename_to {
                            let path = change.path.canonicalize().unwrap_or(change.path);
                            renames.lock().unwrap().insert(path, to);
                        }
                    }
                }
            }
            Err(error) => {
                if is_initialization_failure(&error) {
                    abort.store(true, Ordering::SeqCst);
                }
                failures
                    .lock()
                    .unwrap()
                    .push((file_path.to_path_buf(), error.to_string()));
            }
        }
    });
    walk.map_err(|e| e.to_string())?;

    let mut renames = renames.into_inner().unwrap();
    for (path, original, content) in pending.changed() {
        outcome.changes.push(JssgFileChange {
            rename_to: renames.remove(&path),
            path,
            original,
            content,
        });
    }
    // Files that were only renamed.
    for (path, to) in renames {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        outcome.changes.push(JssgFileChange {
            path,
            original: text.clone(),
            content: text,
            rename_to: Some(to),
        });
    }
    outcome.changes.sort_by(|a, b| a.path.cmp(&b.path));
    outcome.failures = failures.into_inner().unwrap();
    outcome.failures.sort();
    outcome.matched = matched.into_inner().unwrap();
    outcome.matched.sort();
    outcome.unmodified = unmodified.load(Ordering::SeqCst);
    outcome.skipped = skipped.load(Ordering::SeqCst);
    outcome.metrics = metrics.get_all();

    outcome.state = shared_state.changes_since(&request.initial_state);

    if abort.load(Ordering::SeqCst) {
        let first = outcome
            .failures
            .first()
            .map(|(_, e)| e.clone())
            .unwrap_or_default();
        return Err(format!("the transform failed to start: {first}"));
    }
    Ok(outcome)
}

/// A runtime event as a log line: console output as written, otherwise
/// `[warn]` / `[progress]` with the event's metadata.
/// A `codemod/runtime` event or `console` line as a step log line.
pub fn format_runtime_event(event: &RuntimeEvent) -> String {
    if event.meta.as_deref() == Some("console") {
        return event.message.clone();
    }
    let prefix = match event.kind {
        RuntimeEventKind::Warn => "[warn]",
        _ => "[progress]",
    };
    match &event.meta {
        Some(meta) => format!("{prefix} {} {meta}", event.message),
        None => format!("{prefix} {}", event.message),
    }
}

fn is_initialization_failure(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Runtime {
            source: RuntimeError::InitializationFailed { .. }
        }
    )
}
