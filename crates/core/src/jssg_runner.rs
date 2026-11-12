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
    SelectorEngineOptions,
};
use codemod_sandbox::sandbox::errors::{ExecutionError, RuntimeError};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
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
}

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
    let changes = Mutex::new(Vec::<JssgFileChange>::new());
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
        let Ok(content) = std::fs::read_to_string(file_path) else {
            skipped.fetch_add(1, Ordering::SeqCst);
            return;
        };
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
            runtime_event_callback: None,
            cancellation_flag: request.cancel.clone(),
            test_mode: false,
            dry_run: request.dry_run,
            target_directory: &target,
        }));
        match result {
            Ok(CodemodOutput { primary, secondary }) => {
                match &primary {
                    ExecutionResult::Modified(modified) => {
                        matched.lock().unwrap().push(file_path.to_path_buf());
                        changes.lock().unwrap().push(JssgFileChange {
                            path: file_path.to_path_buf(),
                            original: content.clone(),
                            content: modified.content.clone(),
                            rename_to: modified.rename_to.clone(),
                        });
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
                for change in secondary {
                    if let ExecutionResult::Modified(modified) = change.result {
                        let original = std::fs::read_to_string(&change.path).unwrap_or_default();
                        changes.lock().unwrap().push(JssgFileChange {
                            path: change.path,
                            original,
                            content: modified.content,
                            rename_to: modified.rename_to,
                        });
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

    outcome.changes = changes.into_inner().unwrap();
    outcome
        .changes
        .sort_by(|a, b| a.path.cmp(&b.path).then(a.rename_to.cmp(&b.rename_to)));
    outcome.failures = failures.into_inner().unwrap();
    outcome.failures.sort();
    outcome.matched = matched.into_inner().unwrap();
    outcome.matched.sort();
    outcome.unmodified = unmodified.load(Ordering::SeqCst);
    outcome.skipped = skipped.load(Ordering::SeqCst);
    outcome.metrics = metrics.get_all();

    let initial = &request.initial_state;
    for (key, value) in shared_state.get_persistable() {
        if initial.get(&key) != Some(&value) {
            outcome.state.insert(key, value);
        }
    }
    for key in shared_state.get_removals() {
        outcome.state.insert(key, Value::Null);
    }

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

fn is_initialization_failure(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Runtime {
            source: RuntimeError::InitializationFailed { .. }
        }
    )
}
