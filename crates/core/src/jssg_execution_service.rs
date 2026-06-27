use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use ast_grep_config::RuleConfig;
use butterflow_models::{
    step::{SemanticAnalysisConfig, SemanticAnalysisMode, UseJSAstGrep},
    DiffOperation, FieldDiff, Result, StateDiff, TaskExpressionContext,
};
use chrono::Utc;
use codemod_sandbox::sandbox::{
    engine::{
        codemod_lang::CodemodLang, execution_engine::execute_codemod_with_quickjs,
        extract_selector_with_quickjs, CodemodOutput, ExecutionResult, JssgExecutionOptions,
        SelectorEngineOptions,
    },
    errors::{ExecutionError as SandboxExecutionError, RuntimeError as SandboxRuntimeError},
    resolvers::OxcResolver,
    runtime_module::{RuntimeEventCallback, RuntimeEventKind},
};
use codemod_sandbox::{
    utils::project_discovery::find_tsconfig, MetricsContext, SharedStateContext,
};
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

use crate::{
    config::DryRunChange,
    engine::{
        auto_meta_files_include, await_js_ast_grep_execution_task, block_on_runtime_handle,
        build_js_ast_grep_idle_timeout_message, finish_unit_progress, format_runtime_event_log,
        format_runtime_failure_message, js_ast_grep_idle_timeout, record_output_progress,
        record_unit_progress, resolve_optional_glob_list, CapabilitiesData, Engine, StepPhase,
        StepProgressState,
    },
    execution::{CodemodExecutionConfig, PreRunCallback},
    progress_output::{
        append_buffered_diagnostic, append_buffered_log, flush_buffered_execution_output,
        BufferedExecutionOutput,
    },
    slog,
    structured_log::StructuredLogger,
    workflow_runtime::{publish_event, WorkflowEvent},
    Error,
};

pub(crate) struct JssgExecutionRequest<'a> {
    pub id: String,
    pub progress_task_id: Option<String>,
    pub step_id: String,
    pub step_name: String,
    pub report_step_id: Option<String>,
    pub report_step_name: Option<String>,
    pub js_ast_grep: &'a UseJSAstGrep,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub matrix_input: Option<HashMap<String, serde_json::Value>>,
    pub capabilities_data: &'a CapabilitiesData,
    pub bundle_path: &'a Option<PathBuf>,
    pub workflow_run_id: Option<Uuid>,
    pub initial_state: Option<&'a HashMap<String, serde_json::Value>>,
    pub logger: &'a StructuredLogger,
    pub modified_files_collector: Option<Arc<std::sync::Mutex<Vec<PathBuf>>>>,
    pub selector_matched_files_collector: Option<Arc<std::sync::Mutex<Vec<PathBuf>>>>,
    pub task_expr_ctx: Option<&'a TaskExpressionContext>,
}

pub(crate) struct JssgExecutionService<'a> {
    engine: &'a Engine,
}

impl<'a> JssgExecutionService<'a> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self { engine }
    }

    pub(crate) async fn execute(&self, request: JssgExecutionRequest<'_>) -> Result<()> {
        let metrics_context = self.engine.metrics_context.clone();
        let task_log_task_id = Uuid::parse_str(&request.id).ok();

        let effective_bundle_path = request
            .bundle_path
            .as_ref()
            .unwrap_or(&self.engine.workflow_run_config().execution.bundle_path);
        let js_file_path = crate::utils::resolve_workflow_path_within_root(
            effective_bundle_path,
            &request.js_ast_grep.js_file,
            "js-ast-grep.js_file",
        )?;

        let target_path = crate::utils::resolve_optional_workflow_path_within_root(
            &self.engine.workflow_run_config().execution.target_path,
            request.js_ast_grep.base_path.as_deref(),
            "js-ast-grep.base_path",
        )?;

        if let Some(pre_run_callback) = self
            .engine
            .workflow_run_config()
            .execution
            .pre_run_callback
            .as_deref()
        {
            pre_run_callback(
                target_path.as_path(),
                request.js_ast_grep.dry_run.unwrap_or(false),
                self.engine.workflow_run_config(),
            )
            .map_err(|error| Error::Other(format!("Pre-run check failed: {error}")))?;
        }

        if !js_file_path.exists() {
            return Err(Error::StepExecution(format!(
                "JavaScript file '{}' does not exist",
                js_file_path.display()
            )));
        }

        let script_base_dir = js_file_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let tsconfig_path = find_tsconfig(&script_base_dir);
        let resolver = Arc::new(
            OxcResolver::new(script_base_dir.clone(), tsconfig_path)
                .map_err(|e| Error::Other(format!("Failed to create resolver: {e}")))?,
        );

        let capabilities_security_callback_clone = request
            .capabilities_data
            .capabilities_security_callback
            .clone();
        let logger = request.logger.clone();
        let pre_run_callback = PreRunCallback {
            callback: Arc::new(Box::new(move |_, _, config| {
                if let Some(callback) = &capabilities_security_callback_clone {
                    callback(config).map_err(|e| {
                        slog!(logger, error, "Failed to check capabilities: {e}");
                        Box::<dyn std::error::Error + Send + Sync>::from(format!(
                            "Failed to check capabilities: {e}"
                        ))
                    })?;
                }
                Ok(())
            })),
        };

        let empty_params = HashMap::new();
        let empty_state = HashMap::new();
        let resolved_params_ref = request.params.as_ref().unwrap_or(&empty_params);
        let resolved_state_ref = request.initial_state.unwrap_or(&empty_state);

        let resolved_include = resolve_optional_glob_list(
            &request.js_ast_grep.include,
            resolved_params_ref,
            resolved_state_ref,
            request.matrix_input.as_ref(),
            request.task_expr_ctx,
        )?
        .or_else(|| auto_meta_files_include(resolved_state_ref, request.matrix_input.as_ref()));

        let resolved_exclude = resolve_optional_glob_list(
            &request.js_ast_grep.exclude,
            resolved_params_ref,
            resolved_state_ref,
            request.matrix_input.as_ref(),
            request.task_expr_ctx,
        )?;

        let explicit_files = request
            .matrix_input
            .as_ref()
            .and_then(|m| m.get("_meta_files"))
            .and_then(butterflow_models::variable::value_to_string_vec)
            .map(|files| -> Result<Vec<PathBuf>> {
                files
                    .into_iter()
                    .map(|file| {
                        crate::utils::resolve_workflow_path_within_root(
                            &target_path,
                            &file,
                            "matrix._meta_files",
                        )
                    })
                    .collect()
            })
            .transpose()?;

        let config = CodemodExecutionConfig {
            pre_run_callback: Some(pre_run_callback),
            progress_callback: self
                .engine
                .workflow_run_config()
                .execution
                .progress_callback
                .clone(),
            target_path: Some(target_path.clone()),
            base_path: None,
            include_globs: resolved_include,
            explicit_files,
            exclude_globs: resolved_exclude,
            dry_run: request.js_ast_grep.dry_run.unwrap_or(false)
                || self.engine.workflow_run_config().execution.dry_run,
            languages: Some(vec![request
                .js_ast_grep
                .language
                .clone()
                .unwrap_or("typescript".to_string())]),
            threads: request.js_ast_grep.max_threads,
            capabilities: request
                .capabilities_data
                .capabilities
                .as_ref()
                .map(|v| v.clone().into_iter().collect()),
        };

        let language = if let Some(lang_str) = &request.js_ast_grep.language {
            lang_str
                .parse()
                .map_err(|e| Error::StepExecution(format!("Invalid language '{lang_str}': {e}")))?
        } else {
            "typescript".parse().map_err(|e| {
                Error::StepExecution(format!("Failed to parse default language: {e}"))
            })?
        };

        let selector_config = match extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &js_file_path,
            language,
            resolver: Arc::clone(&resolver),
            capabilities: request
                .capabilities_data
                .capabilities
                .as_ref()
                .map(|v| v.clone().into_iter().collect()),
            target_directory: Some(&target_path),
        })
        .await
        {
            Ok(selector_config) => selector_config,
            Err(e) => {
                if Self::is_runtime_initialization_failure(&e) {
                    return Err(Error::StepExecution(format!(
                        "Failed to initialize js-ast-grep codemod: {e}"
                    )));
                }

                let message = format!("Failed to extract js-ast-grep selector: {e}");
                if let Some(task_id) = task_log_task_id {
                    let _ = self.engine.append_task_log(task_id, &message).await;
                }
                slog!(request.logger, warn, "{}", message);
                None
            }
        };

        let semantic_provider = self
            .build_semantic_provider(request.js_ast_grep, &target_path)
            .await?;
        self.pre_index_workspace_semantics(
            semantic_provider.as_ref(),
            &config,
            task_log_task_id,
            request.logger,
        )
        .await;

        self.execute_runtime(
            request,
            task_log_task_id,
            metrics_context,
            js_file_path,
            target_path,
            resolver,
            language,
            selector_config.map(Arc::from),
            semantic_provider,
            config,
        )
        .await
    }

    async fn build_semantic_provider(
        &self,
        js_ast_grep: &UseJSAstGrep,
        target_path: &Path,
    ) -> Result<Option<Arc<dyn SemanticProvider>>> {
        Ok(match &js_ast_grep.semantic_analysis {
            Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)) => {
                Some(Arc::new(LazySemanticProvider::file_scope()))
            }
            Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::Workspace)) => Some(Arc::new(
                LazySemanticProvider::workspace_scope(target_path.to_path_buf()),
            )),
            Some(SemanticAnalysisConfig::Detailed(detailed)) => match detailed.mode {
                SemanticAnalysisMode::File => Some(Arc::new(LazySemanticProvider::file_scope())),
                SemanticAnalysisMode::Workspace => {
                    let root = detailed
                        .root
                        .as_ref()
                        .map(|root| {
                            crate::utils::resolve_workflow_path_within_root(
                                target_path,
                                root,
                                "js-ast-grep.semantic_analysis.root",
                            )
                        })
                        .transpose()?
                        .unwrap_or_else(|| target_path.to_path_buf());
                    Some(Arc::new(LazySemanticProvider::workspace_scope(root)))
                }
            },
            None => None,
        })
    }

    async fn pre_index_workspace_semantics(
        &self,
        provider: Option<&Arc<dyn SemanticProvider>>,
        config: &CodemodExecutionConfig,
        task_log_task_id: Option<Uuid>,
        logger: &StructuredLogger,
    ) {
        let Some(provider) = provider else {
            return;
        };
        if provider.mode() != language_core::ProviderMode::WorkspaceScope {
            return;
        }

        let target_files: Vec<PathBuf> = config.collect_files();
        if let Some(task_id) = task_log_task_id {
            let _ = self
                .engine
                .append_task_log(
                    task_id,
                    format!(
                        "Preparing workspace semantic index for {} file(s)",
                        target_files.len()
                    ),
                )
                .await;
        }

        for file_path in &target_files {
            if file_path.is_file() {
                if let Ok(content) = std::fs::read_to_string(file_path) {
                    if let Err(e) = provider.notify_file_processed(file_path, &content) {
                        slog!(
                            logger,
                            debug,
                            "Failed to pre-index file {} for semantic analysis: {}",
                            file_path.display(),
                            e
                        );
                    }
                }
            }
        }

        if let Some(task_id) = task_log_task_id {
            let _ = self
                .engine
                .append_task_log(task_id, "Workspace semantic index ready")
                .await;
        }
    }

    fn signal_progress(progress_tx: &mpsc::UnboundedSender<()>) {
        let _ = progress_tx.send(());
    }

    fn is_runtime_initialization_failure(error: &SandboxExecutionError) -> bool {
        matches!(
            error,
            SandboxExecutionError::Runtime {
                source: SandboxRuntimeError::InitializationFailed { .. }
            }
        )
    }

    fn is_codemod_source_reference_failure(
        error: &SandboxExecutionError,
        script_path: &Path,
    ) -> bool {
        let SandboxExecutionError::Runtime {
            source: SandboxRuntimeError::ExecutionFailed { message },
        } = error
        else {
            return false;
        };

        if !message.contains(" is not defined") {
            return false;
        }

        let script_path = script_path.to_string_lossy();
        message
            .lines()
            .map(str::trim)
            .any(|line| line.starts_with("at ") && line.contains(script_path.as_ref()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_runtime(
        &self,
        request: JssgExecutionRequest<'_>,
        task_log_task_id: Option<Uuid>,
        metrics_context: MetricsContext,
        js_file_path: PathBuf,
        target_path: PathBuf,
        resolver: Arc<OxcResolver>,
        language: CodemodLang,
        selector_config: Option<Arc<RuleConfig<CodemodLang>>>,
        semantic_provider: Option<Arc<dyn SemanticProvider>>,
        config: CodemodExecutionConfig,
    ) -> Result<()> {
        let runtime_handle = tokio::runtime::Handle::current();
        let js_file_path_clone = js_file_path.clone();
        let resolver_clone = resolver.clone();
        let request_id = request.id.clone();
        let progress_task_id = request
            .progress_task_id
            .clone()
            .unwrap_or_else(|| request_id.clone());
        let id_clone = Arc::new(progress_task_id.clone());
        let progress_callback = self
            .engine
            .workflow_run_config()
            .execution
            .progress_callback
            .clone();
        let progress_callback_for_closure = progress_callback.clone();
        let file_writer = self.engine.file_writer();
        let shared_state_context = if let Some(state) = request.initial_state {
            SharedStateContext::with_initial_state(state.clone())
        } else {
            SharedStateContext::new()
        };
        let metrics_context_clone = metrics_context.clone();
        let shared_state_context_clone = shared_state_context.clone();
        let logger = request.logger.clone();
        let modified_files_collector_clone = request.modified_files_collector.clone();
        let selector_matched_files_collector_clone =
            request.selector_matched_files_collector.clone();
        let target_path_for_logs = target_path.clone();
        let canceled_during_execution = Arc::new(AtomicBool::new(false));
        let idle_timeout = js_ast_grep_idle_timeout();
        let progress_state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
        let idle_timed_out = Arc::new(AtomicBool::new(false));
        let idle_notify = Arc::new(Notify::new());
        let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));
        let has_selector = selector_config.is_some();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<()>();

        let deferred_deletions: Arc<std::sync::Mutex<Vec<PathBuf>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let logger_for_deferred = logger.clone();
        let deferred_deletions_clone = Arc::clone(&deferred_deletions);
        let canceled_flag_for_closure = Arc::clone(&canceled_during_execution);
        let progress_state_for_closure = Arc::clone(&progress_state);
        let progress_tx_for_closure = progress_tx.clone();

        if let Some(task_id) = task_log_task_id {
            let progress_state_for_output = Arc::clone(&progress_state);
            let progress_tx_for_output = progress_tx.clone();
            self.engine.register_output_heartbeat(
                task_id,
                Arc::new(move || {
                    record_output_progress(&progress_state_for_output);
                    Self::signal_progress(&progress_tx_for_output);
                }),
            );
        }

        let progress_state_for_watchdog = Arc::clone(&progress_state);
        let idle_timed_out_for_watchdog = Arc::clone(&idle_timed_out);
        let idle_notify_for_watchdog = Arc::clone(&idle_notify);
        let idle_failure_message_for_watchdog = Arc::clone(&idle_failure_message);
        let state_adapter_for_watchdog = self.engine.state_adapter();
        let watchdog_task = tokio::spawn(async move {
            let sleep = tokio::time::sleep(idle_timeout);
            tokio::pin!(sleep);

            loop {
                tokio::select! {
                    _ = &mut sleep => {
                        let message = {
                            let snapshot = progress_state_for_watchdog
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            build_js_ast_grep_idle_timeout_message(&snapshot, idle_timeout)
                        };

                        idle_timed_out_for_watchdog.store(true, Ordering::Release);
                        if let Ok(mut slot) = idle_failure_message_for_watchdog.lock() {
                            *slot = Some(message.clone());
                        }
                        idle_notify_for_watchdog.notify_waiters();

                        if let Some(task_id) = task_log_task_id {
                            let mut adapter = state_adapter_for_watchdog.lock().await;
                            if let Ok(mut task) = adapter.get_task(task_id).await {
                                task.logs.push(message.clone());
                                let _ = adapter.save_task(&task).await;
                                publish_event(
                                    task.workflow_run_id,
                                    WorkflowEvent::TaskLogAppended {
                                        workflow_run_id: task.workflow_run_id,
                                        task_id,
                                        line: message,
                                        at: Utc::now(),
                                    },
                                );
                            }
                        }
                        break;
                    }
                    maybe_progress = progress_rx.recv() => {
                        if maybe_progress.is_none() {
                            break;
                        }
                        sleep
                            .as_mut()
                            .reset(tokio::time::Instant::now() + idle_timeout);
                    }
                }
            }
        });

        Self::signal_progress(&progress_tx);

        if let Some(task_id) = task_log_task_id {
            self.engine
                .register_step_cancel_signal(task_id, Arc::clone(&canceled_during_execution));
        }

        let idle_timed_out_for_closure = Arc::clone(&idle_timed_out);
        let idle_notify_for_closure = Arc::clone(&idle_notify);
        let idle_failure_message_for_closure = Arc::clone(&idle_failure_message);
        let runtime_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));
        let runtime_failure_message_for_closure = Arc::clone(&runtime_failure_message);
        let execution_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));
        let execution_failure_message_for_closure = Arc::clone(&execution_failure_message);
        let buffered_execution_output =
            Arc::new(std::sync::Mutex::new(BufferedExecutionOutput::default()));
        let attempted_file_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let succeeded_file_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let failed_file_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let engine = self.engine;
        let step_id = request.step_id.clone();
        let step_name = request.step_name.clone();
        let report_step_id = request.report_step_id.clone();
        let report_step_name = request.report_step_name.clone();
        let params = request.params;
        let matrix_input = request.matrix_input;
        let workflow_run_id = request.workflow_run_id;
        let buffered_execution_output_for_closure = Arc::clone(&buffered_execution_output);
        let attempted_file_count_for_closure = Arc::clone(&attempted_file_count);
        let succeeded_file_count_for_closure = Arc::clone(&succeeded_file_count);
        let failed_file_count_for_closure = Arc::clone(&failed_file_count);

        let execute_result = config
            .execute_with_task_id_before_finish(
                &progress_task_id,
                move |file_path, config| {
                    if canceled_flag_for_closure.load(Ordering::Acquire)
                        || idle_timed_out_for_closure.load(Ordering::Acquire)
                    {
                        return;
                    }

                    if !file_path.is_file() {
                        return;
                    }
                    attempted_file_count_for_closure.fetch_add(1, Ordering::Relaxed);

                    let relative_path = file_path
                        .strip_prefix(&target_path_for_logs)
                        .unwrap_or(file_path)
                        .display()
                        .to_string();
                    record_unit_progress(
                        &progress_state_for_closure,
                        &relative_path,
                        StepPhase::FileQueued,
                    );
                    Self::signal_progress(&progress_tx_for_closure);

                    let content = match std::fs::read_to_string(file_path) {
                        Ok(content) => content,
                        Err(e) => {
                            slog!(
                                logger,
                                warn,
                                "Failed to read file {}: {}",
                                file_path.display(),
                                e
                            );
                            finish_unit_progress(
                                &progress_state_for_closure,
                                &relative_path,
                                StepPhase::ExecutionErrored,
                            );
                            failed_file_count_for_closure.fetch_add(1, Ordering::Relaxed);
                            let execution_message = format!(
                                "Failed to process {relative_path}: Failed to read file: {e}"
                            );
                            append_buffered_diagnostic(
                                &buffered_execution_output_for_closure,
                                relative_path.clone(),
                                format!("Failed to read file: {e}"),
                            );
                            if let Ok(mut execution_failure_message) =
                                execution_failure_message_for_closure.lock()
                            {
                                if execution_failure_message.is_none() {
                                    *execution_failure_message = Some(execution_message.clone());
                                }
                            }
                            if let (Some(task_id), Some(run_id)) =
                                (task_log_task_id, workflow_run_id)
                            {
                                publish_event(
                                    run_id,
                                    WorkflowEvent::TaskLogAppended {
                                        workflow_run_id: run_id,
                                        task_id,
                                        line: execution_message,
                                        at: Utc::now(),
                                    },
                                );
                            }
                            engine
                                .execution_stats
                                .files_with_errors
                                .fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                    };
                    record_unit_progress(
                        &progress_state_for_closure,
                        &relative_path,
                        StepPhase::FileLoaded,
                    );
                    Self::signal_progress(&progress_tx_for_closure);

                    std::env::set_var("CODEMOD_STEP_ID", &step_id);
                    record_unit_progress(
                        &progress_state_for_closure,
                        &relative_path,
                        StepPhase::ExecutionStarted,
                    );
                    Self::signal_progress(&progress_tx_for_closure);
                    let dry_run = config.dry_run;
                    let relative_path_for_execution = relative_path.clone();
                    let progress_state_for_execution = Arc::clone(&progress_state_for_closure);
                    let cancellation_flag_for_execution = Arc::clone(&canceled_flag_for_closure);
                    let current_runtime_unit =
                        Arc::new(std::sync::Mutex::new(relative_path.clone()));
                    let current_runtime_unit_for_callback = Arc::clone(&current_runtime_unit);
                    let progress_state_for_runtime_events = Arc::clone(&progress_state_for_closure);
                    let relative_path_for_runtime_events = relative_path.clone();
                    let buffered_execution_output_for_runtime_events =
                        Arc::clone(&buffered_execution_output_for_closure);
                    let runtime_event_task_id = task_log_task_id;
                    let runtime_event_run_id = workflow_run_id;
                    let progress_tx_for_runtime_events = progress_tx_for_closure.clone();
                    let runtime_event_callback: RuntimeEventCallback =
                        Arc::new(move |event| match event.kind {
                            RuntimeEventKind::SetCurrentUnit => {
                                let new_runtime_unit = format!(
                                    "{relative_path_for_runtime_events} :: {}",
                                    event.message
                                );
                                let previous_runtime_unit = {
                                    let mut current_runtime_unit =
                                        current_runtime_unit_for_callback
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    let previous_runtime_unit = current_runtime_unit.clone();
                                    *current_runtime_unit = new_runtime_unit.clone();
                                    previous_runtime_unit
                                };

                                finish_unit_progress(
                                    &progress_state_for_runtime_events,
                                    &previous_runtime_unit,
                                    StepPhase::ExecutionFinished,
                                );
                                record_unit_progress(
                                    &progress_state_for_runtime_events,
                                    &new_runtime_unit,
                                    StepPhase::ExecutionStarted,
                                );
                                Self::signal_progress(&progress_tx_for_runtime_events);
                            }
                            RuntimeEventKind::Progress | RuntimeEventKind::Warn => {
                                let runtime_unit = current_runtime_unit_for_callback
                                    .lock()
                                    .map(|runtime_unit| runtime_unit.clone())
                                    .unwrap_or_else(|_| relative_path_for_runtime_events.clone());
                                let formatted_log = format_runtime_event_log(&event);
                                record_unit_progress(
                                    &progress_state_for_runtime_events,
                                    &runtime_unit,
                                    StepPhase::Output,
                                );
                                Self::signal_progress(&progress_tx_for_runtime_events);
                                if let Some(message) = formatted_log.as_ref() {
                                    append_buffered_log(
                                        &buffered_execution_output_for_runtime_events,
                                        runtime_unit.clone(),
                                        message.clone(),
                                    );
                                }
                                if let (Some(task_id), Some(run_id), Some(message)) =
                                    (runtime_event_task_id, runtime_event_run_id, formatted_log)
                                {
                                    publish_event(
                                        run_id,
                                        WorkflowEvent::TaskLogAppended {
                                            workflow_run_id: run_id,
                                            task_id,
                                            line: message,
                                            at: Utc::now(),
                                        },
                                    );
                                }
                            }
                        });
                    let execution_result = block_on_runtime_handle(&runtime_handle, async {
                        let local = tokio::task::LocalSet::new();
                        let file_path_owned = file_path.to_path_buf();
                        let content_owned = content.clone();
                        let js_file_path_owned = js_file_path_clone.clone();
                        let resolver_owned = resolver_clone.clone();
                        let selector_config_owned = selector_config.clone();
                        let params_owned = params.clone();
                        let matrix_input_owned = matrix_input.clone();
                        let capabilities_owned = config.capabilities.clone();
                        let semantic_provider_owned = semantic_provider.clone();
                        let metrics_context_owned = metrics_context_clone.clone();
                        let shared_state_context_owned = shared_state_context_clone.clone();
                        let target_path_owned = target_path.clone();
                        let idle_timed_out = Arc::clone(&idle_timed_out_for_closure);
                        let idle_notify = Arc::clone(&idle_notify_for_closure);
                        let idle_failure_message = Arc::clone(&idle_failure_message_for_closure);

                        local
                            .run_until(async move {
                                let execution_task = tokio::task::spawn_local(async move {
                                    execute_codemod_with_quickjs(JssgExecutionOptions {
                                        script_path: &js_file_path_owned,
                                        resolver: resolver_owned,
                                        language,
                                        file_path: &file_path_owned,
                                        content: &content_owned,
                                        selector_config: selector_config_owned,
                                        params: params_owned,
                                        matrix_values: matrix_input_owned,
                                        capabilities: capabilities_owned,
                                        semantic_provider: semantic_provider_owned,
                                        metrics_context: Some(metrics_context_owned),
                                        shared_state_context: Some(shared_state_context_owned),
                                        runtime_event_callback: Some(runtime_event_callback),
                                        cancellation_flag: Some(cancellation_flag_for_execution),
                                        test_mode: false,
                                        dry_run,
                                        target_directory: &target_path_owned,
                                    })
                                    .await
                                });

                                await_js_ast_grep_execution_task(
                                    execution_task,
                                    idle_timed_out,
                                    idle_notify,
                                    idle_failure_message,
                                    progress_state_for_execution,
                                    idle_timeout,
                                    &relative_path_for_execution,
                                )
                                .await
                            })
                            .await
                    });

                    if canceled_flag_for_closure.load(Ordering::Acquire) {
                        finish_unit_progress(
                            &progress_state_for_closure,
                            &relative_path,
                            StepPhase::ExecutionErrored,
                        );
                        return;
                    }

                    match execution_result {
                        Ok(Ok(CodemodOutput { primary, secondary })) => {
                            succeeded_file_count_for_closure.fetch_add(1, Ordering::Relaxed);
                            let apply_change =
                                |change_path: &Path, result: &ExecutionResult| match result {
                                    ExecutionResult::Modified(ref modified) => {
                                        let write_path =
                                            modified.rename_to.as_deref().unwrap_or(change_path);
                                        if config.dry_run {
                                            engine
                                                .execution_stats
                                                .files_modified
                                                .fetch_add(1, Ordering::Relaxed);

                                            if let Some(callback) = &engine
                                                .workflow_run_config()
                                                .output
                                                .dry_run_callback
                                            {
                                                let original = if change_path == file_path {
                                                    content.clone()
                                                } else {
                                                    std::fs::read_to_string(change_path)
                                                        .unwrap_or_default()
                                                };
                                                callback(DryRunChange {
                                                    file_path: change_path.to_path_buf(),
                                                    original_content: original,
                                                    new_content: modified.content.clone(),
                                                    step_id: Some(step_id.clone()),
                                                    step_name: Some(step_name.clone()),
                                                    parent_step_id: report_step_id.clone(),
                                                    parent_step_name: report_step_name.clone(),
                                                });
                                            }

                                            slog!(
                                                logger,
                                                debug,
                                                "Would modify file (dry run): {}",
                                                change_path.display()
                                            );
                                        } else {
                                            if let Some(callback) = &engine
                                                .workflow_run_config()
                                                .output
                                                .dry_run_callback
                                            {
                                                let original = if change_path == file_path {
                                                    content.clone()
                                                } else {
                                                    std::fs::read_to_string(change_path)
                                                        .unwrap_or_default()
                                                };
                                                callback(DryRunChange {
                                                    file_path: change_path.to_path_buf(),
                                                    original_content: original,
                                                    new_content: modified.content.clone(),
                                                    step_id: Some(step_id.clone()),
                                                    step_name: Some(step_name.clone()),
                                                    parent_step_id: report_step_id.clone(),
                                                    parent_step_name: report_step_name.clone(),
                                                });
                                            }

                                            let write_result =
                                                block_on_runtime_handle(&runtime_handle, async {
                                                    file_writer
                                                        .write_file(
                                                            write_path.to_path_buf(),
                                                            modified.content.clone(),
                                                        )
                                                        .await
                                                });

                                            if let Err(e) = write_result {
                                                slog!(
                                                    logger,
                                                    error,
                                                    "Failed to write modified file {}: {}",
                                                    write_path.display(),
                                                    e
                                                );
                                                engine
                                                    .execution_stats
                                                    .files_with_errors
                                                    .fetch_add(1, Ordering::Relaxed);
                                            } else {
                                                if modified.rename_to.is_some()
                                                    && write_path != change_path
                                                {
                                                    if let Ok(mut deletions) =
                                                        deferred_deletions_clone.lock()
                                                    {
                                                        deletions.push(change_path.to_path_buf());
                                                    }
                                                    slog!(
                                                    logger,
                                                    debug,
                                                    "Renamed file: {} -> {} (deferred deletion)",
                                                    change_path.display(),
                                                    write_path.display()
                                                );
                                                } else {
                                                    slog!(
                                                        logger,
                                                        debug,
                                                        "Modified file: {}",
                                                        change_path.display()
                                                    );
                                                }
                                                if let Some(ref provider) = semantic_provider {
                                                    let _ = provider.notify_file_processed(
                                                        write_path,
                                                        &modified.content,
                                                    );
                                                }
                                                engine
                                                    .execution_stats
                                                    .files_modified
                                                    .fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                    }
                                    ExecutionResult::Unmodified | ExecutionResult::Skipped => {}
                                };

                            match &primary {
                                ExecutionResult::Modified(_) => {
                                    apply_change(file_path, &primary);
                                    if let Some(ref collector) = modified_files_collector_clone {
                                        collector
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                                            .push(file_path.to_path_buf());
                                    }
                                    if let Some(ref collector) =
                                        selector_matched_files_collector_clone
                                    {
                                        collector
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                                            .push(file_path.to_path_buf());
                                    }
                                }
                                ExecutionResult::Unmodified => {
                                    if has_selector {
                                        if let Some(ref collector) =
                                            selector_matched_files_collector_clone
                                        {
                                            collector
                                                .lock()
                                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                                .push(file_path.to_path_buf());
                                        }
                                    }
                                    engine
                                        .execution_stats
                                        .files_unmodified
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                ExecutionResult::Skipped => {
                                    engine
                                        .execution_stats
                                        .files_unmodified
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                            }

                            for change in &secondary {
                                apply_change(&change.path, &change.result);
                            }

                            finish_unit_progress(
                                &progress_state_for_closure,
                                &current_runtime_unit
                                    .lock()
                                    .map(|runtime_unit| runtime_unit.clone())
                                    .unwrap_or_else(|_| relative_path.clone()),
                                StepPhase::ExecutionFinished,
                            );
                            Self::signal_progress(&progress_tx_for_closure);
                        }
                        Ok(Err(e)) => {
                            let runtime_unit = current_runtime_unit
                                .lock()
                                .map(|runtime_unit| runtime_unit.clone())
                                .unwrap_or_else(|_| relative_path.clone());
                            finish_unit_progress(
                                &progress_state_for_closure,
                                &runtime_unit,
                                StepPhase::ExecutionErrored,
                            );
                            if let SandboxExecutionError::RuntimeHook { source } = &e {
                                let message = format_runtime_failure_message(source);
                                if let (Some(task_id), Some(run_id)) =
                                    (task_log_task_id, workflow_run_id)
                                {
                                    publish_event(
                                        run_id,
                                        WorkflowEvent::TaskLogAppended {
                                            workflow_run_id: run_id,
                                            task_id,
                                            line: message.clone(),
                                            at: Utc::now(),
                                        },
                                    );
                                }
                                canceled_flag_for_closure.store(true, Ordering::Release);
                                if let Ok(mut runtime_failure_message) =
                                    runtime_failure_message_for_closure.lock()
                                {
                                    if runtime_failure_message.is_none() {
                                        *runtime_failure_message = Some(message);
                                    }
                                }
                            } else if Self::is_runtime_initialization_failure(&e)
                                || Self::is_codemod_source_reference_failure(
                                    &e,
                                    &js_file_path_clone,
                                )
                            {
                                canceled_flag_for_closure.store(true, Ordering::Release);
                            }
                            slog!(
                                logger,
                                error,
                                "Failed to execute codemod on {}: {}",
                                relative_path,
                                e
                            );
                            let execution_message =
                                format!("Failed to process {relative_path}: {e}");
                            failed_file_count_for_closure.fetch_add(1, Ordering::Relaxed);
                            append_buffered_diagnostic(
                                &buffered_execution_output_for_closure,
                                runtime_unit.clone(),
                                e.to_string(),
                            );
                            if let Ok(mut execution_failure_message) =
                                execution_failure_message_for_closure.lock()
                            {
                                if execution_failure_message.is_none() {
                                    *execution_failure_message = Some(execution_message.clone());
                                }
                            }
                            if let (Some(task_id), Some(run_id)) =
                                (task_log_task_id, workflow_run_id)
                            {
                                publish_event(
                                    run_id,
                                    WorkflowEvent::TaskLogAppended {
                                        workflow_run_id: run_id,
                                        task_id,
                                        line: execution_message,
                                        at: Utc::now(),
                                    },
                                );
                            }
                            engine
                                .execution_stats
                                .files_with_errors
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            let runtime_unit = current_runtime_unit
                                .lock()
                                .map(|runtime_unit| runtime_unit.clone())
                                .unwrap_or_else(|_| relative_path.clone());
                            finish_unit_progress(
                                &progress_state_for_closure,
                                &runtime_unit,
                                StepPhase::ExecutionErrored,
                            );
                            slog!(
                                logger,
                                error,
                                "Failed to execute codemod on {}: {}",
                                relative_path,
                                e
                            );
                            let execution_message =
                                format!("Failed to process {relative_path}: {e}");
                            failed_file_count_for_closure.fetch_add(1, Ordering::Relaxed);
                            append_buffered_diagnostic(
                                &buffered_execution_output_for_closure,
                                runtime_unit.clone(),
                                e.to_string(),
                            );
                            if let Ok(mut execution_failure_message) =
                                execution_failure_message_for_closure.lock()
                            {
                                if execution_failure_message.is_none() {
                                    *execution_failure_message = Some(execution_message.clone());
                                }
                            }
                            if let (Some(task_id), Some(run_id)) =
                                (task_log_task_id, workflow_run_id)
                            {
                                publish_event(
                                    run_id,
                                    WorkflowEvent::TaskLogAppended {
                                        workflow_run_id: run_id,
                                        task_id,
                                        line: execution_message,
                                        at: Utc::now(),
                                    },
                                );
                            }
                            engine
                                .execution_stats
                                .files_with_errors
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    if let Some(callback) = progress_callback_for_closure.as_ref() {
                        let callback = callback.callback.clone();
                        callback(
                            &id_clone,
                            &file_path.to_string_lossy(),
                            "next",
                            Some(&1),
                            &0,
                        );
                    }
                },
                || {
                    flush_buffered_execution_output(
                        &buffered_execution_output,
                        &progress_callback,
                        &request_id,
                    );
                },
            )
            .map_err(|e| Error::StepExecution(e.to_string()));

        if let Some(task_id) = task_log_task_id {
            self.engine.unregister_output_heartbeat(task_id);
            self.engine.unregister_step_cancel_signal(task_id);
        }
        drop(progress_tx);
        watchdog_task.abort();
        let _ = watchdog_task.await;

        if idle_timed_out.load(Ordering::Acquire) {
            let message = idle_failure_message
                .lock()
                .ok()
                .and_then(|message| message.clone())
                .unwrap_or_else(|| {
                    let snapshot = progress_state.lock().ok();
                    snapshot
                        .as_deref()
                        .map(|state| build_js_ast_grep_idle_timeout_message(state, idle_timeout))
                        .unwrap_or_else(|| {
                            format!(
                                "No progress observed for {}s during js-ast-grep execution",
                                idle_timeout.as_secs()
                            )
                        })
                });
            return Err(Error::Runtime(message));
        }

        if let Err(error) = execute_result {
            flush_buffered_execution_output(
                &buffered_execution_output,
                &progress_callback,
                &request_id,
            );
            return Err(error);
        }

        if let Some(message) = runtime_failure_message
            .lock()
            .ok()
            .and_then(|message| message.clone())
        {
            return Err(Error::StepExecution(message));
        }

        let attempted_files = attempted_file_count.load(Ordering::Relaxed);
        let failed_files = failed_file_count.load(Ordering::Relaxed);
        let succeeded_files = succeeded_file_count.load(Ordering::Relaxed);
        if attempted_files > 0 && failed_files == attempted_files && succeeded_files == 0 {
            if let Some(message) = execution_failure_message
                .lock()
                .ok()
                .and_then(|message| message.clone())
            {
                return Err(Error::StepExecution(message));
            }
        }

        if canceled_during_execution.load(Ordering::Acquire) {
            if let Some(message) = execution_failure_message
                .lock()
                .ok()
                .and_then(|message| message.clone())
            {
                return Err(Error::StepExecution(message));
            }
            return Err(Error::Runtime("Canceled by user".to_string()));
        }

        if let Ok(deletions) = deferred_deletions.lock() {
            for path in deletions.iter() {
                if let Err(e) = std::fs::remove_file(path) {
                    slog!(
                        logger_for_deferred,
                        error,
                        "Failed to remove original file {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }

        if let Some(wf_run_id) = workflow_run_id {
            if !self
                .engine
                .workflow_run_config()
                .execution
                .skip_state_writes
                && !config.dry_run
            {
                let persistable = shared_state_context.get_persistable();
                let removals = shared_state_context.get_removals();

                if !persistable.is_empty() || !removals.is_empty() {
                    let mut fields = HashMap::new();
                    for (key, value) in persistable {
                        fields.insert(
                            key,
                            FieldDiff {
                                operation: DiffOperation::Update,
                                value: Some(value),
                            },
                        );
                    }
                    for key in removals {
                        fields.insert(
                            key,
                            FieldDiff {
                                operation: DiffOperation::Remove,
                                value: None,
                            },
                        );
                    }

                    self.engine
                        .state_adapter()
                        .lock()
                        .await
                        .apply_state_diff(&StateDiff {
                            workflow_run_id: wf_run_id,
                            fields,
                        })
                        .await?;
                }
            }
        }

        Ok(())
    }
}
