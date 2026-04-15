use butterflow_models::schema::resolve_values_with_default;
use codemod_ai::execute::{execute_ai_step, ExecuteAiStepConfig};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

use crate::ai_handoff::{
    build_agent_command, detect_parent_coding_agent, discover_installed_agents,
    find_agent_executable, resolve_agent_name, DetectionConfidence,
};
use crate::config::{
    CapabilitiesSecurityCallback, DryRunChange, InstallSkillExecutionRequest,
    ShellCommandExecutionRequest, WorkflowRunConfig,
};
use crate::execution::{CodemodExecutionConfig, PreRunCallback, ProgressCallback};
use crate::execution_stats::ExecutionStats;
use crate::file_ops::AsyncFileWriter;
use crate::slog;
use crate::structured_log::{StdoutCaptureGuard, StepContext, StructuredLogger};
use crate::utils::validate_workflow;
use chrono::Utc;
use codemod_sandbox::sandbox::engine::{
    extract_selector_with_quickjs, CodemodOutput, ExecutionResult, JssgExecutionOptions,
    SelectorEngineOptions,
};
use codemod_sandbox::sandbox::errors::ExecutionError as SandboxExecutionError;
use codemod_sandbox::sandbox::runtime_module::{
    RuntimeEvent, RuntimeEventCallback, RuntimeEventKind, RuntimeFailure, RuntimeFailureKind,
};
use codemod_sandbox::{scan_file_with_combined_scan, with_combined_scan};
use log::{debug, error, info, warn};
use std::path::Path;
use tokio::fs::read_to_string;
use tokio::sync::Mutex;
use tokio::time;
use uuid::Uuid;

use crate::registry::ResolvedPackage;
use butterflow_models::runtime::RuntimeType;

use butterflow_models::step::{
    SemanticAnalysisConfig, SemanticAnalysisMode, StepAction, UseAI, UseAstGrep, UseCodemod,
    UseJSAstGrep,
};
use butterflow_models::{
    evaluate_condition, resolve_string_list, resolve_string_with_expression, DiffOperation, Error,
    FieldDiff, Node, Result, StateDiff, Strategy, Task, TaskDiff, TaskExpressionContext,
    TaskStatus, Workflow, WorkflowRun, WorkflowRunDiff, WorkflowStatus,
};
use butterflow_runners::direct_runner::DirectRunner;
#[cfg(feature = "docker")]
use butterflow_runners::docker_runner::DockerRunner;
#[cfg(feature = "podman")]
use butterflow_runners::podman_runner::PodmanRunner;
use butterflow_runners::{OutputCallback, Runner};
use butterflow_scheduler::Scheduler;
use butterflow_state::local_adapter::LocalStateAdapter;
use butterflow_state::StateAdapter;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::{
    sandbox::{engine::execution_engine::execute_codemod_with_quickjs, resolvers::OxcResolver},
    utils::project_discovery::find_tsconfig,
    MetricsContext, SharedStateContext,
};
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;

/// True when every task for each `depends_on` node is [`TaskStatus::Completed`], matching
/// [`Scheduler::find_runnable_tasks_internal`]. Used so matrix masters are not marked
/// completed while dependency nodes (e.g. shard evaluation) are still running.
fn workflow_node_dependencies_satisfied(workflow: &Workflow, tasks: &[Task], node_id: &str) -> bool {
    let Some(node) = workflow.nodes.iter().find(|n| n.id == node_id) else {
        return false;
    };
    for dep_id in &node.depends_on {
        let dep_tasks: Vec<&Task> = tasks.iter().filter(|t| t.node_id == *dep_id).collect();
        if dep_tasks.is_empty() {
            return false;
        }
        if !dep_tasks
            .iter()
            .all(|t| t.status == TaskStatus::Completed)
        {
            return false;
        }
    }
    true
}

/// Guard that ensures task completion notification is sent even on panic/timeout
struct TaskCleanupGuard {
    notify: Arc<Notify>,
    sent: bool,
}

impl TaskCleanupGuard {
    fn new(notify: Arc<Notify>) -> Self {
        Self {
            notify,
            sent: false,
        }
    }

    fn mark_sent(&mut self) {
        self.sent = true;
    }
}

impl Drop for TaskCleanupGuard {
    fn drop(&mut self) {
        if !self.sent {
            debug!("TaskCleanupGuard: Sending task completion notification on cleanup");
            self.notify.notify_one();
        }
    }
}

struct PreparedStepExecution {
    env: HashMap<String, String>,
    state_outputs_path: PathBuf,
    state_input_path: PathBuf,
}

const JS_AST_GREP_IDLE_TIMEOUT_MS_DEFAULT: u64 = 60_000;

type ProgressHeartbeatCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepPhase {
    Starting,
    FileQueued,
    FileLoaded,
    ExecutionStarted,
    Output,
    ExecutionFinished,
    ExecutionErrored,
}

impl std::fmt::Display for StepPhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            StepPhase::Starting => "starting",
            StepPhase::FileQueued => "file queued",
            StepPhase::FileLoaded => "file loaded",
            StepPhase::ExecutionStarted => "execution started",
            StepPhase::Output => "output",
            StepPhase::ExecutionFinished => "execution finished",
            StepPhase::ExecutionErrored => "execution errored",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug)]
struct UnitProgressState {
    last_progress_at: Instant,
    phase: StepPhase,
}

impl UnitProgressState {
    fn new(phase: StepPhase) -> Self {
        let now = Instant::now();
        Self {
            last_progress_at: now,
            phase,
        }
    }
}

#[derive(Debug)]
struct StepProgressState {
    global_last_progress_at: Instant,
    global_phase: StepPhase,
    active_units: HashMap<String, UnitProgressState>,
    output_active_units: HashSet<String>,
}

impl StepProgressState {
    fn new() -> Self {
        Self {
            global_last_progress_at: Instant::now(),
            global_phase: StepPhase::Starting,
            active_units: HashMap::new(),
            output_active_units: HashSet::new(),
        }
    }
}

fn js_ast_grep_idle_timeout() -> Duration {
    let override_ms = std::env::var("CODEMOD_JS_AST_GREP_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0);
    Duration::from_millis(override_ms.unwrap_or(JS_AST_GREP_IDLE_TIMEOUT_MS_DEFAULT))
}

fn record_unit_progress(
    state: &Arc<std::sync::Mutex<StepProgressState>>,
    unit_key: &str,
    phase: StepPhase,
) {
    if let Ok(mut state) = state.lock() {
        let now = Instant::now();
        state.global_last_progress_at = now;
        state.global_phase = phase;
        let entry = state
            .active_units
            .entry(unit_key.to_string())
            .or_insert_with(|| UnitProgressState::new(phase));
        entry.last_progress_at = now;
        entry.phase = phase;
        if phase == StepPhase::ExecutionStarted {
            state.output_active_units.insert(unit_key.to_string());
        }
    }
}

fn record_output_progress(state: &Arc<std::sync::Mutex<StepProgressState>>) {
    if let Ok(mut state) = state.lock() {
        let now = Instant::now();
        state.global_last_progress_at = now;
        state.global_phase = StepPhase::Output;

        let output_units: Vec<String> = state.output_active_units.iter().cloned().collect();
        for unit_key in output_units {
            if let Some(unit) = state.active_units.get_mut(&unit_key) {
                unit.last_progress_at = now;
                unit.phase = StepPhase::Output;
            }
        }
    }
}

fn finish_unit_progress(
    state: &Arc<std::sync::Mutex<StepProgressState>>,
    unit_key: &str,
    phase: StepPhase,
) {
    if let Ok(mut state) = state.lock() {
        state.global_last_progress_at = Instant::now();
        state.global_phase = phase;
        state.active_units.remove(unit_key);
        state.output_active_units.remove(unit_key);
    }
}

fn build_js_ast_grep_idle_timeout_message(
    state: &StepProgressState,
    idle_timeout: Duration,
) -> String {
    let active_unit_count = state.active_units.len();
    if let Some((unit_key, unit_state)) = state.active_units.iter().max_by(|left, right| {
        left.1
            .last_progress_at
            .elapsed()
            .cmp(&right.1.last_progress_at.elapsed())
    }) {
        format!(
            "No progress observed for {}s while processing {} ({}, active units: {})",
            idle_timeout.as_secs(),
            unit_key,
            unit_state.phase,
            active_unit_count
        )
    } else {
        format!(
            "No progress observed for {}s during js-ast-grep execution ({}, active units: 0)",
            idle_timeout.as_secs(),
            state.global_phase
        )
    }
}

fn format_runtime_event_log(event: &RuntimeEvent) -> Option<String> {
    let prefix = match event.kind {
        RuntimeEventKind::Progress => "[progress]",
        RuntimeEventKind::Warn => "[warn]",
        RuntimeEventKind::SetCurrentUnit => return None,
    };

    let mut message = format!("{prefix} {}", event.message);
    if let Some(meta) = &event.meta {
        message.push(' ');
        message.push_str(meta);
    }
    Some(message)
}

fn format_runtime_failure_message(failure: &RuntimeFailure) -> String {
    let prefix = match failure.kind {
        RuntimeFailureKind::File => "[error] file failed:",
        RuntimeFailureKind::Step => "[error] step failed:",
    };
    let mut message = format!("{prefix} {}", failure.message);
    if let Some(meta) = &failure.meta {
        message.push(' ');
        message.push_str(meta);
    }
    message
}

async fn await_js_ast_grep_execution_task(
    execution_task: tokio::task::JoinHandle<
        std::result::Result<CodemodOutput, codemod_sandbox::sandbox::errors::ExecutionError>,
    >,
    idle_timed_out: Arc<AtomicBool>,
    idle_failure_message: Arc<std::sync::Mutex<Option<String>>>,
    progress_state: Arc<std::sync::Mutex<StepProgressState>>,
    idle_timeout: Duration,
    relative_path: &str,
) -> Result<std::result::Result<CodemodOutput, codemod_sandbox::sandbox::errors::ExecutionError>> {
    let mut execution_task = std::pin::pin!(execution_task);
    loop {
        if idle_timed_out.load(Ordering::Acquire) {
            execution_task.as_mut().abort();
            let _ = execution_task.await;
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
                                "No progress observed for {}s while processing {}",
                                idle_timeout.as_secs(),
                                relative_path
                            )
                        })
                });
            return Err(Error::Runtime(message));
        }

        if execution_task.as_ref().is_finished() {
            return execution_task
                .await
                .map_err(|e| Error::StepExecution(format!("Codemod execution join failed: {e}")));
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn log_step_output(logger: &StructuredLogger, output: &str) {
    if !logger.is_jsonl() {
        return;
    }

    for line in output.lines().filter(|line| !line.is_empty()) {
        logger.log("info", line);
    }
}

fn format_shell_command_log_notice(request: &ShellCommandExecutionRequest) -> String {
    format!(
        "About to execute shell command for step '{}' in node '{}'.",
        request.step_name, request.node_name
    )
}

fn format_shell_command_notice(request: &ShellCommandExecutionRequest) -> String {
    let mut message = format!(
        "About to execute shell command for step '{}' in node '{}':",
        request.step_name, request.node_name
    );
    for line in request.command.lines() {
        message.push_str("\n  ");
        message.push_str(line);
    }
    message
}

/// Resolve an optional list of glob patterns, expanding `${{ }}` expressions.
/// Returns `None` when the input is `None` or all items resolve to empty strings.
fn resolve_optional_glob_list(
    items: &Option<Vec<String>>,
    params: &HashMap<String, serde_json::Value>,
    state: &HashMap<String, serde_json::Value>,
    matrix_values: Option<&HashMap<String, serde_json::Value>>,
    task_context: Option<&TaskExpressionContext>,
) -> Result<Option<Vec<String>>> {
    let Some(items) = items else {
        return Ok(None);
    };
    let resolved = resolve_string_list(items, params, state, matrix_values, None, task_context)?;
    Ok(if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    })
}

/// Workflow engine
pub struct Engine {
    /// State adapter for persisting workflow state
    state_adapter: Arc<Mutex<Box<dyn StateAdapter>>>,

    scheduler: Scheduler,

    workflow_run_config: WorkflowRunConfig,

    pub execution_stats: Arc<ExecutionStats>,

    /// Metrics context for tracking metrics across all JSSG steps
    pub metrics_context: MetricsContext,

    /// Async file writer for batched I/O operations
    file_writer: Arc<AsyncFileWriter>,

    /// Notification for when running tasks complete
    task_completion_notify: Arc<Notify>,

    /// Structured logger for JSONL output
    pub structured_logger: StructuredLogger,

    /// Optional per-task heartbeat callbacks invoked when captured output arrives.
    output_heartbeat_callbacks: Arc<std::sync::Mutex<HashMap<Uuid, ProgressHeartbeatCallback>>>,
}

/// Represents a codemod dependency chain for cycle detection
#[derive(Debug, Clone)]
pub struct CodemodDependency {
    /// Source identifier (registry package or local path)
    source: String,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CapabilitiesData {
    pub capabilities: Option<Vec<LlrtSupportedModules>>,
    pub capabilities_security_callback: Option<CapabilitiesSecurityCallback>,
}

impl Engine {
    async fn launch_agent(
        &self,
        canonical: &str,
        executable: &Path,
        system_prompt: Option<&str>,
        prompt: &str,
        logger: &StructuredLogger,
    ) -> Result<()> {
        let working_dir = &self.workflow_run_config.target_path;
        let full_prompt = if let Some(sys) = system_prompt {
            format!("{}\n\n{}", sys, prompt)
        } else {
            prompt.to_string()
        };
        let full_prompt_len = full_prompt.len();

        let mut cmd =
            build_agent_command(canonical, executable, prompt, system_prompt, working_dir)
                .ok_or_else(|| {
                    Error::StepExecution(format!(
                        "Failed to build command for agent '{}'",
                        canonical
                    ))
                })?;

        slog!(
            logger,
            info,
            "Launching agent '{}' at {}",
            canonical,
            executable.display()
        );
        slog!(
            logger,
            info,
            "Agent launch details: cwd={}, prompt_len={} chars",
            working_dir.display(),
            full_prompt_len
        );

        if canonical == "claude-code" || canonical == "codex" {
            slog!(logger, info, "{} prompt delivery: stdin pipe", canonical);
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        } else {
            cmd.stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        }

        let launch_args = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        slog!(
            logger,
            info,
            "Agent argv: program={}, args={:?}",
            cmd.get_program().to_string_lossy(),
            launch_args
        );

        let mut child = cmd.spawn().map_err(|error| {
            Error::StepExecution(format!("Failed to spawn agent '{}': {}", canonical, error))
        })?;
        if canonical == "claude-code" || canonical == "codex" {
            let mut stdin = child.stdin.take().ok_or_else(|| {
                Error::StepExecution(format!("{} stdin pipe was not available", canonical))
            })?;
            let stdin_payload = full_prompt.clone();
            let agent_name = canonical.to_string();
            tokio::task::spawn_blocking(move || -> Result<()> {
                stdin.write_all(stdin_payload.as_bytes()).map_err(|error| {
                    Error::StepExecution(format!(
                        "Failed to write {} prompt to stdin: {error}",
                        agent_name
                    ))
                })?;
                stdin.write_all(b"\n").map_err(|error| {
                    Error::StepExecution(format!(
                        "Failed to terminate {} prompt on stdin: {error}",
                        agent_name
                    ))
                })?;
                stdin.flush().map_err(|error| {
                    Error::StepExecution(format!("Failed to flush {} stdin: {error}", agent_name))
                })?;
                // Explicitly drop stdin to close the pipe and signal EOF to the child
                drop(stdin);
                Ok(())
            })
            .await
            .map_err(|error| {
                Error::StepExecution(format!(
                    "Failed to join {} stdin writer task: {}",
                    canonical, error
                ))
            })??;
        }
        let child_pid = child.id();
        slog!(
            logger,
            info,
            "Spawned agent '{}' with pid {:?}; waiting for it to exit",
            canonical,
            child_pid
        );

        let started_waiting = std::time::Instant::now();
        let wait_task = tokio::task::spawn_blocking(move || child.wait());
        let mut wait_task = std::pin::pin!(wait_task);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let status = loop {
            tokio::select! {
                result = &mut wait_task => {
                    let status = result.map_err(|error| {
                        Error::StepExecution(format!(
                            "Failed to join agent process for '{}': {}",
                            canonical, error
                        ))
                    })??;
                    break status;
                }
                _ = heartbeat.tick() => {
                    let waited = started_waiting.elapsed().as_secs();
                    slog!(
                        logger,
                        info,
                        "Still waiting on agent '{}' pid {:?} after {}s",
                        canonical,
                        child_pid,
                        waited
                    );
                }
            }
        };

        if status.success() {
            slog!(logger, info, "Agent '{}' completed successfully", canonical);
            Ok(())
        } else {
            let code = status.code().unwrap_or(-1);
            slog!(
                logger,
                warn,
                "Agent '{}' exited with code {}",
                canonical,
                code
            );
            Err(Error::StepExecution(format!(
                "Agent '{}' exited with code {}",
                canonical, code
            )))
        }
    }

    fn emit_ai_instructions(
        &self,
        logger: &StructuredLogger,
        system_prompt: Option<&str>,
        resolved_prompt: &str,
    ) {
        if logger.is_jsonl() {
            logger.log("info", "[AI INSTRUCTIONS]");
            if let Some(system_prompt) = system_prompt {
                logger.log("info", system_prompt);
            }
            logger.log("info", resolved_prompt);
            logger.log("info", "[/AI INSTRUCTIONS]");
        } else if !self.workflow_run_config.quiet {
            println!();
            println!("[AI INSTRUCTIONS]");
            println!();
            if let Some(system_prompt) = system_prompt {
                println!("{system_prompt}");
                println!();
            }
            println!("{resolved_prompt}");
            println!();
            println!("[/AI INSTRUCTIONS]");
            println!();
        }
    }

    /// Create a new engine with a local state adapter
    pub fn new() -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> =
            Arc::new(Mutex::new(Box::new(LocalStateAdapter::new())));

        Self {
            state_adapter: Arc::clone(&state_adapter),
            scheduler: Scheduler::new(),
            workflow_run_config: WorkflowRunConfig::default(),
            execution_stats: Arc::new(ExecutionStats::default()),
            metrics_context: MetricsContext::new(),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
            structured_logger: StructuredLogger::default(),
            output_heartbeat_callbacks: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Create a new engine with a local state adapter
    pub fn with_workflow_run_config(workflow_run_config: WorkflowRunConfig) -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> =
            Arc::new(Mutex::new(Box::new(LocalStateAdapter::new())));
        let structured_logger = StructuredLogger::new(workflow_run_config.output_format);

        Self {
            state_adapter: Arc::clone(&state_adapter),
            scheduler: Scheduler::new(),
            workflow_run_config,
            execution_stats: Arc::new(ExecutionStats::default()),
            metrics_context: MetricsContext::new(),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
            structured_logger,
            output_heartbeat_callbacks: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Get a mutable reference to the workflow run config
    pub fn workflow_run_config_mut(&mut self) -> &mut WorkflowRunConfig {
        &mut self.workflow_run_config
    }

    /// Create a new engine with a custom state adapter
    pub fn with_state_adapter(
        state_adapter: Box<dyn StateAdapter>,
        workflow_run_config: WorkflowRunConfig,
    ) -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> = Arc::new(Mutex::new(state_adapter));
        let structured_logger = StructuredLogger::new(workflow_run_config.output_format);

        Self {
            state_adapter: Arc::clone(&state_adapter),
            scheduler: Scheduler::new(),
            workflow_run_config,
            execution_stats: Arc::new(ExecutionStats::default()),
            metrics_context: MetricsContext::new(),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
            structured_logger,
            output_heartbeat_callbacks: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Enable or disable quiet mode (suppresses stdout/stderr when TUI is active)
    pub fn set_quiet(&mut self, quiet: bool) {
        self.workflow_run_config.quiet = quiet;
    }

    fn emit_error(&self, message: String) {
        if !self.workflow_run_config.quiet {
            error!("{message}");
        }
    }

    /// Replace the progress callback used by workflow execution.
    pub fn set_progress_callback(&mut self, progress_callback: Arc<Option<ProgressCallback>>) {
        self.workflow_run_config.progress_callback = progress_callback;
    }

    /// Set the human-readable name for this workflow run
    pub fn set_name(&mut self, name: Option<String>) {
        self.workflow_run_config.name = name;
    }

    /// Get the workflow file path
    pub fn get_workflow_file_path(&self) -> PathBuf {
        self.workflow_run_config.workflow_file_path.clone()
    }

    /// Get the target path for this workflow run
    pub fn get_target_path(&self) -> PathBuf {
        self.workflow_run_config.target_path.clone()
    }

    /// Check if the engine is in dry-run mode
    pub fn is_dry_run(&self) -> bool {
        self.workflow_run_config.dry_run
    }

    /// Enable or disable dry-run mode
    pub fn set_dry_run(&mut self, dry_run: bool) {
        self.workflow_run_config.dry_run = dry_run;
    }

    /// Get the current capabilities
    pub fn get_capabilities(&self) -> &Option<HashSet<LlrtSupportedModules>> {
        &self.workflow_run_config.capabilities
    }

    /// Set the capabilities
    pub fn set_capabilities(&mut self, capabilities: Option<HashSet<LlrtSupportedModules>>) {
        self.workflow_run_config.capabilities = capabilities;
    }

    /// Spawn a task asynchronously on a dedicated thread with its own runtime.
    ///
    /// Uses a dedicated multi-thread Tokio runtime per task thread so async
    /// work invoked from worker threads (for example network activity inside
    /// js-ast-grep codemods) can make progress reliably.
    async fn spawn_task_with_handle(&self, task_id: Uuid) -> Result<()> {
        let engine = self.clone();
        let task_completion_notify = Arc::clone(&self.task_completion_notify);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build task runtime");

            rt.block_on(async move {
                // Always ensure task completion notification is sent, even on panic or hang
                let mut cleanup_guard = TaskCleanupGuard::new(task_completion_notify.clone());

                // Add timeout to prevent infinite hanging
                let task_timeout = tokio::time::Duration::from_secs(45 * 60);

                match tokio::time::timeout(task_timeout, engine.execute_task(task_id)).await {
                    Ok(Ok(())) => {
                        debug!("Task {} completed successfully", task_id);
                        cleanup_guard.mark_sent();
                    }
                    Ok(Err(e)) => {
                        engine.emit_error(format!("Task {} execution failed: {}", task_id, e));
                    }
                    Err(_) => {
                        engine.emit_error(format!(
                            "Task {} timed out after {} seconds",
                            task_id,
                            task_timeout.as_secs()
                        ));
                        if let Err(e) = engine.mark_task_as_failed(task_id, "Task timed out").await
                        {
                            engine.emit_error(format!(
                                "Failed to mark task {} as failed: {}",
                                task_id, e
                            ));
                        }
                    }
                }
            });
        });

        Ok(())
    }

    /// Mark a task as failed due to timeout or other issues
    async fn mark_task_as_failed(&self, task_id: Uuid, error_message: &str) -> Result<()> {
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(TaskStatus::Failed)?),
            },
        );
        fields.insert(
            "ended_at".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(Utc::now())?),
            },
        );
        fields.insert(
            "error".to_string(),
            FieldDiff {
                operation: DiffOperation::Add,
                value: Some(serde_json::to_value(error_message.to_string())?),
            },
        );
        let task_diff = TaskDiff { task_id, fields };

        self.state_adapter
            .lock()
            .await
            .apply_task_diff(&task_diff)
            .await?;

        Ok(())
    }

    fn spawn_workflow_executor(&self, workflow_run_id: Uuid) {
        let mut workflow_engine = self.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build workflow runtime");
            rt.block_on(async move {
                if let Err(e) = workflow_engine.execute_workflow(workflow_run_id).await {
                    workflow_engine.emit_error(format!("Workflow execution failed: {e}"));
                }
            });
        });
    }

    async fn append_task_log(&self, task_id: Uuid, message: impl Into<String>) -> Result<()> {
        let mut adapter = self.state_adapter.lock().await;
        let mut task = adapter.get_task(task_id).await?;
        task.logs.push(message.into());
        adapter.save_task(&task).await?;
        Ok(())
    }

    async fn is_task_canceled(&self, workflow_run_id: Uuid, task_id: Uuid) -> Result<bool> {
        let adapter = self.state_adapter.lock().await;
        let workflow_run = adapter.get_workflow_run(workflow_run_id).await?;
        if workflow_run.status == WorkflowStatus::Canceled {
            return Ok(true);
        }

        let task = adapter.get_task(task_id).await?;
        Ok(task.status == TaskStatus::Failed && task.error.as_deref() == Some("Canceled by user"))
    }

    fn spawn_task_log_persistor(
        &self,
        task_id: Uuid,
    ) -> (
        tokio::sync::mpsc::UnboundedSender<String>,
        tokio::task::JoinHandle<()>,
    ) {
        let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let state_adapter = Arc::clone(&self.state_adapter);
        let log_persist_task = tokio::spawn(async move {
            while let Some(line) = log_rx.recv().await {
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                if line.is_empty() {
                    continue;
                }

                let mut adapter = state_adapter.lock().await;
                let Ok(mut current_task) = adapter.get_task(task_id).await else {
                    continue;
                };
                current_task.logs.push(line);
                let _ = adapter.save_task(&current_task).await;
            }
        });

        (log_tx, log_persist_task)
    }

    fn register_output_heartbeat(&self, task_id: Uuid, callback: ProgressHeartbeatCallback) {
        if let Ok(mut callbacks) = self.output_heartbeat_callbacks.lock() {
            callbacks.insert(task_id, callback);
        }
    }

    fn unregister_output_heartbeat(&self, task_id: Uuid) {
        if let Ok(mut callbacks) = self.output_heartbeat_callbacks.lock() {
            callbacks.remove(&task_id);
        }
    }

    async fn update_parent_matrix_master_for_task(&self, task: &Task) -> Result<()> {
        if let Some(master_task_id) = task.master_task_id {
            self.update_matrix_master_status(master_task_id).await?;
        }
        Ok(())
    }

    /// Wait for all currently running tasks to complete using pure notification-based approach
    async fn wait_for_running_tasks_to_complete(&self, workflow_run_id: Uuid) -> Result<()> {
        let mut consecutive_empty_checks = 0;
        const MAX_EMPTY_CHECKS: u8 = 3;

        loop {
            // Check the actual task status from the database (source of truth)
            let current_tasks = self
                .state_adapter
                .lock()
                .await
                .get_tasks(workflow_run_id)
                .await?;

            let running_tasks: Vec<_> = current_tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Running)
                .collect();

            if running_tasks.is_empty() {
                consecutive_empty_checks += 1;
                if consecutive_empty_checks >= MAX_EMPTY_CHECKS {
                    // Multiple checks confirm no running tasks
                    break;
                }
                // Brief pause to ensure task status updates are fully propagated
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                continue;
            }

            // Reset counter if we found running tasks
            consecutive_empty_checks = 0;

            debug!(
                "Waiting for {} running tasks to complete before matrix recompilation",
                running_tasks.len()
            );

            self.task_completion_notify.notified().await;
        }
        Ok(())
    }

    /// Create initial tasks for all nodes
    async fn create_initial_tasks(&self, workflow_run: &WorkflowRun) -> Result<()> {
        let mut tasks = self.scheduler.calculate_initial_tasks(workflow_run).await?;

        // Flatten matrix nodes: replace all master+children with a single
        // regular task per node so it runs exactly once.
        if self.workflow_run_config.flatten_matrix_tasks {
            let mut matrix_node_ids = std::collections::BTreeSet::new();
            // Collect all node IDs that have matrix tasks
            for task in &tasks {
                if task.is_master || task.master_task_id.is_some() {
                    matrix_node_ids.insert(task.node_id.clone());
                }
            }
            // Drop all matrix-related tasks
            tasks.retain(|task| !task.is_master && task.master_task_id.is_none());
            // Create a single regular task for each matrix node
            let wf_run_id = workflow_run.id;
            for node_id in matrix_node_ids {
                tasks.push(Task::new(wf_run_id, node_id, false));
            }
        }

        for task in tasks {
            self.state_adapter.lock().await.save_task(&task).await?;

            if task.is_master {
                self.update_matrix_master_status(task.id).await?;
            }
        }

        Ok(())
    }

    /// Run a workflow
    pub async fn run_workflow(
        &self,
        workflow: Workflow,
        params: HashMap<String, serde_json::Value>,
        bundle_path: Option<PathBuf>,
        capabilities: Option<&HashSet<LlrtSupportedModules>>,
    ) -> Result<Uuid> {
        validate_workflow(&workflow, bundle_path.as_deref().unwrap_or(Path::new("")))?;
        self.validate_codemod_dependencies(&workflow, &[]).await?;

        let workflow_run_id = Uuid::new_v4();
        let workflow_run = WorkflowRun {
            id: workflow_run_id,
            workflow: workflow.clone(),
            status: WorkflowStatus::Pending,
            params: params.clone(),
            bundle_path,
            tasks: Vec::new(),
            started_at: Utc::now(),
            ended_at: None,
            capabilities: capabilities.cloned(),
            name: self.workflow_run_config.name.clone(),
            target_path: Some(self.workflow_run_config.target_path.clone()),
        };

        self.state_adapter
            .lock()
            .await
            .save_workflow_run(&workflow_run)
            .await?;

        self.spawn_workflow_executor(workflow_run_id);

        Ok(workflow_run_id)
    }

    /// Resume a workflow run
    pub async fn resume_workflow(&self, workflow_run_id: Uuid, task_ids: Vec<Uuid>) -> Result<()> {
        // Just make sure the workflow run exists
        let _workflow_run = self
            .state_adapter
            .lock()
            .await
            .get_workflow_run(workflow_run_id)
            .await?;

        let mut triggered = false;
        for task_id in task_ids {
            let task = self.state_adapter.lock().await.get_task(task_id).await?;

            // If the task is awaiting trigger we can trigger it
            // OR if it is in a terminal state, we can trigger it again
            if task.status == TaskStatus::AwaitingTrigger
                || task.status == TaskStatus::Completed
                || task.status == TaskStatus::Failed
            {
                let mut fields = HashMap::new();
                fields.insert(
                    "status".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::to_value(TaskStatus::Pending)?),
                    },
                );
                fields.insert(
                    "started_at".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::Value::Null),
                    },
                );
                fields.insert(
                    "ended_at".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::Value::Null),
                    },
                );
                fields.insert(
                    "error".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::Value::Null),
                    },
                );
                let task_diff = TaskDiff { task_id, fields };

                self.state_adapter
                    .lock()
                    .await
                    .apply_task_diff(&task_diff)
                    .await?;

                if let Err(e) = self.spawn_task_with_handle(task_id).await {
                    self.emit_error(format!("Failed to spawn task {}: {}", task_id, e));
                }

                self.update_parent_matrix_master_for_task(&task).await?;

                triggered = true;
                info!("Triggered task {} ({})", task_id, task.node_id);
            } else {
                warn!("Task {task_id} is not awaiting trigger");
            }
        }

        if !triggered {
            return Err(Error::Other("No tasks were triggered".to_string()));
        }

        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(WorkflowStatus::Running)?),
            },
        );
        let workflow_run_diff = WorkflowRunDiff {
            workflow_run_id,
            fields,
        };

        self.state_adapter
            .lock()
            .await
            .apply_workflow_run_diff(&workflow_run_diff)
            .await?;

        self.spawn_workflow_executor(workflow_run_id);

        Ok(())
    }

    /// Trigger all awaiting tasks in a workflow run
    pub async fn trigger_all(&self, workflow_run_id: Uuid) -> Result<bool> {
        // TODO: Do we need this?
        let _workflow_run = self
            .state_adapter
            .lock()
            .await
            .get_workflow_run(workflow_run_id)
            .await?;

        let tasks = self
            .state_adapter
            .lock()
            .await
            .get_tasks(workflow_run_id)
            .await?;

        let awaiting_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::AwaitingTrigger)
            .collect();

        if awaiting_tasks.is_empty() {
            // Check if all tasks are complete
            let active_tasks = tasks.iter().any(|t| {
                matches!(
                    t.status,
                    TaskStatus::Pending | TaskStatus::Running | TaskStatus::AwaitingTrigger
                )
            });

            // If no tasks are active, mark the workflow as completed
            if !active_tasks {
                let mut fields = HashMap::new();
                fields.insert(
                    "status".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::to_value(WorkflowStatus::Completed)?),
                    },
                );
                let workflow_run_diff = WorkflowRunDiff {
                    workflow_run_id,
                    fields,
                };

                self.state_adapter
                    .lock()
                    .await
                    .apply_workflow_run_diff(&workflow_run_diff)
                    .await?;

                info!("Workflow run {workflow_run_id} is now complete");
                return Ok(true);
            }

            // If we reached here, it means the workflow is still running but no tasks need triggers
            info!("No tasks in workflow run {workflow_run_id} are awaiting triggers");
            return Ok(false);
        }

        let mut triggered = false;
        for task in awaiting_tasks {
            let mut fields = HashMap::new();
            fields.insert(
                "status".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(TaskStatus::Pending)?),
                },
            );
            fields.insert(
                "started_at".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::Value::Null),
                },
            );
            fields.insert(
                "ended_at".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::Value::Null),
                },
            );
            fields.insert(
                "error".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::Value::Null),
                },
            );
            let task_diff = TaskDiff {
                task_id: task.id,
                fields,
            };

            self.state_adapter
                .lock()
                .await
                .apply_task_diff(&task_diff)
                .await?;

            let task_id = task.id;
            if let Err(e) = self.spawn_task_with_handle(task_id).await {
                self.emit_error(format!("Failed to spawn task {}: {}", task_id, e));
            }

            self.update_parent_matrix_master_for_task(task).await?;

            triggered = true;
            info!("Triggered task {} ({})", task.id, task.node_id);
        }

        // If no tasks were triggered, it means they're all done or in progress
        // We don't need to error out, just return successfully
        if !triggered {
            return Ok(false);
        }

        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(WorkflowStatus::Running)?),
            },
        );
        let workflow_run_diff = WorkflowRunDiff {
            workflow_run_id,
            fields,
        };

        self.state_adapter
            .lock()
            .await
            .apply_workflow_run_diff(&workflow_run_diff)
            .await?;

        self.spawn_workflow_executor(workflow_run_id);
        Ok(true)
    }

    /// Cancel a workflow run
    pub async fn cancel_workflow(&self, workflow_run_id: Uuid) -> Result<()> {
        // Get the workflow run
        let workflow_run = self
            .state_adapter
            .lock()
            .await
            .get_workflow_run(workflow_run_id)
            .await?;

        // Check if the workflow is running or awaiting triggers
        if workflow_run.status != WorkflowStatus::Running
            && workflow_run.status != WorkflowStatus::AwaitingTrigger
        {
            return Err(Error::Other(format!(
                "Workflow run {workflow_run_id} is not running or awaiting triggers"
            )));
        }

        // Get all tasks
        let tasks = self
            .state_adapter
            .lock()
            .await
            .get_tasks(workflow_run_id)
            .await?;

        // Cancel all running tasks
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Running) {
            // Create a task diff to update the status
            let mut fields = HashMap::new();
            fields.insert(
                "status".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(TaskStatus::Failed)?),
                },
            );
            fields.insert(
                "error".to_string(),
                FieldDiff {
                    operation: DiffOperation::Add,
                    value: Some(serde_json::to_value("Canceled by user")?),
                },
            );
            fields.insert(
                "ended_at".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(Utc::now())?),
                },
            );
            let task_diff = TaskDiff {
                task_id: task.id,
                fields,
            };

            // Apply the diff
            self.state_adapter
                .lock()
                .await
                .apply_task_diff(&task_diff)
                .await?;

            info!("Canceled task {} ({})", task.id, task.node_id);
            self.update_parent_matrix_master_for_task(task).await?;
        }

        // Create a workflow run diff to update the status
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(WorkflowStatus::Canceled)?),
            },
        );
        fields.insert(
            "ended_at".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(Utc::now())?),
            },
        );
        let workflow_run_diff = WorkflowRunDiff {
            workflow_run_id,
            fields,
        };

        // Apply the diff
        self.state_adapter
            .lock()
            .await
            .apply_workflow_run_diff(&workflow_run_diff)
            .await?;

        self.task_completion_notify.notify_waiters();

        Ok(())
    }

    /// Get workflow run status
    pub async fn get_workflow_status(&self, workflow_run_id: Uuid) -> Result<WorkflowStatus> {
        let workflow_run = self
            .state_adapter
            .lock()
            .await
            .get_workflow_run(workflow_run_id)
            .await?;
        Ok(workflow_run.status)
    }

    /// Get workflow run
    pub async fn get_workflow_run(&self, workflow_run_id: Uuid) -> Result<WorkflowRun> {
        self.state_adapter
            .lock()
            .await
            .get_workflow_run(workflow_run_id)
            .await
    }

    /// Get tasks for a workflow run
    pub async fn get_tasks(&self, workflow_run_id: Uuid) -> Result<Vec<Task>> {
        self.state_adapter
            .lock()
            .await
            .get_tasks(workflow_run_id)
            .await
    }

    /// List workflow runs
    pub async fn list_workflow_runs(&self, limit: usize) -> Result<Vec<WorkflowRun>> {
        self.state_adapter
            .lock()
            .await
            .list_workflow_runs(limit)
            .await
    }

    /// Persisted workflow state (e.g. `shards` written by shard steps). Stored on disk by the
    /// local adapter under `<data_dir>/butterflow/state/<workflow_run_id>.json`.
    pub async fn get_workflow_state(
        &self,
        workflow_run_id: Uuid,
    ) -> Result<HashMap<String, serde_json::Value>> {
        self.state_adapter
            .lock()
            .await
            .get_state(workflow_run_id)
            .await
    }

    /// Validate codemod dependencies to prevent infinite recursion cycles
    ///
    /// This method recursively checks all codemod dependencies in a workflow to ensure
    /// there are no circular references that would cause infinite loops during execution.
    ///
    /// Examples of cycles that will be detected:
    /// - Direct cycle: A → A
    /// - Two-step cycle: A → B → A
    /// - Multi-step cycle: A → B → C → A
    ///
    /// # Arguments
    /// * `workflow` - The workflow to validate
    /// * `dependency_chain` - Current chain of codemod dependencies being tracked
    ///
    /// # Returns
    /// * `Ok(())` if no cycles are detected
    /// * `Err(Error::Other)` if a cycle is found, with detailed information about the cycle
    async fn validate_codemod_dependencies(
        &self,
        workflow: &Workflow,
        dependency_chain: &[CodemodDependency],
    ) -> Result<()> {
        for node in &workflow.nodes {
            for step in &node.steps {
                if let StepAction::Codemod(codemod) = &step.action {
                    // Check if this codemod is already in the dependency chain
                    if let Some(cycle_start) =
                        self.find_cycle_in_chain(&codemod.source, dependency_chain)
                    {
                        let chain_str = dependency_chain
                            .iter()
                            .map(|d| d.source.as_str())
                            .collect::<Vec<_>>()
                            .join(" → ");

                        return Err(Error::Other(format!(
                            "Codemod dependency cycle detected!\n\
                            Cycle: {} → {} → {}\n\
                            This would cause infinite recursion during execution.\n\
                            Please review your codemod dependencies to remove the circular reference.",
                            cycle_start,
                            if chain_str.is_empty() { "(root)" } else { &chain_str },
                            codemod.source
                        )));
                    }

                    // Resolve the codemod package to validate its workflow
                    match self
                        .resolve_and_validate_codemod(&codemod.source, dependency_chain)
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            warn!(
                                "Failed to validate codemod dependency {}: {}",
                                codemod.source, e
                            );
                            // We'll continue validation but log the warning
                            // The actual execution will handle the error appropriately
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Find if a codemod source creates a cycle in the dependency chain
    pub fn find_cycle_in_chain(
        &self,
        source: &str,
        dependency_chain: &[CodemodDependency],
    ) -> Option<String> {
        for dep in dependency_chain {
            if dep.source == source {
                return Some(dep.source.clone());
            }
        }
        None
    }

    /// Resolve a codemod and recursively validate its dependencies
    async fn resolve_and_validate_codemod(
        &self,
        source: &str,
        dependency_chain: &[CodemodDependency],
    ) -> Result<()> {
        // Resolve the package
        let resolved_package = self
            .workflow_run_config
            .registry_client
            .resolve_package(source, None, false, None)
            .await
            .map_err(|e| Error::Other(format!("Failed to resolve codemod {source}: {e}")))?;

        // Load the codemod's workflow
        let workflow_path = resolved_package.package_dir.join("workflow.yaml");
        if !workflow_path.exists() {
            return Err(Error::Other(format!(
                "Workflow file not found in codemod package: {}",
                workflow_path.display()
            )));
        }

        let workflow_content = std::fs::read_to_string(&workflow_path)
            .map_err(|e| Error::Other(format!("Failed to read workflow file: {e}")))?;

        let codemod_workflow: Workflow = serde_yaml::from_str(&workflow_content)
            .map_err(|e| Error::Other(format!("Failed to parse workflow YAML: {e}")))?;

        // Create new dependency chain including this codemod
        let mut new_chain = dependency_chain.to_vec();
        new_chain.push(CodemodDependency {
            source: source.to_string(),
        });

        // Recursively validate the codemod's workflow dependencies
        Box::pin(self.validate_codemod_dependencies(&codemod_workflow, &new_chain)).await?;

        Ok(())
    }

    /// Execute a workflow
    async fn execute_workflow(&mut self, workflow_run_id: Uuid) -> Result<()> {
        // Get the workflow run
        let workflow_run = self
            .state_adapter
            .lock()
            .await
            .get_workflow_run(workflow_run_id)
            .await?;

        // Use the stored target path from the workflow run so that the engine
        // operates on the correct directory, even when launched from a
        // different cwd (e.g. via `workflow tui`).
        if let Some(target_path) = &workflow_run.target_path {
            self.workflow_run_config.target_path = target_path.clone();
        }

        // Create a workflow run diff to update the status
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(WorkflowStatus::Running)?),
            },
        );
        let workflow_run_diff = WorkflowRunDiff {
            workflow_run_id,
            fields,
        };

        // Apply the diff
        self.state_adapter
            .lock()
            .await
            .apply_workflow_run_diff(&workflow_run_diff)
            .await?;

        info!("Starting workflow run {workflow_run_id}");

        // Create tasks for all nodes if they don't exist yet
        let existing_tasks = self
            .state_adapter
            .lock()
            .await
            .get_tasks(workflow_run_id)
            .await?;
        if existing_tasks.is_empty() {
            self.create_initial_tasks(&workflow_run).await?;
        }

        // Track the last state hash to detect state changes
        let mut last_state_hash: Option<u64> = None;

        // Main execution loop
        loop {
            // Get the current workflow run state
            let current_workflow_run = self
                .state_adapter
                .lock()
                .await
                .get_workflow_run(workflow_run_id)
                .await?;

            // Get all tasks
            let current_tasks = self
                .state_adapter
                .lock()
                .await
                .get_tasks(workflow_run_id)
                .await?;

            // Wait for any running tasks to complete before proceeding
            self.wait_for_running_tasks_to_complete(workflow_run_id)
                .await?;

            // --- Recompile matrix tasks based on current state (only if workflow has matrix strategies) ---
            // This ensures the task list reflects the latest state before scheduling
            let has_matrix_strategies = current_workflow_run.workflow.nodes.iter().any(|n| {
                matches!(
                    n.strategy,
                    Some(Strategy {
                        r#type: butterflow_models::strategy::StrategyType::Matrix,
                        ..
                    })
                )
            });

            // Check if state has changed for matrix recompilation
            // Skip recompilation when matrix tasks are flattened (no dynamic expansion)
            let should_recompile = if has_matrix_strategies
                && !self.workflow_run_config.flatten_matrix_tasks
            {
                let current_state = self
                    .state_adapter
                    .lock()
                    .await
                    .get_state(workflow_run_id)
                    .await?;

                // Calculate hash of current state
                let mut hasher = DefaultHasher::new();
                for (key, value) in &current_state {
                    key.hash(&mut hasher);
                    // Hash the JSON string representation of the value
                    value.to_string().hash(&mut hasher);
                }
                let current_hash = hasher.finish();

                // Check if state has changed
                let state_changed = match last_state_hash {
                    Some(last_hash) => last_hash != current_hash,
                    None => true, // First time, always recompile
                };

                if state_changed {
                    last_state_hash = Some(current_hash);
                    debug!("State changed, triggering matrix recompilation for workflow {workflow_run_id}");
                }

                state_changed
            } else {
                false
            };

            if should_recompile {
                debug!("Starting matrix task recompilation for workflow {workflow_run_id}");
                if let Err(e) = self
                    .recompile_matrix_tasks(workflow_run_id, &current_workflow_run, &current_tasks)
                    .await
                {
                    self.emit_error(format!(
                        "Failed during matrix task recompilation for run {workflow_run_id}: {e}"
                    ));
                    // Decide how to handle recompilation errors, e.g., fail the workflow?
                    // For now, we log and continue, but this might need refinement.
                }
                debug!("Completed matrix task recompilation for workflow {workflow_run_id}");
            }

            // Get potentially updated tasks after recompilation (only if we ran recompilation)
            let tasks_after_recompilation = if should_recompile {
                self.state_adapter
                    .lock()
                    .await
                    .get_tasks(workflow_run_id)
                    .await?
            } else {
                current_tasks
            };
            // --- End of Recompilation ---

            // Check if all tasks are completed or failed
            let all_done = tasks_after_recompilation.iter().all(|t| {
                t.status == TaskStatus::Completed
                    || t.status == TaskStatus::Failed
                    || t.status == TaskStatus::WontDo
            });

            if all_done {
                // Check if any tasks failed
                let any_failed = tasks_after_recompilation
                    .iter()
                    .any(|t| t.status == TaskStatus::Failed);

                // Create a workflow run diff to update the status
                let mut fields = HashMap::new();
                fields.insert(
                    "status".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::to_value(if any_failed {
                            WorkflowStatus::Failed
                        } else {
                            WorkflowStatus::Completed
                        })?),
                    },
                );
                fields.insert(
                    "ended_at".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::to_value(Utc::now())?),
                    },
                );
                let workflow_run_diff = WorkflowRunDiff {
                    workflow_run_id,
                    fields,
                };

                // Apply the diff
                self.state_adapter
                    .lock()
                    .await
                    .apply_workflow_run_diff(&workflow_run_diff)
                    .await?;

                info!(
                    "Workflow run {} {}",
                    workflow_run_id,
                    if any_failed { "failed" } else { "completed" }
                );

                break;
            }

            // Find runnable tasks based on the potentially updated task list
            let runnable_tasks_result = self
                .scheduler
                .find_runnable_tasks(&current_workflow_run, &tasks_after_recompilation)
                .await?;

            let tasks_to_await_trigger = runnable_tasks_result.tasks_to_await_trigger;
            let mut runnable_tasks = runnable_tasks_result.runnable_tasks;

            if self.workflow_run_config.auto_trigger_manual_steps {
                // In auto-trigger mode (e.g. pro codemod dry-run), treat manual
                // steps as immediately runnable instead of blocking — but only
                // if their dependencies are satisfied (the scheduler adds manual
                // tasks to tasks_to_await_trigger before checking deps).
                let current_workflow = &current_workflow_run.workflow;
                for task_id in &tasks_to_await_trigger {
                    let task = tasks_after_recompilation.iter().find(|t| t.id == *task_id);
                    let node = task
                        .and_then(|t| current_workflow.nodes.iter().find(|n| n.id == t.node_id));
                    let deps_satisfied = node.is_some_and(|n| {
                        n.depends_on.iter().all(|dep_id| {
                            tasks_after_recompilation
                                .iter()
                                .filter(|t| t.node_id == *dep_id)
                                .all(|t| t.status == TaskStatus::Completed)
                        })
                    });
                    if deps_satisfied {
                        runnable_tasks.push(*task_id);
                    }
                }
            } else {
                let mut parent_master_ids = HashSet::new();
                for task_id in tasks_to_await_trigger {
                    if let Some(task) = tasks_after_recompilation
                        .iter()
                        .find(|task| task.id == task_id)
                    {
                        if let Some(master_task_id) = task.master_task_id {
                            parent_master_ids.insert(master_task_id);
                        }
                    }

                    // Create a task diff to update the status
                    let mut fields = HashMap::new();
                    fields.insert(
                        "status".to_string(),
                        FieldDiff {
                            operation: DiffOperation::Update,
                            value: Some(serde_json::to_value(TaskStatus::AwaitingTrigger)?),
                        },
                    );
                    let task_diff = TaskDiff { task_id, fields };

                    // Apply the diff
                    self.state_adapter
                        .lock()
                        .await
                        .apply_task_diff(&task_diff)
                        .await?;
                }

                for master_task_id in parent_master_ids {
                    self.update_matrix_master_status(master_task_id).await?;
                }
            }

            let tasks_after_status_updates = self
                .state_adapter
                .lock()
                .await
                .get_tasks(workflow_run_id)
                .await?;

            // Check if any tasks are awaiting trigger
            let awaiting_trigger = tasks_after_status_updates
                .iter()
                .any(|t| t.status == TaskStatus::AwaitingTrigger);
            let any_running = tasks_after_status_updates
                .iter()
                .any(|t| t.status == TaskStatus::Running);

            // If there are tasks awaiting trigger and no runnable tasks and no running tasks,
            // then we need to pause the workflow and wait for manual triggers
            if awaiting_trigger && runnable_tasks.is_empty() && !any_running {
                // Create a workflow run diff to update the status
                let mut fields = HashMap::new();
                fields.insert(
                    "status".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::to_value(WorkflowStatus::AwaitingTrigger)?),
                    },
                );
                let workflow_run_diff = WorkflowRunDiff {
                    workflow_run_id,
                    fields,
                };

                // Apply the diff
                self.state_adapter
                    .lock()
                    .await
                    .apply_workflow_run_diff(&workflow_run_diff)
                    .await?;

                info!("Workflow run {workflow_run_id} is awaiting triggers");

                // Exit the execution loop, will be resumed when triggers are received
                break;
            }

            let runnable_tasks_is_empty = runnable_tasks.is_empty();

            // Execute runnable tasks synchronously to avoid race conditions with matrix recompilation
            for task_id in runnable_tasks {
                let task = tasks_after_status_updates
                    .iter()
                    .find(|t| t.id == task_id)
                    .unwrap(); // Should exist as runnable_tasks is derived from this list
                let _node = current_workflow_run // Use the fetched run state
                    .workflow
                    .nodes
                    .iter()
                    .find(|n| n.id == task.node_id)
                    .unwrap(); // Should exist based on how tasks are created

                // Execute task synchronously to ensure state updates are applied before matrix recompilation
                if let Err(e) = self.execute_task(task_id).await {
                    self.emit_error(format!("Task execution failed: {e}"));
                }
            }

            // Only wait if no tasks were executed (to avoid busy waiting)
            if runnable_tasks_is_empty {
                time::sleep(Duration::from_secs(1)).await;
            }
        }

        Ok(())
    }

    /// Recompile matrix tasks based on the current state.
    /// Creates new tasks for added matrix items and marks tasks for removed items as WontDo.
    async fn recompile_matrix_tasks(
        &self,
        workflow_run_id: Uuid,
        workflow_run: &WorkflowRun,
        tasks: &[Task],
    ) -> Result<()> {
        debug!("Starting matrix task recompilation for run {workflow_run_id}");

        let state = self
            .state_adapter
            .lock()
            .await
            .get_state(workflow_run_id)
            .await?;

        // Use scheduler to calculate matrix task changes
        let changes = self
            .scheduler
            .calculate_matrix_task_changes(workflow_run_id, workflow_run, tasks, &state)
            .await?;

        // Create new tasks
        for task in changes.new_tasks {
            debug!("Creating new matrix task for node '{}'", task.node_id);
            self.state_adapter.lock().await.save_task(&task).await?;
        }

        // Mark tasks as WontDo
        for task_id in changes.tasks_to_mark_wont_do {
            debug!("Marking task {task_id} as WontDo");
            let mut fields = HashMap::new();
            fields.insert(
                "status".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(TaskStatus::WontDo)?),
                },
            );
            let task_diff = TaskDiff { task_id, fields };
            self.state_adapter
                .lock()
                .await
                .apply_task_diff(&task_diff)
                .await?;
        }

        for task_id in changes.tasks_to_reset_to_pending {
            debug!("Resetting task {task_id} from Failed to Pending");
            let mut fields = HashMap::new();
            fields.insert(
                "status".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(TaskStatus::Pending)?),
                },
            );
            fields.insert(
                "error".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::Value::Null),
                },
            );
            fields.insert(
                "ended_at".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::Value::Null),
                },
            );
            let task_diff = TaskDiff { task_id, fields };
            self.state_adapter
                .lock()
                .await
                .apply_task_diff(&task_diff)
                .await?;
        }

        // Update master task status
        for master_task_id in changes.master_tasks_to_update {
            debug!("Updating master task {master_task_id} status");
            self.update_matrix_master_status(master_task_id).await?;
        }

        debug!("Finished matrix task recompilation for run {workflow_run_id}");
        Ok(())
    }

    /// Execute a task
    async fn execute_task(&self, task_id: Uuid) -> Result<()> {
        let task = self.state_adapter.lock().await.get_task(task_id).await?;

        let workflow_run = self
            .state_adapter
            .lock()
            .await
            .get_workflow_run(task.workflow_run_id)
            .await?;

        let resolved_params = workflow_run
            .workflow
            .params
            .as_ref()
            .map(|p| resolve_values_with_default(&p.schema, &workflow_run.params))
            .unwrap_or_else(|| workflow_run.params);

        let node = workflow_run
            .workflow
            .nodes
            .iter()
            .find(|n| n.id == task.node_id)
            .ok_or_else(|| Error::NodeNotFound(task.node_id.clone()))?;

        // Create a task diff to update the status
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(TaskStatus::Running)?),
            },
        );
        fields.insert(
            "started_at".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(Utc::now())?),
            },
        );
        let task_diff = TaskDiff { task_id, fields };

        // Apply the diff
        self.state_adapter
            .lock()
            .await
            .apply_task_diff(&task_diff)
            .await?;

        self.update_parent_matrix_master_for_task(&task).await?;

        info!("Executing task {} ({})", task_id, node.id);

        // Cloud mode: checkout a task-specific branch before running steps
        let cloud_mode = crate::git_ops::is_cloud_mode();
        // Always build task expression context so CODEMOD_TASK_* env vars are
        // available as `task.*` template variables regardless of mode.
        let task_expr_ctx = Some(crate::git_ops::build_task_expression_context(
            &task.id.to_string(),
        ));
        let cloud_branch_name = if cloud_mode {
            let ctx = task_expr_ctx.as_ref().unwrap();
            let configured_branch = node.branch_name.as_ref().map(|tmpl| {
                resolve_string_with_expression(
                    tmpl,
                    &resolved_params,
                    &HashMap::new(),
                    task.matrix_values.as_ref(),
                    None,
                    Some(ctx),
                )
                .unwrap_or_else(|_| format!("codemod-{}", ctx.signature))
            });
            let branch =
                crate::git_ops::resolve_branch_name(configured_branch.as_deref(), &ctx.signature);
            crate::git_ops::checkout_branch(&branch, &self.workflow_run_config.target_path).await?;
            Some(branch)
        } else {
            None
        };

        let runtime_type = node
            .runtime
            .as_ref()
            .map(|r| r.r#type)
            .unwrap_or(RuntimeType::Direct);

        // Track whether any commit checkpoint was created during the step loop
        let mut had_commit_checkpoint = false;

        // Execute each step in the node
        for (step_index, step) in node.steps.iter().enumerate() {
            if self.is_task_canceled(workflow_run.id, task_id).await? {
                return Err(Error::Runtime("Canceled by user".to_string()));
            }

            let state = self
                .state_adapter
                .lock()
                .await
                .get_state(workflow_run.id)
                .await?;

            if let Some(condition) = &step.condition {
                // TODO: Load step outputs from STEP_OUTPUTS file and pass here
                let should_execute = evaluate_condition(
                    condition,
                    &resolved_params,
                    &state,
                    task.matrix_values.as_ref(),
                    None, // step outputs
                    task_expr_ctx.as_ref(),
                )
                .unwrap_or_default();

                if !should_execute {
                    slog!(
                        self.structured_logger,
                        info,
                        "Skipping step '{}' - condition not met: {}",
                        step.name,
                        condition
                    );
                    continue;
                }
            }

            let step_context = StepContext {
                step_name: step.name.clone(),
                step_index,
                node_id: node.id.clone(),
                node_name: node.name.clone(),
                task_id: task_id.to_string(),
                step_id: None,
            };
            let step_logger = self.structured_logger.with_context(step_context);

            let runner: Box<dyn Runner> = match runtime_type {
                RuntimeType::Direct => {
                    Box::new(DirectRunner::with_quiet(self.workflow_run_config.quiet))
                }
                RuntimeType::Docker => {
                    #[cfg(feature = "docker")]
                    {
                        Box::new(DockerRunner::new())
                    }
                    #[cfg(not(feature = "docker"))]
                    {
                        return Err(Error::UnsupportedRuntime(RuntimeType::Docker));
                    }
                }
                RuntimeType::Podman => {
                    #[cfg(feature = "podman")]
                    {
                        Box::new(PodmanRunner::new())
                    }
                    #[cfg(not(feature = "podman"))]
                    {
                        return Err(Error::UnsupportedRuntime(RuntimeType::Podman));
                    }
                }
            };

            let _ = self
                .append_task_log(task_id, format!("Step started: {}", step.name))
                .await;
            step_logger.step_start();
            if !step_logger.is_jsonl() && !self.workflow_run_config.quiet {
                println!("\x1b[1;36m⏺ {}\x1b[0m", step.name);
            }
            let step_start_time = std::time::Instant::now();

            let quiet_capture =
                self.workflow_run_config.quiet && !self.structured_logger.is_jsonl();
            let (quiet_log_tx, quiet_log_persist_task) = if quiet_capture {
                let (log_tx, log_persist_task) = self.spawn_task_log_persistor(task_id);
                (Some(log_tx), Some(log_persist_task))
            } else {
                (None, None)
            };

            // In JSONL mode, capture ALL stdout (fd 1) during step execution.
            // Any println!, console.log, etc. from child processes, AI agents,
            // or JS codemods will be intercepted and wrapped in JSONL with the
            // correct step context. The structured logger bypasses the capture
            // by writing directly to the saved real stdout fd.
            let _stdout_capture = if self.structured_logger.is_jsonl() {
                StdoutCaptureGuard::start(Some(&step_logger), None)
            } else if self.workflow_run_config.quiet {
                let output_heartbeat_callbacks = Arc::clone(&self.output_heartbeat_callbacks);
                let line_callback = quiet_log_tx.as_ref().map(|log_tx| {
                    let log_tx = log_tx.clone();
                    Arc::new(move |line: String| {
                        let heartbeat = output_heartbeat_callbacks
                            .lock()
                            .ok()
                            .and_then(|callbacks| callbacks.get(&task_id).cloned());
                        if let Some(heartbeat) = heartbeat {
                            heartbeat();
                        }
                        let _ = log_tx.send(line);
                    }) as crate::structured_log::CapturedLineCallback
                });
                StdoutCaptureGuard::start(None, line_callback)
            } else {
                None
            };

            let result = self
                .execute_step_action(
                    runner.as_ref(),
                    &step.action,
                    &step.name,
                    &step.env,
                    &step.id,
                    node,
                    &task,
                    &resolved_params,
                    &state,
                    &workflow_run.workflow,
                    &workflow_run.bundle_path,
                    &[],
                    &self.workflow_run_config.capabilities,
                    task_expr_ctx.as_ref(),
                    &step_logger,
                )
                .await;

            // Drop the capture guard to restore stdout before emitting step_end.
            // This ensures all captured output is flushed and attributed to this step.
            drop(_stdout_capture);
            drop(quiet_log_tx);
            if let Some(log_persist_task) = quiet_log_persist_task {
                let _ = log_persist_task.await;
            }

            if self.is_task_canceled(workflow_run.id, task_id).await? {
                return Err(Error::Runtime("Canceled by user".to_string()));
            }

            match result {
                Ok(_) => {
                    step_logger.step_end("success", step_start_time.elapsed().as_millis() as u64);
                    slog!(
                        step_logger,
                        info,
                        "Step '{}' finished successfully for task {}",
                        step.name,
                        task_id
                    );

                    // Cloud mode: execute commit checkpoint if configured on this step
                    if cloud_mode {
                        if let Some(commit_config) = &step.commit {
                            let resolved_message = resolve_string_with_expression(
                                &commit_config.message,
                                &resolved_params,
                                &state,
                                task.matrix_values.as_ref(),
                                None,
                                task_expr_ctx.as_ref(),
                            )
                            .unwrap_or_else(|_| commit_config.message.clone());

                            let paths = commit_config.add.clone().unwrap_or_default();
                            match crate::git_ops::commit(
                                &resolved_message,
                                &paths,
                                commit_config.allow_empty,
                                &self.workflow_run_config.target_path,
                            )
                            .await
                            {
                                Ok(true) => {
                                    had_commit_checkpoint = true;
                                    slog!(
                                        step_logger,
                                        info,
                                        "Commit checkpoint created: {}",
                                        resolved_message
                                    );
                                }
                                Ok(false) => {
                                    slog!(
                                        step_logger,
                                        info,
                                        "Commit checkpoint skipped (no changes): {}",
                                        resolved_message
                                    );
                                }
                                Err(e) => {
                                    self.emit_error(format!(
                                        "Commit checkpoint failed for step '{}': {}",
                                        step.name, e
                                    ));
                                    return Err(e);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    step_logger.step_end("failure", step_start_time.elapsed().as_millis() as u64);

                    // Create a task diff to update the status
                    let mut fields = HashMap::new();
                    fields.insert(
                        "status".to_string(),
                        FieldDiff {
                            operation: DiffOperation::Update,
                            value: Some(serde_json::to_value(TaskStatus::Failed)?),
                        },
                    );
                    fields.insert(
                        "ended_at".to_string(),
                        FieldDiff {
                            operation: DiffOperation::Update,
                            value: Some(serde_json::to_value(Utc::now())?),
                        },
                    );
                    fields.insert(
                        "error".to_string(),
                        FieldDiff {
                            operation: DiffOperation::Add,
                            value: Some(serde_json::to_value(format!(
                                "Step {} failed: {}",
                                step.name, e
                            ))?),
                        },
                    );
                    let task_diff = TaskDiff { task_id, fields };

                    // Apply the diff
                    self.state_adapter
                        .lock()
                        .await
                        .apply_task_diff(&task_diff)
                        .await?;

                    self.update_parent_matrix_master_for_task(&task).await?;

                    self.emit_error(format!(
                        "Task {} ({}) step {} failed: {}",
                        task_id, node.id, step.name, e
                    ));

                    return Err(e);
                }
            }
        }

        // Cloud mode: finalize — fallback commit if needed, then push + create PR
        if cloud_mode {
            if let Some(ref branch) = cloud_branch_name {
                let target_path = &self.workflow_run_config.target_path;

                let git_step_logger = self.structured_logger.with_context(StepContext {
                    step_name: "Push & create pull request".to_string(),
                    step_index: node.steps.len(),
                    node_id: node.id.clone(),
                    node_name: node.name.clone(),
                    task_id: task_id.to_string(),
                    step_id: Some("_codemod_auto_push".to_string()),
                });

                git_step_logger.step_start();
                if !git_step_logger.is_jsonl() && !self.workflow_run_config.quiet {
                    println!("\x1b[1;36m⏺ Push & create pull request\x1b[0m");
                }
                let git_step_start = std::time::Instant::now();

                // If no explicit commit checkpoints were created but there are
                // uncommitted changes, create a fallback commit using the node name.
                if !had_commit_checkpoint {
                    if let Ok(true) = crate::git_ops::has_changes(target_path).await {
                        slog!(
                            git_step_logger,
                            info,
                            "No commit checkpoints in node '{}' but changes detected — creating fallback commit",
                            node.name
                        );
                        match crate::git_ops::commit(&node.name, &[], true, target_path).await {
                            Ok(true) => {
                                had_commit_checkpoint = true;
                            }
                            Ok(false) => {}
                            Err(e) => {
                                self.emit_error(format!("Fallback commit failed: {}", e));
                                // Non-fatal: continue to PR creation attempt
                            }
                        }
                    }
                }

                // Push and create PR if any commits were made
                if had_commit_checkpoint {
                    let push_and_pr_result: Result<Option<String>> = async {
                        crate::git_ops::push_branch(branch, target_path).await?;

                        // Resolve PR config or use defaults from node name
                        let (pr_title, pr_body, pr_draft, pr_base) =
                            if let Some(pr_config) = &node.pull_request {
                                let title = resolve_string_with_expression(
                                    &pr_config.title,
                                    &resolved_params,
                                    &HashMap::new(),
                                    task.matrix_values.as_ref(),
                                    None,
                                    task_expr_ctx.as_ref(),
                                )
                                .unwrap_or_else(|_| pr_config.title.clone());

                                let body = pr_config.body.as_ref().map(|b| {
                                    resolve_string_with_expression(
                                        b,
                                        &resolved_params,
                                        &HashMap::new(),
                                        task.matrix_values.as_ref(),
                                        None,
                                        task_expr_ctx.as_ref(),
                                    )
                                    .unwrap_or_else(|_| b.clone())
                                });

                                (
                                    title,
                                    body,
                                    pr_config.draft.unwrap_or(false),
                                    pr_config.base.clone(),
                                )
                            } else {
                                (node.name.clone(), None, false, None)
                            };

                        crate::git_ops::create_pull_request(
                            &pr_title,
                            pr_body.as_deref(),
                            pr_draft,
                            branch,
                            pr_base.as_deref(),
                            &task.id.to_string(),
                            target_path,
                        )
                        .await
                    }
                    .await;

                    match &push_and_pr_result {
                        Ok(Some(pr_url)) => {
                            slog!(git_step_logger, info, "Pull request created: {}", pr_url);
                        }
                        Ok(None) => {
                            slog!(git_step_logger, info, "Pull request created successfully");
                        }
                        _ => {}
                    }

                    if let Err(e) = push_and_pr_result {
                        git_step_logger
                            .step_end("failure", git_step_start.elapsed().as_millis() as u64);

                        // Mark the task as failed
                        let mut fields = HashMap::new();
                        fields.insert(
                            "status".to_string(),
                            FieldDiff {
                                operation: DiffOperation::Update,
                                value: Some(serde_json::to_value(TaskStatus::Failed)?),
                            },
                        );
                        fields.insert(
                            "ended_at".to_string(),
                            FieldDiff {
                                operation: DiffOperation::Update,
                                value: Some(serde_json::to_value(Utc::now())?),
                            },
                        );
                        fields.insert(
                            "error".to_string(),
                            FieldDiff {
                                operation: DiffOperation::Add,
                                value: Some(serde_json::to_value(format!(
                                    "Push/PR creation failed: {}",
                                    e
                                ))?),
                            },
                        );
                        let task_diff = TaskDiff { task_id, fields };
                        self.state_adapter
                            .lock()
                            .await
                            .apply_task_diff(&task_diff)
                            .await?;

                        self.emit_error(format!(
                            "Task {} ({}) push/PR creation failed: {}",
                            task_id, node.id, e
                        ));
                        return Err(e);
                    }

                    git_step_logger
                        .step_end("success", git_step_start.elapsed().as_millis() as u64);
                } else {
                    slog!(
                        git_step_logger,
                        info,
                        "No changes detected in node '{}' — skipping push and PR creation",
                        node.name
                    );
                    git_step_logger
                        .step_end("success", git_step_start.elapsed().as_millis() as u64);
                }
            }
        }

        // Prepare environment variables
        info!("Task {} all steps finished; preparing completion", task_id);
        let mut env = HashMap::new();

        // Add workflow parameters
        for (key, value) in &resolved_params {
            env.insert(format!("PARAM_{}", key.to_uppercase()), value.clone());
        }

        // Add node environment variables
        for (key, value) in &node.env {
            env.insert(key.clone(), serde_json::to_value(value.clone())?);
        }

        // Add matrix values
        if let Some(matrix_values) = &task.matrix_values {
            for (key, value) in matrix_values {
                env.insert(format!("MATRIX_{}", key.to_uppercase()), value.clone());
            }
        }

        // Create a task diff to update the status
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(TaskStatus::Completed)?),
            },
        );
        fields.insert(
            "ended_at".to_string(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(serde_json::to_value(Utc::now())?),
            },
        );
        let task_diff = TaskDiff { task_id, fields };

        // Apply the diff
        info!("Task {} applying completed status diff", task_id);
        self.state_adapter
            .lock()
            .await
            .apply_task_diff(&task_diff)
            .await?;
        info!("Task {} completed status diff applied", task_id);

        info!("Task {} ({}) completed", task_id, node.id);

        // If this is a matrix task, update the master task status
        if let Some(master_task_id) = task.master_task_id {
            info!(
                "Task {} updating matrix master task status for {}",
                task_id, master_task_id
            );
            self.update_matrix_master_status(master_task_id).await?;
            info!(
                "Task {} finished updating matrix master task status for {}",
                task_id, master_task_id
            );
        }

        // Notify that a task has completed (for event-driven waiting)
        info!("Task {} notifying task completion listeners", task_id);
        self.task_completion_notify.notify_one();
        info!("Task {} completion notification sent", task_id);

        Ok(())
    }

    /// Execute a specific step action with dependency chain tracking for cycle detection
    #[allow(clippy::too_many_arguments)]
    async fn execute_step_action(
        &self,
        runner: &dyn Runner,
        action: &StepAction,
        step_name: &str,
        step_env: &Option<HashMap<String, String>>,
        step_id: &Option<String>,
        node: &Node,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        workflow: &Workflow,
        bundle_path: &Option<PathBuf>,
        dependency_chain: &[CodemodDependency],
        capabilities: &Option<HashSet<LlrtSupportedModules>>,
        task_expr_ctx: Option<&TaskExpressionContext>,
        logger: &StructuredLogger,
    ) -> Result<()> {
        match action {
            StepAction::RunScript(run) => {
                // Skip run script steps in dry-run mode
                if self.workflow_run_config.dry_run {
                    if !self.workflow_run_config.quiet {
                        eprintln!(
                            "\n[WARN] Skipping run: script step (cannot preview):\n  {}...",
                            run.chars().take(80).collect::<String>()
                        );
                    }
                    return Ok(());
                }
                self.execute_run_script_step(
                    runner,
                    run,
                    step_name,
                    step_env,
                    step_id,
                    node,
                    task,
                    params,
                    state,
                    bundle_path,
                    logger,
                )
                .await
            }
            StepAction::UseTemplate(template_use) => {
                // Find the template using the passed workflow reference
                let template = workflow
                    .templates
                    .iter()
                    .find(|t| t.id == template_use.template)
                    .ok_or_else(|| {
                        Error::Template(format!("Template not found: {}", template_use.template))
                    })?;

                // Combine workflow params with template-specific inputs
                let mut combined_params = params.clone();
                combined_params.extend(template_use.inputs.clone());

                for template_step in &template.steps {
                    if let Some(condition) = &template_step.condition {
                        // TODO: Load step outputs from STEP_OUTPUTS file and pass here
                        let should_execute = evaluate_condition(
                            condition,
                            &combined_params,
                            state,
                            task.matrix_values.as_ref(),
                            None, // step outputs
                            task_expr_ctx,
                        )?;

                        if !should_execute {
                            info!(
                                "Skipping template step '{}' - condition not met: {}",
                                template_step.name, condition
                            );
                            continue;
                        }
                    }

                    Box::pin(self.execute_step_action(
                        runner,
                        &template_step.action,
                        &template_step.name,
                        &template_step.env,
                        &template_step.id,
                        node,
                        task,
                        &combined_params,
                        state,
                        workflow,
                        bundle_path,
                        dependency_chain,
                        capabilities,
                        task_expr_ctx,
                        logger,
                    ))
                    .await?;
                }
                Ok(())
            }
            StepAction::AstGrep(ast_grep) => {
                // Resolve ${{ }} expressions in include/exclude globs
                let mut resolved_ast_grep = ast_grep.clone();
                resolved_ast_grep.include = resolve_optional_glob_list(
                    &ast_grep.include,
                    params,
                    state,
                    task.matrix_values.as_ref(),
                    task_expr_ctx,
                )?;
                resolved_ast_grep.exclude = resolve_optional_glob_list(
                    &ast_grep.exclude,
                    params,
                    state,
                    task.matrix_values.as_ref(),
                    task_expr_ctx,
                )?;
                self.execute_ast_grep_step(node.id.clone(), &resolved_ast_grep, logger)
                    .await
            }
            StepAction::JSAstGrep(js_ast_grep) => {
                self.execute_js_ast_grep_step(
                    task.id.to_string(),
                    step_id.clone().unwrap_or_default(),
                    js_ast_grep,
                    Some(params.clone()),
                    task.matrix_values.clone(),
                    &CapabilitiesData {
                        capabilities: capabilities
                            .as_ref()
                            .map(|v| v.clone().into_iter().collect()),
                        capabilities_security_callback: self
                            .workflow_run_config
                            .capabilities_security_callback
                            .clone(),
                    },
                    bundle_path,
                    Some(task.workflow_run_id),
                    Some(state),
                    logger,
                    None,
                    task_expr_ctx,
                )
                .await
            }
            StepAction::Codemod(codemod) => {
                Box::pin(self.execute_codemod_step(
                    codemod,
                    step_env,
                    node,
                    task,
                    params,
                    state,
                    bundle_path,
                    dependency_chain,
                    capabilities,
                    logger,
                ))
                .await
            }
            StepAction::AI(ai_config) => {
                if self.workflow_run_config.dry_run {
                    info!("Skipping AI step in dry-run mode");
                    return Ok(());
                }
                self.execute_ai_step(ai_config, step_env, node, task, params, state, logger)
                    .await
            }
            StepAction::Shard(shard_config) => {
                if self.workflow_run_config.skip_shard_steps {
                    info!("Skipping shard step in dry-run preview mode");
                    return Ok(());
                }
                self.execute_shard_step(shard_config, task, logger).await
            }
            StepAction::InstallSkill(install_skill) => {
                if self.workflow_run_config.skip_install_skill_steps {
                    if self.workflow_run_config.no_interactive {
                        eprintln!(
                            "\n[INFO] install-skill step skipped in non-interactive mode by default. Re-run with --install-skill to execute this step:\n  package={}",
                            install_skill.package
                        );
                    } else {
                        eprintln!(
                            "\n[INFO] Skipping install-skill step in this run mode:\n  package={}",
                            install_skill.package
                        );
                    }
                    return Ok(());
                }

                if self.workflow_run_config.dry_run {
                    eprintln!(
                        "\n[WARN] Skipping install-skill step in dry-run mode:\n  package={}",
                        install_skill.package
                    );
                    return Ok(());
                }

                let Some(install_skill_executor) =
                    self.workflow_run_config.install_skill_executor.as_ref()
                else {
                    return Err(Error::Runtime(
                        "install-skill step requested but no install-skill executor is configured"
                            .to_string(),
                    ));
                };

                let prepared =
                    self.prepare_step_execution(step_env, node, task, state, bundle_path)?;
                let output = install_skill_executor
                    .execute(InstallSkillExecutionRequest {
                        install_skill: install_skill.clone(),
                        no_interactive: self.workflow_run_config.no_interactive,
                        target_path: self.workflow_run_config.target_path.clone(),
                        env: prepared.env.clone(),
                        output_format: self.workflow_run_config.output_format,
                    })
                    .await
                    .map_err(|error| {
                        Error::Runtime(format!("Failed to execute install-skill step: {error}"))
                    })?;

                log_step_output(logger, &output);
                self.finalize_step_execution(task, output, prepared).await
            }
        }
    }

    pub async fn execute_ast_grep_step(
        &self,
        id: String,
        ast_grep: &UseAstGrep,
        logger: &StructuredLogger,
    ) -> Result<()> {
        let bundle_path = self.workflow_run_config.bundle_path.clone();

        let config_path = bundle_path.join(&ast_grep.config_file);

        if !config_path.exists() {
            return Err(Error::StepExecution(format!(
                "AST grep config file not found: {}",
                config_path.display()
            )));
        }

        if let Some(pre_run_callback) = self.workflow_run_config.pre_run_callback.as_ref() {
            pre_run_callback(
                &self.workflow_run_config.target_path,
                self.workflow_run_config.dry_run,
            );
        }

        let config_path_clone = config_path.clone();

        with_combined_scan(
            &config_path_clone.to_string_lossy(),
            |combined_scan_with_rule| {
                let rule_refs = combined_scan_with_rule.rule_refs.clone();
                let languages = rule_refs.iter().map(|r| r.language).collect::<Vec<_>>();

                let execution_config = CodemodExecutionConfig {
                    pre_run_callback: None,
                    progress_callback: self.workflow_run_config.progress_callback.clone(),
                    target_path: Some(self.workflow_run_config.target_path.clone()),
                    base_path: ast_grep.base_path.as_deref().map(PathBuf::from),
                    include_globs: ast_grep.include.as_deref().map(|v| v.to_vec()),
                    exclude_globs: ast_grep.exclude.as_deref().map(|v| v.to_vec()),
                    dry_run: self.workflow_run_config.dry_run,
                    languages: Some(languages.iter().map(|l| l.to_string()).collect()),
                    threads: ast_grep.max_threads,
                    capabilities: None,
                };

                // Clone variables needed in the closure
                let id_clone = id.clone();
                let file_writer = Arc::clone(&self.file_writer);
                let runtime_handle = tokio::runtime::Handle::current();
                let logger = logger.clone();

                let _ = execution_config.execute(move |path, config| {
                    // Only process files, not directories
                    if !path.is_file() {
                        return;
                    }

                    // Execute ast-grep on this file
                    match scan_file_with_combined_scan(
                        path,
                        &combined_scan_with_rule.combined_scan,
                        !config.dry_run, // apply_fixes = !dry_run
                    ) {
                        Ok((matches, file_modified, new_content)) => {
                            if !matches.is_empty() {
                                slog!(
                                    logger,
                                    info,
                                    "Found {} matches in {}",
                                    matches.len(),
                                    path.display()
                                );
                            }
                            if file_modified {
                                if let Some(new_content) = new_content {
                                    // Use async file writing to avoid blocking the thread
                                    let write_result = runtime_handle.block_on(async {
                                        file_writer
                                            .write_file(path.to_path_buf(), new_content)
                                            .await
                                    });

                                    if let Err(e) = write_result {
                                        slog!(
                                            logger,
                                            error,
                                            "Failed to write modified file {}: {}",
                                            path.display(),
                                            e
                                        );
                                        self.execution_stats
                                            .files_with_errors
                                            .fetch_add(1, Ordering::Relaxed);
                                        return;
                                    }
                                }
                                self.execution_stats
                                    .files_modified
                                    .fetch_add(1, Ordering::Relaxed);
                            } else {
                                self.execution_stats
                                    .files_unmodified
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(e) => {
                            slog!(logger, error, "{e}");
                            self.execution_stats
                                .files_with_errors
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    };

                    if let Some(callback) = self.workflow_run_config.progress_callback.as_ref() {
                        let callback = callback.callback.clone();
                        callback(&id_clone, &path.to_string_lossy(), "next", Some(&1), &0);
                    }
                });

                Ok(())
            },
        )
        .map_err(|e| Error::StepExecution(e.to_string()))?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_js_ast_grep_step(
        &self,
        id: String,
        step_id: String,
        js_ast_grep: &UseJSAstGrep,
        params: Option<HashMap<String, serde_json::Value>>,
        matrix_input: Option<HashMap<String, serde_json::Value>>,
        capabilities_data: &CapabilitiesData,
        bundle_path: &Option<PathBuf>,
        workflow_run_id: Option<Uuid>,
        initial_state: Option<&HashMap<String, serde_json::Value>>,
        logger: &StructuredLogger,
        modified_files_collector: Option<Arc<std::sync::Mutex<Vec<PathBuf>>>>,
        task_expr_ctx: Option<&TaskExpressionContext>,
    ) -> Result<()> {
        let metrics_context = self.metrics_context.clone();
        let task_log_task_id = Uuid::parse_str(&id).ok();

        // Use the passed bundle_path if provided, otherwise fall back to workflow_run_config.bundle_path
        let effective_bundle_path = bundle_path
            .as_ref()
            .unwrap_or(&self.workflow_run_config.bundle_path);
        let js_file_path = effective_bundle_path.join(&js_ast_grep.js_file);

        // Combine target_path with base_path if base_path is specified
        let target_path = if let Some(base_path) = &js_ast_grep.base_path {
            self.workflow_run_config.target_path.join(base_path)
        } else {
            self.workflow_run_config.target_path.clone()
        };

        if let Some(pre_run_callback) = self.workflow_run_config.pre_run_callback.as_deref() {
            pre_run_callback(target_path.as_path(), js_ast_grep.dry_run.unwrap_or(false));
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

        let capabilities_security_callback_clone =
            capabilities_data.capabilities_security_callback.clone();
        let quiet = self.workflow_run_config.quiet;
        let pre_run_callback = PreRunCallback {
            callback: Arc::new(Box::new(move |_, _, config: &CodemodExecutionConfig| {
                if let Some(callback) = &capabilities_security_callback_clone {
                    callback(config).map_err(|e| {
                        if !quiet {
                            error!("Failed to check capabilities: {e}");
                        }
                        Box::<dyn std::error::Error + Send + Sync>::from(format!(
                            "Failed to check capabilities: {e}"
                        ))
                    })?;
                }
                Ok(())
            })),
        };
        // Resolve ${{ }} expressions in include/exclude globs.
        let empty_params = HashMap::new();
        let empty_state = HashMap::new();
        let resolved_params_ref = params.as_ref().unwrap_or(&empty_params);
        let resolved_state_ref = initial_state.unwrap_or(&empty_state);

        let resolved_include = resolve_optional_glob_list(
            &js_ast_grep.include,
            resolved_params_ref,
            resolved_state_ref,
            matrix_input.as_ref(),
            task_expr_ctx,
        )?
        .or_else(|| {
            // Auto-apply matrix._meta_files as the include list when no
            // explicit include is configured or resolves to empty.
            matrix_input
                .as_ref()
                .and_then(|m| m.get("_meta_files"))
                .and_then(butterflow_models::variable::value_to_string_vec)
        });

        let resolved_exclude = resolve_optional_glob_list(
            &js_ast_grep.exclude,
            resolved_params_ref,
            resolved_state_ref,
            matrix_input.as_ref(),
            task_expr_ctx,
        )?;

        let config = CodemodExecutionConfig {
            pre_run_callback: Some(pre_run_callback),
            progress_callback: self.workflow_run_config.progress_callback.clone(),
            target_path: Some(target_path.clone()),
            base_path: None,
            include_globs: resolved_include,
            exclude_globs: resolved_exclude,
            dry_run: js_ast_grep.dry_run.unwrap_or(false) || self.workflow_run_config.dry_run,
            languages: Some(vec![js_ast_grep
                .language
                .clone()
                .unwrap_or("typescript".to_string())]),
            threads: js_ast_grep.max_threads,
            capabilities: capabilities_data
                .capabilities
                .as_ref()
                .map(|v| v.clone().into_iter().collect()),
        };

        // Set language first to get default extensions
        let language = if let Some(lang_str) = &js_ast_grep.language {
            lang_str
                .parse()
                .map_err(|e| Error::StepExecution(format!("Invalid language '{lang_str}': {e}")))?
        } else {
            // Parse TypeScript as default
            "typescript".parse().map_err(|e| {
                Error::StepExecution(format!("Failed to parse default language: {e}"))
            })?
        };

        let selector_config = extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &js_file_path,
            language,
            resolver: Arc::clone(&resolver),
            capabilities: capabilities_data
                .capabilities
                .as_ref()
                .map(|v| v.clone().into_iter().collect()),
        })
        .await
        .map_err(|e| Error::StepExecution(format!("Failed to extract selector: {e}")))?;

        let semantic_provider: Option<Arc<dyn SemanticProvider>> =
            match &js_ast_grep.semantic_analysis {
                Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)) => {
                    Some(Arc::new(LazySemanticProvider::file_scope()))
                }
                Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::Workspace)) => {
                    // use target_path as workspace root by default
                    Some(Arc::new(LazySemanticProvider::workspace_scope(
                        target_path.clone(),
                    )))
                }
                Some(SemanticAnalysisConfig::Detailed(detailed)) => {
                    match detailed.mode {
                        SemanticAnalysisMode::File => {
                            Some(Arc::new(LazySemanticProvider::file_scope()))
                        }
                        SemanticAnalysisMode::Workspace => {
                            // use custom root if provided, otherwise use target_path
                            let root = detailed
                                .root
                                .as_ref()
                                .map(PathBuf::from)
                                .unwrap_or_else(|| target_path.clone());
                            Some(Arc::new(LazySemanticProvider::workspace_scope(root)))
                        }
                    }
                }
                None => None,
            };

        // For workspace scope semantic analysis, pre-index all target files
        // This ensures cross-file references work correctly
        if let Some(ref provider) = semantic_provider {
            if provider.mode() == language_core::ProviderMode::WorkspaceScope {
                let target_files: Vec<PathBuf> = config.collect_files();

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
            }
        }

        // Capture variables for use in parallel threads
        let runtime_handle = tokio::runtime::Handle::current();
        let js_file_path_clone = js_file_path.clone();
        let resolver_clone = resolver.clone();
        let id_clone = Arc::new(id);
        let progress_callback = self.workflow_run_config.progress_callback.clone();
        let file_writer = Arc::clone(&self.file_writer);
        let selector_config = selector_config.map(Arc::new);
        let shared_state_context = if let Some(state) = initial_state {
            SharedStateContext::with_initial_state(state.clone())
        } else {
            SharedStateContext::new()
        };
        let metrics_context_clone = metrics_context.clone();
        let shared_state_context_clone = shared_state_context.clone();
        let logger = logger.clone();
        let modified_files_collector_clone = modified_files_collector.clone();
        let state_adapter = Arc::clone(&self.state_adapter);
        let target_path_for_logs = target_path.clone();
        let canceled_during_execution = Arc::new(AtomicBool::new(false));
        let idle_timeout = js_ast_grep_idle_timeout();
        let progress_state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
        let idle_timed_out = Arc::new(AtomicBool::new(false));
        let watchdog_done = Arc::new(AtomicBool::new(false));
        let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));

        // Collect deferred file deletions from renames — applied after all transforms complete
        let deferred_deletions: Arc<std::sync::Mutex<Vec<PathBuf>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let logger_for_deferred = logger.clone();
        let deferred_deletions_clone = Arc::clone(&deferred_deletions);
        let workflow_run_id_for_cancel = workflow_run_id;
        let canceled_flag_for_closure = Arc::clone(&canceled_during_execution);
        let progress_state_for_closure = Arc::clone(&progress_state);
        let progress_state_for_watchdog = Arc::clone(&progress_state);
        let idle_timed_out_for_watchdog = Arc::clone(&idle_timed_out);
        let watchdog_done_for_watchdog = Arc::clone(&watchdog_done);
        let idle_failure_message_for_watchdog = Arc::clone(&idle_failure_message);
        let state_adapter_for_watchdog = Arc::clone(&self.state_adapter);

        if let Some(task_id) = task_log_task_id {
            let progress_state_for_output = Arc::clone(&progress_state);
            self.register_output_heartbeat(
                task_id,
                Arc::new(move || {
                    record_output_progress(&progress_state_for_output);
                }),
            );
        }

        let watchdog_task = {
            tokio::spawn(async move {
                loop {
                    if watchdog_done_for_watchdog.load(Ordering::Acquire) {
                        break;
                    }

                    tokio::time::sleep(Duration::from_secs(1)).await;

                    if watchdog_done_for_watchdog.load(Ordering::Acquire) {
                        break;
                    }

                    let timed_out_message = {
                        let state = progress_state_for_watchdog.lock().unwrap();
                        if state.global_last_progress_at.elapsed() > idle_timeout {
                            Some(build_js_ast_grep_idle_timeout_message(&state, idle_timeout))
                        } else {
                            None
                        }
                    };

                    if let Some(message) = timed_out_message {
                        idle_timed_out_for_watchdog.store(true, Ordering::Release);
                        if let Ok(mut slot) = idle_failure_message_for_watchdog.lock() {
                            *slot = Some(message.clone());
                        }

                        if let Some(task_id) = task_log_task_id {
                            let mut adapter = state_adapter_for_watchdog.lock().await;
                            if let Ok(mut task) = adapter.get_task(task_id).await {
                                task.logs.push(message);
                                let _ = adapter.save_task(&task).await;
                            }
                        }
                        break;
                    }
                }
            })
        };

        // Execute the codemod on each file using the config's multi-threading
        let idle_timed_out_for_closure = Arc::clone(&idle_timed_out);
        let idle_failure_message_for_closure = Arc::clone(&idle_failure_message);
        let runtime_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));
        let runtime_failure_message_for_closure = Arc::clone(&runtime_failure_message);

        let execute_result = config
            .execute(move |file_path, config| {
                if canceled_flag_for_closure.load(Ordering::Acquire)
                    || idle_timed_out_for_closure.load(Ordering::Acquire)
                {
                    return;
                }

                if let (Some(task_id), Some(run_id)) =
                    (task_log_task_id, workflow_run_id_for_cancel)
                {
                    let state_adapter = Arc::clone(&state_adapter);
                    let was_canceled = runtime_handle.block_on(async move {
                        let adapter = state_adapter.lock().await;
                        let workflow_canceled = adapter
                            .get_workflow_run(run_id)
                            .await
                            .ok()
                            .is_some_and(|run| run.status == WorkflowStatus::Canceled);
                        if workflow_canceled {
                            return true;
                        }
                        adapter.get_task(task_id).await.ok().is_some_and(|task| {
                            task.status == TaskStatus::Failed
                                && task.error.as_deref() == Some("Canceled by user")
                        })
                    });

                    if was_canceled {
                        canceled_flag_for_closure.store(true, Ordering::Release);
                        return;
                    }
                }

                // Only process files
                if !file_path.is_file() {
                    return;
                }

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

                if let Some(task_id) = task_log_task_id {
                    let state_adapter = Arc::clone(&state_adapter);
                    let progress_message = format!("Processing file: {relative_path}");
                    runtime_handle.block_on(async move {
                        let mut adapter = state_adapter.lock().await;
                        if let Ok(mut task) = adapter.get_task(task_id).await {
                            task.logs.push(progress_message);
                            let _ = adapter.save_task(&task).await;
                        }
                    });
                }

                // Read file content synchronously
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
                        return;
                    }
                };
                record_unit_progress(
                    &progress_state_for_closure,
                    &relative_path,
                    StepPhase::FileLoaded,
                );

                // Execute the async codemod using the captured runtime handle
                std::env::set_var("CODEMOD_STEP_ID", &step_id);
                record_unit_progress(
                    &progress_state_for_closure,
                    &relative_path,
                    StepPhase::ExecutionStarted,
                );
                let dry_run = config.dry_run;
                let relative_path_for_execution = relative_path.clone();
                let progress_state_for_execution = Arc::clone(&progress_state_for_closure);
                let cancellation_flag_for_execution = Arc::clone(&canceled_flag_for_closure);
                let current_runtime_unit = Arc::new(std::sync::Mutex::new(relative_path.clone()));
                let current_runtime_unit_for_callback = Arc::clone(&current_runtime_unit);
                let progress_state_for_runtime_events = Arc::clone(&progress_state_for_closure);
                let state_adapter_for_runtime_events = Arc::clone(&state_adapter);
                let runtime_handle_for_runtime_events = runtime_handle.clone();
                let relative_path_for_runtime_events = relative_path.clone();
                let runtime_event_task_id = task_log_task_id;
                let runtime_event_callback: RuntimeEventCallback =
                    Arc::new(move |event| match event.kind {
                        RuntimeEventKind::SetCurrentUnit => {
                            let new_runtime_unit =
                                format!("{relative_path_for_runtime_events} :: {}", event.message);
                            let previous_runtime_unit = {
                                let mut current_runtime_unit = current_runtime_unit_for_callback
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
                        }
                        RuntimeEventKind::Progress | RuntimeEventKind::Warn => {
                            let runtime_unit = current_runtime_unit_for_callback
                                .lock()
                                .map(|runtime_unit| runtime_unit.clone())
                                .unwrap_or_else(|_| relative_path_for_runtime_events.clone());
                            record_unit_progress(
                                &progress_state_for_runtime_events,
                                &runtime_unit,
                                StepPhase::Output,
                            );
                            if let (Some(task_id), Some(message)) =
                                (runtime_event_task_id, format_runtime_event_log(&event))
                            {
                                let state_adapter = Arc::clone(&state_adapter_for_runtime_events);
                                std::mem::drop(runtime_handle_for_runtime_events.spawn(
                                    async move {
                                        let mut adapter = state_adapter.lock().await;
                                        if let Ok(mut task) = adapter.get_task(task_id).await {
                                            task.logs.push(message);
                                            let _ = adapter.save_task(&task).await;
                                        }
                                    },
                                ));
                            }
                        }
                    });
                let execution_result = runtime_handle.block_on(async {
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
                                    target_directory: Some(&target_path_owned),
                                })
                                .await
                            });

                            await_js_ast_grep_execution_task(
                                execution_task,
                                idle_timed_out,
                                idle_failure_message,
                                progress_state_for_execution,
                                idle_timeout,
                                &relative_path_for_execution,
                            )
                            .await
                        })
                        .await
                });

                if let (Some(task_id), Some(run_id)) =
                    (task_log_task_id, workflow_run_id_for_cancel)
                {
                    let state_adapter = Arc::clone(&state_adapter);
                    let was_canceled = runtime_handle.block_on(async move {
                        let adapter = state_adapter.lock().await;
                        let workflow_canceled = adapter
                            .get_workflow_run(run_id)
                            .await
                            .ok()
                            .is_some_and(|run| run.status == WorkflowStatus::Canceled);
                        if workflow_canceled {
                            return true;
                        }
                        adapter.get_task(task_id).await.ok().is_some_and(|task| {
                            task.status == TaskStatus::Failed
                                && task.error.as_deref() == Some("Canceled by user")
                        })
                    });

                    if was_canceled {
                        canceled_flag_for_closure.store(true, Ordering::Release);
                        finish_unit_progress(
                            &progress_state_for_closure,
                            &relative_path,
                            StepPhase::ExecutionErrored,
                        );
                        return;
                    }
                }

                match execution_result {
                    Ok(Ok(CodemodOutput { primary, secondary })) => {
                        let apply_change = |change_path: &Path, result: &ExecutionResult| {
                            match result {
                                ExecutionResult::Modified(ref modified) => {
                                    let write_path =
                                        modified.rename_to.as_deref().unwrap_or(change_path);
                                    if config.dry_run {
                                        self.execution_stats
                                            .files_modified
                                            .fetch_add(1, Ordering::Relaxed);

                                        // Report the change via callback if provided
                                        if let Some(callback) =
                                            &self.workflow_run_config.dry_run_callback
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
                                            });
                                        }

                                        slog!(
                                            logger,
                                            debug,
                                            "Would modify file (dry run): {}",
                                            change_path.display()
                                        );
                                    } else {
                                        // Capture diff for report before writing (original content is still on disk)
                                        if let Some(callback) =
                                            &self.workflow_run_config.dry_run_callback
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
                                            });
                                        }

                                        // Use async file writing to avoid blocking the thread
                                        let write_result = runtime_handle.block_on(async {
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
                                            self.execution_stats
                                                .files_with_errors
                                                .fetch_add(1, Ordering::Relaxed);
                                        } else {
                                            // If renamed, defer deletion of the original file
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
                                            self.execution_stats
                                                .files_modified
                                                .fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                }
                                ExecutionResult::Unmodified | ExecutionResult::Skipped => {}
                            }
                        };

                        match &primary {
                            ExecutionResult::Modified(_) => {
                                apply_change(file_path, &primary);
                                if let Some(ref collector) = modified_files_collector_clone {
                                    collector.lock().unwrap().push(file_path.to_path_buf());
                                }
                            }
                            ExecutionResult::Unmodified | ExecutionResult::Skipped => {
                                self.execution_stats
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
                            if let Some(task_id) = task_log_task_id {
                                let state_adapter = Arc::clone(&state_adapter);
                                let message_for_log = message.clone();
                                runtime_handle.block_on(async move {
                                    let mut adapter = state_adapter.lock().await;
                                    if let Ok(mut task) = adapter.get_task(task_id).await {
                                        task.logs.push(message_for_log);
                                        let _ = adapter.save_task(&task).await;
                                    }
                                });
                            }
                            canceled_flag_for_closure.store(true, Ordering::Release);
                            if let Ok(mut runtime_failure_message) =
                                runtime_failure_message_for_closure.lock()
                            {
                                if runtime_failure_message.is_none() {
                                    *runtime_failure_message = Some(message);
                                }
                            }
                        }
                        slog!(
                            logger,
                            error,
                            "Failed to execute codemod on {}: {:?}",
                            relative_path,
                            e
                        );
                        if let Some(task_id) = task_log_task_id {
                            let state_adapter = Arc::clone(&state_adapter);
                            let message = format!("Failed to process {relative_path}: {e}");
                            runtime_handle.block_on(async move {
                                let mut adapter = state_adapter.lock().await;
                                if let Ok(mut task) = adapter.get_task(task_id).await {
                                    task.logs.push(message);
                                    let _ = adapter.save_task(&task).await;
                                }
                            });
                        }
                        self.execution_stats
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
                            "Failed to execute codemod on {}: {:?}",
                            relative_path,
                            e
                        );
                        if let Some(task_id) = task_log_task_id {
                            let state_adapter = Arc::clone(&state_adapter);
                            let message = format!("Failed to process {relative_path}: {e}");
                            runtime_handle.block_on(async move {
                                let mut adapter = state_adapter.lock().await;
                                if let Ok(mut task) = adapter.get_task(task_id).await {
                                    task.logs.push(message);
                                    let _ = adapter.save_task(&task).await;
                                }
                            });
                        }
                        self.execution_stats
                            .files_with_errors
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }

                if let Some(callback) = progress_callback.as_ref() {
                    let callback = callback.callback.clone();
                    callback(
                        &id_clone,
                        &file_path.to_string_lossy(),
                        "next",
                        Some(&1),
                        &0,
                    );
                }
            })
            .map_err(|e| Error::StepExecution(e.to_string()));

        watchdog_done.store(true, Ordering::Release);
        let _ = watchdog_task.await;
        if let Some(task_id) = task_log_task_id {
            self.unregister_output_heartbeat(task_id);
        }

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

        execute_result?;

        if let Some(message) = runtime_failure_message
            .lock()
            .ok()
            .and_then(|message| message.clone())
        {
            return Err(Error::StepExecution(message));
        }

        if canceled_during_execution.load(Ordering::Acquire) {
            return Err(Error::Runtime("Canceled by user".to_string()));
        }

        // Apply deferred file deletions from renames now that all transforms are complete
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

        // Persist shared state to the workflow state adapter
        if let Some(wf_run_id) = workflow_run_id {
            if !self.workflow_run_config.skip_state_writes {
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

                    self.state_adapter
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

    /// Execute an AI agent step
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_ai_step(
        &self,
        ai_config: &UseAI,
        _step_env: &Option<HashMap<String, String>>,
        _node: &Node,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        logger: &StructuredLogger,
    ) -> Result<()> {
        // Resolve the prompt with parameters, state, and matrix values
        // TODO: Load step outputs from STEP_OUTPUTS file and pass here
        let resolved_prompt = resolve_string_with_expression(
            &ai_config.prompt,
            params,
            state,
            task.matrix_values.as_ref(),
            None, // step outputs
            None, // task context
        )?;

        slog!(
            logger,
            info,
            "Executing AI agent step with prompt: {}",
            resolved_prompt
        );

        // 1. Check if running inside a parent coding agent
        let handoff_detection = detect_parent_coding_agent();
        let detected_agent = handoff_detection.agent_name.as_deref().unwrap_or("none");
        slog!(
            logger,
            info,
            "AI handoff detection confidence={} agent={} reasons={}",
            handoff_detection.confidence.as_str(),
            detected_agent,
            handoff_detection.reasons.join(" | ")
        );

        if handoff_detection.confidence == DetectionConfidence::Detected {
            self.emit_ai_instructions(logger, ai_config.system_prompt.as_deref(), &resolved_prompt);
            slog!(
                logger,
                info,
                "AI handoff mode=handoff confidence={} agent={}",
                handoff_detection.confidence.as_str(),
                detected_agent
            );
            return Ok(());
        }

        // 2. Check if agent was explicitly specified via --agent
        if let Some(ref agent_name) = self.workflow_run_config.agent {
            if let Some(canonical) = resolve_agent_name(agent_name) {
                slog!(logger, info, "Agent specified via --agent: {}", canonical);
                if let Some(executable) = find_agent_executable(canonical) {
                    return self
                        .launch_agent(
                            canonical,
                            &executable,
                            ai_config.system_prompt.as_deref(),
                            &resolved_prompt,
                            logger,
                        )
                        .await;
                } else {
                    slog!(
                        logger,
                        warn,
                        "Agent '{}' is not installed, falling back to built-in AI",
                        canonical
                    );
                }
            } else {
                slog!(
                    logger,
                    warn,
                    "Unknown agent '{}' specified via --agent, ignoring",
                    agent_name
                );
            }
        }

        // 2b. Check for LLM_AGENT env var (useful in non-interactive mode)
        if let Ok(env_agent) = std::env::var("LLM_AGENT") {
            if !env_agent.is_empty() {
                if let Some(canonical) = resolve_agent_name(&env_agent) {
                    slog!(
                        logger,
                        info,
                        "Agent specified via LLM_AGENT env var: {}",
                        canonical
                    );
                    if let Some(executable) = find_agent_executable(canonical) {
                        return self
                            .launch_agent(
                                canonical,
                                &executable,
                                ai_config.system_prompt.as_deref(),
                                &resolved_prompt,
                                logger,
                            )
                            .await;
                    } else {
                        slog!(
                            logger,
                            warn,
                            "Agent '{}' from LLM_AGENT is not installed, falling back to built-in AI",
                            canonical
                        );
                    }
                } else {
                    slog!(
                        logger,
                        warn,
                        "Unknown agent '{}' specified via LLM_AGENT, ignoring",
                        env_agent
                    );
                }
            }
        }

        // 3. If interactive, discover installed agents and prompt user to select
        if !self.workflow_run_config.no_interactive {
            if let Some(ref callback) = self.workflow_run_config.agent_selection_callback {
                let agents = discover_installed_agents();

                // Loop to allow preview → re-select flow
                let mut selection_result = callback(&agents);
                loop {
                    match selection_result.as_deref() {
                        Some("__preview_prompt__") => {
                            // Show the prompt, then re-prompt for agent selection
                            self.emit_ai_instructions(
                                logger,
                                ai_config.system_prompt.as_deref(),
                                &resolved_prompt,
                            );
                            if !self.workflow_run_config.quiet {
                                eprintln!();
                            }
                            selection_result = callback(&agents);
                            continue;
                        }
                        Some("__print_prompt__") => {
                            slog!(logger, info, "User chose to print prompt and skip");
                            self.emit_ai_instructions(
                                logger,
                                ai_config.system_prompt.as_deref(),
                                &resolved_prompt,
                            );
                            return Ok(());
                        }
                        Some(selected) => {
                            slog!(logger, info, "User selected agent: {}", selected);
                            if let Some(executable) = find_agent_executable(selected) {
                                return self
                                    .launch_agent(
                                        selected,
                                        &executable,
                                        ai_config.system_prompt.as_deref(),
                                        &resolved_prompt,
                                        logger,
                                    )
                                    .await;
                            } else {
                                slog!(
                                    logger,
                                    warn,
                                    "Agent '{}' executable not found, falling back to built-in AI",
                                    selected
                                );
                            }
                        }
                        None => {
                            // User dismissed the selection — fall through to built-in AI
                        }
                    }
                    break;
                }
            }
        }

        slog!(
            logger,
            info,
            "AI handoff mode=rig confidence={} agent={}",
            handoff_detection.confidence.as_str(),
            detected_agent
        );

        // Configure LLM settings - check for API key from config or environment
        let api_key = match ai_config
            .api_key
            .clone()
            .or_else(|| std::env::var("LLM_API_KEY").ok())
        {
            Some(key) => key,
            None => {
                // No API key - surface instructions for coding agents and skip the step
                self.emit_ai_instructions(
                    logger,
                    ai_config.system_prompt.as_deref(),
                    &resolved_prompt,
                );

                slog!(
                    logger,
                    info,
                    "Skipping AI step - no API key provided. See [AI INSTRUCTIONS] above."
                );
                return Ok(());
            }
        };

        let model = ai_config
            .model
            .clone()
            .or_else(|| std::env::var("LLM_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o".to_string());

        let llm_provider = ai_config
            .llm_protocol
            .clone()
            .or_else(|| std::env::var("LLM_PROVIDER").ok())
            .unwrap_or_else(|| "openai".to_string());

        let endpoint = ai_config
            .endpoint
            .clone()
            .or_else(|| std::env::var("LLM_BASE_URL").ok())
            .unwrap_or_else(|| match llm_provider.as_str() {
                "openai" => "https://api.openai.com/v1".to_string(),
                "anthropic" => "https://api.anthropic.com".to_string(),
                "google_ai" => "https://generativelanguage.googleapis.com/v1beta".to_string(),
                "azure_openai" => "https://api.openai.com".to_string(),
                _ => "https://api.openai.com/v1".to_string(),
            });

        let config = ExecuteAiStepConfig {
            api_key,
            endpoint,
            model,
            system_prompt: ai_config.system_prompt.clone(),
            max_steps: ai_config.max_steps,
            tools: ai_config.tools.clone(),
            prompt: resolved_prompt,
            working_dir: self.workflow_run_config.target_path.clone(),
            llm_protocol: llm_provider,
        };

        let mut ai_future = std::pin::pin!(execute_ai_step(config));
        let mut progress_interval = time::interval_at(
            time::Instant::now() + Duration::from_secs(20),
            Duration::from_secs(20),
        );
        progress_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut elapsed_seconds = 0u64;

        let ai_result = loop {
            tokio::select! {
                result = &mut ai_future => {
                    break result;
                }
                _ = progress_interval.tick() => {
                    elapsed_seconds += 20;
                    slog!(
                        logger,
                        info,
                        "AI step still running ({}s elapsed); waiting for model/tool completion...",
                        elapsed_seconds
                    );
                }
            }
        };

        let output = match ai_result {
            Ok(output) => output,
            Err(error) => {
                if let Some(diagnostics) = error.memory_exhaustion_diagnostics() {
                    let trigger = diagnostics.trigger.as_deref().unwrap_or("unknown");
                    let before_chars = diagnostics.before_chars.unwrap_or(0);
                    let after_chars = diagnostics.after_chars.unwrap_or(0);
                    let archived_docs = diagnostics.archived_docs.unwrap_or(0);
                    let retrieved_docs = diagnostics.retrieved_docs.unwrap_or(0);
                    slog!(
                        logger,
                        info,
                        "AI memory exhaustion: attempts={} trigger={} before_chars={} after_chars={} archived_docs={} retrieved_docs={} soft_char_budget={} soft_token_budget={}",
                        diagnostics.attempts,
                        trigger,
                        before_chars,
                        after_chars,
                        archived_docs,
                        retrieved_docs,
                        diagnostics.soft_char_budget,
                        diagnostics.soft_token_budget
                    );
                }
                return Err(Error::StepExecution(error.to_string()));
            }
        };

        for event in &output.compaction_events {
            slog!(
                logger,
                info,
                "AI memory compaction applied: attempt={} trigger={} before_chars={} after_chars={} archived_docs={} retrieved_docs={}",
                event.attempt,
                event.trigger,
                event.before_chars,
                event.after_chars,
                event.archived_docs,
                event.retrieved_docs
            );
        }

        let ai_output = output.data.unwrap_or_default();
        slog!(logger, info, "AI agent output:\n{ai_output}");
        slog!(logger, info, "AI agent step completed successfully");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_codemod_step(
        &self,
        codemod: &UseCodemod,
        step_env: &Option<HashMap<String, String>>,
        node: &Node,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        bundle_path: &Option<PathBuf>,
        dependency_chain: &[CodemodDependency],
        capabilities: &Option<HashSet<LlrtSupportedModules>>,
        logger: &StructuredLogger,
    ) -> Result<()> {
        slog!(logger, info, "Executing codemod step: {}", codemod.source);

        // Check for runtime cycles before execution
        if let Some(cycle_start) = self.find_cycle_in_chain(&codemod.source, dependency_chain) {
            let chain_str = dependency_chain
                .iter()
                .map(|d| d.source.as_str())
                .collect::<Vec<_>>()
                .join(" → ");

            return Err(Error::Other(format!(
                "Runtime codemod dependency cycle detected!\n\
                Cycle: {} → {} → {}\n\
                This cycle was not caught during validation, indicating a dynamic dependency.\n\
                Please review your codemod dependencies to remove the circular reference.",
                cycle_start,
                if chain_str.is_empty() {
                    "(root)"
                } else {
                    &chain_str
                },
                codemod.source
            )));
        }

        // Resolve the package (local path or registry package)
        let resolved_package = self
            .workflow_run_config
            .registry_client
            .resolve_package(&codemod.source, None, false, None)
            .await
            .map_err(|e| Error::Other(format!("Failed to resolve package: {e}")))?;

        slog!(
            logger,
            info,
            "Resolved codemod package: {} -> {}",
            codemod.source,
            resolved_package.package_dir.display()
        );

        // Create new dependency chain including this codemod
        let mut new_chain = dependency_chain.to_vec();
        new_chain.push(CodemodDependency {
            source: codemod.source.clone(),
        });

        // Execute the resolved codemod workflow
        self.run_codemod_workflow_with_chain(
            &resolved_package,
            codemod,
            step_env,
            node,
            task,
            params,
            state,
            bundle_path,
            &new_chain,
            capabilities,
            logger,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_codemod_workflow_with_chain(
        &self,
        resolved_package: &ResolvedPackage,
        codemod: &UseCodemod,
        step_env: &Option<HashMap<String, String>>,
        _node: &Node,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        bundle_path: &Option<PathBuf>,
        dependency_chain: &[CodemodDependency],
        capabilities: &Option<HashSet<LlrtSupportedModules>>,
        logger: &StructuredLogger,
    ) -> Result<()> {
        let workflow_path = resolved_package.package_dir.join("workflow.yaml");

        if !workflow_path.exists() {
            return Err(Error::Other(format!(
                "Workflow file not found in codemod package: {}",
                workflow_path.display()
            )));
        }

        // Load the codemod workflow
        let workflow_content = std::fs::read_to_string(&workflow_path)
            .map_err(|e| Error::Other(format!("Failed to read workflow file: {e}")))?;

        let codemod_workflow: Workflow = serde_yaml::from_str(&workflow_content)
            .map_err(|e| Error::Other(format!("Failed to parse workflow YAML: {e}")))?;

        // Prepare parameters for the codemod workflow
        let mut codemod_params = HashMap::new();

        // Add arguments as parameters if provided
        if let Some(args) = &codemod.args {
            for (i, arg) in args.iter().enumerate() {
                codemod_params.insert(format!("arg_{i}"), arg.clone());

                // Also try to parse key=value format
                if let Some((key, value)) = arg.split_once('=') {
                    codemod_params.insert(key.to_string(), value.to_string());
                }
            }
        }

        // Add environment variables from step configuration
        if let Some(env) = &codemod.env {
            for (key, value) in env {
                codemod_params.insert(format!("env_{key}"), value.clone());
            }
        }

        // Add step-level environment variables
        if let Some(step_env) = step_env {
            for (key, value) in step_env {
                codemod_params.insert(format!("env_{key}"), value.clone());
            }
        }

        // Resolve working directory
        let working_dir = if let Some(wd) = &codemod.working_dir {
            if wd.starts_with("/") {
                PathBuf::from(wd)
            } else if let Some(base) = bundle_path {
                base.join(wd)
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(wd)
            }
        } else {
            // Default to current working directory
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        codemod_params.insert(
            "working_dir".to_string(),
            working_dir.to_string_lossy().to_string(),
        );

        slog!(
            logger,
            info,
            "Running codemod workflow: {} with {} parameters",
            resolved_package.spec.name,
            codemod_params.len()
        );

        // Execute the codemod workflow synchronously by running its steps directly
        // This avoids the recursive engine execution cycle
        slog!(logger, info, "Executing codemod workflow steps directly");

        // Create a direct runner for executing the codemod steps
        let runner: Box<dyn Runner> =
            Box::new(DirectRunner::with_quiet(self.workflow_run_config.quiet));

        // Build task expression context for variable resolution in codemod steps
        let codemod_task_expr_ctx =
            crate::git_ops::build_task_expression_context(&task.id.to_string());

        // Execute each node in the codemod workflow
        for node in &codemod_workflow.nodes {
            for step in &node.steps {
                if let Some(condition) = &step.condition {
                    let should_execute = evaluate_condition(
                        condition,
                        params,
                        state,
                        task.matrix_values.as_ref(),
                        None,
                        Some(&codemod_task_expr_ctx),
                    )?;
                    if !should_execute {
                        slog!(
                            logger,
                            info,
                            "Skipping codemod step '{}' - condition not met: {}",
                            step.name,
                            condition
                        );
                        continue;
                    }
                }

                Box::pin(self.execute_step_action(
                    runner.as_ref(),
                    &step.action,
                    &step.name,
                    &step.env,
                    &step.id,
                    node,
                    task, // Use the current task context
                    params,
                    state,
                    &codemod_workflow,
                    &Some(resolved_package.package_dir.clone()),
                    dependency_chain,
                    capabilities,
                    Some(&codemod_task_expr_ctx),
                    logger,
                ))
                .await?;
            }
        }

        slog!(logger, info, "Codemod workflow completed successfully");
        Ok(())
    }

    /// Execute a shard step — evaluate file shards and write results to workflow state.
    async fn execute_shard_step(
        &self,
        shard_config: &butterflow_models::step::UseShard,
        task: &Task,
        logger: &StructuredLogger,
    ) -> Result<()> {
        use crate::shard::evaluate_builtin_shards;
        use butterflow_models::step::ShardMethod;

        let target_path = self.workflow_run_config.target_path.clone();

        // If a js-ast-grep config is set, pre-scan to find only files with matches
        let eligible_files = if let Some(js_ast_grep) = &shard_config.js_ast_grep {
            Some(
                self.scan_eligible_files_with_jssg(shard_config, js_ast_grep, &target_path, logger)
                    .await?,
            )
        } else {
            None
        };

        // Load previous shard state for incremental evaluation
        let previous_shards = {
            let state = self
                .state_adapter
                .lock()
                .await
                .get_state(task.workflow_run_id)
                .await?;
            state
                .get(&shard_config.output_state)
                .and_then(|v| v.as_array())
                .cloned()
        };

        let shards = match &shard_config.method {
            ShardMethod::Builtin(_) => evaluate_builtin_shards(
                shard_config,
                &target_path,
                eligible_files.as_deref(),
                previous_shards.as_ref(),
            )
            .map_err(|e| Error::Runtime(format!("Shard evaluation failed: {e}")))?,
            ShardMethod::Function(func) => {
                self.execute_custom_shard_function(
                    shard_config,
                    func,
                    eligible_files.as_deref(),
                    previous_shards.as_ref(),
                    &target_path,
                    logger,
                )
                .await?
            }
        };

        let total_files: usize = shards.iter().map(|s| s._meta_files.len()).sum();
        slog!(
            logger,
            info,
            "Evaluated {} files into {} shards",
            total_files,
            shards.len()
        );

        // Write shard results to workflow state via StateDiff
        let shard_value = serde_json::to_value(&shards)
            .map_err(|e| Error::Runtime(format!("Failed to serialize shard results: {e}")))?;

        let mut fields = HashMap::new();
        fields.insert(
            shard_config.output_state.clone(),
            FieldDiff {
                operation: DiffOperation::Update,
                value: Some(shard_value),
            },
        );

        self.state_adapter
            .lock()
            .await
            .apply_state_diff(&StateDiff {
                workflow_run_id: task.workflow_run_id,
                fields,
            })
            .await?;

        Ok(())
    }

    /// Dry-run the js-ast-grep step via `execute_js_ast_grep_step` to find which files
    /// would be modified. Returns relative file paths (relative to target_path).
    async fn scan_eligible_files_with_jssg(
        &self,
        shard_config: &butterflow_models::step::UseShard,
        js_ast_grep: &butterflow_models::step::UseJSAstGrep,
        target_path: &Path,
        logger: &StructuredLogger,
    ) -> Result<Vec<String>> {
        // Clone the config and force dry_run mode
        let mut dry_run_config = js_ast_grep.clone();
        dry_run_config.dry_run = Some(true);

        // Override base_path with the shard target so the executor scans the right directory
        if let Some(target) = &shard_config.target {
            dry_run_config.base_path = Some(target.clone());
        }

        let collector: Arc<std::sync::Mutex<Vec<PathBuf>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let capabilities = self
            .workflow_run_config
            .capabilities
            .as_ref()
            .map(|v| v.clone().into_iter().collect());

        self.execute_js_ast_grep_step(
            "shard-scan".to_string(),
            "shard-scan".to_string(),
            &dry_run_config,
            None,
            None,
            &CapabilitiesData {
                capabilities,
                capabilities_security_callback: self
                    .workflow_run_config
                    .capabilities_security_callback
                    .as_ref()
                    .map(|callback| callback.clone()),
            },
            &None,
            None,
            None,
            logger,
            Some(collector.clone()),
            None,
        )
        .await?;

        let modified_paths = Arc::try_unwrap(collector)
            .map(|mutex| mutex.into_inner().unwrap())
            .unwrap_or_else(|arc| arc.lock().unwrap().clone());

        let eligible: Vec<String> = modified_paths
            .into_iter()
            .filter_map(|p| {
                p.strip_prefix(target_path)
                    .ok()
                    .map(|rel| rel.to_string_lossy().to_string())
            })
            .collect();

        slog!(
            logger,
            info,
            "Found {} eligible files for sharding",
            eligible.len()
        );

        Ok(eligible)
    }

    /// Execute a custom shard function using the jssg engine (QuickJS).
    ///
    /// The user's script exports a default function that receives a typed `ShardInput`
    /// and returns `ShardResult[]`. The engine handles all file I/O and serialization.
    async fn execute_custom_shard_function(
        &self,
        shard_config: &butterflow_models::step::UseShard,
        func: &butterflow_models::step::CustomShardFunction,
        eligible_files: Option<&[String]>,
        previous_shards: Option<&Vec<serde_json::Value>>,
        target_path: &Path,
        logger: &StructuredLogger,
    ) -> Result<Vec<crate::shard::ShardResult>> {
        use crate::shard::collect_files_with_pattern;
        use codemod_sandbox::sandbox::engine::execution_engine::{
            execute_shard_function_with_quickjs, ShardFunctionOptions,
        };
        use codemod_sandbox::sandbox::resolvers::OxcResolver;
        use codemod_sandbox::utils::project_discovery::find_tsconfig;

        let effective_bundle_path = &self.workflow_run_config.bundle_path;
        let func_path = effective_bundle_path.join(&func.function);

        if !func_path.exists() {
            return Err(Error::Runtime(format!(
                "Custom shard function '{}' does not exist",
                func_path.display()
            )));
        }

        // Collect the file list to pass to the function
        let files: Vec<String> = if let Some(eligible) = eligible_files {
            eligible.to_vec()
        } else if let Some(file_pattern) = &shard_config.file_pattern {
            let target = shard_config.target.as_deref().unwrap_or(".");
            let search_base = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                target_path.join(target)
            };
            let found = collect_files_with_pattern(&search_base, file_pattern)
                .map_err(|e| Error::Runtime(format!("Failed to collect files: {e}")))?;
            found
                .iter()
                .filter_map(|f| {
                    f.strip_prefix(target_path)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                })
                .collect()
        } else {
            return Err(Error::Runtime(
                "Custom shard function requires either 'file_pattern' or 'js-ast-grep' to collect files".to_string(),
            ));
        };

        slog!(
            logger,
            info,
            "Running custom shard function with {} files...",
            files.len()
        );

        let input = serde_json::json!({
            "files": files,
            "targetDir": target_path.to_string_lossy(),
            "previousShards": previous_shards.unwrap_or(&Vec::new()),
        });

        let script_base_dir = func_path.parent().unwrap_or(Path::new(".")).to_path_buf();

        let tsconfig_path = find_tsconfig(&script_base_dir);
        let resolver = Arc::new(
            OxcResolver::new(script_base_dir, tsconfig_path).map_err(|e| {
                Error::Runtime(format!("Failed to create resolver for shard function: {e}"))
            })?,
        );

        let options = ShardFunctionOptions {
            script_path: &func_path,
            resolver,
            input,
            capabilities: self.workflow_run_config.capabilities.clone(),
        };

        let result = execute_shard_function_with_quickjs(options)
            .await
            .map_err(|e| Error::Runtime(format!("Shard function execution failed: {e}")))?;

        let shards: Vec<crate::shard::ShardResult> = serde_json::from_value(result)
            .map_err(|e| Error::Runtime(format!("Failed to parse shard function output: {e}")))?;

        Ok(shards)
    }

    /// Execute a single RunScript step
    #[allow(clippy::too_many_arguments)]
    async fn execute_run_script_step(
        &self,
        runner: &dyn Runner,
        run: &str,
        step_name: &str,
        step_env: &Option<HashMap<String, String>>,
        step_id: &Option<String>,
        node: &Node,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        bundle_path: &Option<PathBuf>,
        logger: &StructuredLogger,
    ) -> Result<()> {
        // Resolve variables
        // TODO: Load step outputs from STEP_OUTPUTS file and pass here
        let resolved_command = resolve_string_with_expression(
            run,
            params,
            state,
            task.matrix_values.as_ref(),
            None,
            None,
        )?;
        let request = ShellCommandExecutionRequest {
            command: resolved_command.clone(),
            node_id: node.id.clone(),
            node_name: node.name.clone(),
            step_id: step_id.clone(),
            step_name: step_name.to_string(),
            task_id: task.id.to_string(),
        };
        let notice = format_shell_command_notice(&request);
        logger.log("info", &format_shell_command_log_notice(&request));

        if self
            .workflow_run_config
            .shell_command_approval_callback
            .is_none()
            && !logger.is_jsonl()
            && !self.workflow_run_config.quiet
        {
            eprintln!();
            eprintln!("{notice}");
            eprintln!();
        }

        if let Some(approval_callback) = &self.workflow_run_config.shell_command_approval_callback {
            let approved = approval_callback(&request).map_err(|error| {
                Error::StepExecution(format!(
                    "Failed to confirm shell command execution for step '{}': {error}",
                    step_name
                ))
            })?;

            if !approved {
                let message = format!(
                    "Shell command execution was declined by the user for step '{}'",
                    step_name
                );
                logger.log("warn", &message);
                return Err(Error::StepExecution(message));
            }
        }

        let prepared = self.prepare_step_execution(step_env, node, task, state, bundle_path)?;

        let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let state_adapter = Arc::clone(&self.state_adapter);
        let task_id = task.id;
        let log_persist_task = tokio::spawn(async move {
            while let Some(line) = log_rx.recv().await {
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                if line.is_empty() {
                    continue;
                }

                let mut adapter = state_adapter.lock().await;
                let Ok(mut current_task) = adapter.get_task(task_id).await else {
                    continue;
                };
                current_task.logs.push(line);
                let _ = adapter.save_task(&current_task).await;
            }
        });

        let output_callback: OutputCallback = Arc::new(move |line: String| {
            let _ = log_tx.send(line);
        });

        let output = runner
            .run_command(
                &resolved_command,
                &prepared.env,
                Some(Arc::clone(&output_callback)),
            )
            .await;
        drop(output_callback);
        let _ = log_persist_task.await;

        self.finalize_step_execution(task, output?, prepared).await
    }

    fn prepare_step_execution(
        &self,
        step_env: &Option<HashMap<String, String>>,
        node: &Node,
        task: &Task,
        state: &HashMap<String, serde_json::Value>,
        bundle_path: &Option<PathBuf>,
    ) -> Result<PreparedStepExecution> {
        // Start with a copy of the parent process's environment
        let mut env: HashMap<String, String> = std::env::vars().collect();

        // Set npm_config_yes for non-interactive mode (auto-accept package installations)
        if self.workflow_run_config.no_interactive {
            env.insert("npm_config_yes".to_string(), "true".to_string());
        }

        // Add node environment variables
        for (key, value) in &node.env {
            env.insert(key.clone(), value.clone());
        }

        // Add state variables
        for (key, value) in state {
            env.insert(
                format!("CODEMOD_STATE_{}", key.to_uppercase()),
                serde_json::to_string(value).unwrap_or("".to_string()),
            );
        }

        // Add step environment variables
        if let Some(step_env) = step_env {
            for (key, value) in step_env {
                env.insert(key.clone(), value.clone());
            }
        }

        // Add matrix values
        if let Some(matrix_values) = &task.matrix_values {
            for (key, value) in matrix_values {
                env.insert(
                    key.clone(),
                    serde_json::to_string(value).unwrap_or(value.to_string()),
                );
            }
        }

        // Add temp file var for step outputs
        let temp_dir = std::env::temp_dir();
        let state_outputs_path = temp_dir.join(task.id.to_string());
        File::create(&state_outputs_path)?;

        // Write state to a temp file and pass its path (env vars have OS size limits)
        let state_input_path = temp_dir.join(format!("{}-state-input", task.id));
        std::fs::write(
            &state_input_path,
            serde_json::to_string(state).unwrap_or_else(|_| "{}".to_string()),
        )?;
        env.insert(
            String::from("CODEMOD_STATE"),
            state_input_path
                .canonicalize()?
                .to_str()
                .expect("File path should be valid UTF-8")
                .to_string(),
        );

        if let Some(bundle_path) = bundle_path {
            env.insert(
                String::from("CODEMOD_PATH"),
                bundle_path.to_str().unwrap_or("").to_string(),
            );
        }

        env.insert(
            String::from("STATE_OUTPUTS"),
            state_outputs_path
                .canonicalize()?
                .to_str()
                .expect("File path should be valid UTF-8")
                .to_string(),
        );

        // Add task and workflow run IDs
        env.insert(String::from("CODEMOD_TASK_ID"), task.id.to_string());

        env.insert(
            String::from("CODEMOD_WORKFLOW_RUN_ID"),
            task.workflow_run_id.to_string(),
        );

        Ok(PreparedStepExecution {
            env,
            state_outputs_path,
            state_input_path,
        })
    }

    async fn finalize_step_execution(
        &self,
        task: &Task,
        _output: String,
        prepared: PreparedStepExecution,
    ) -> Result<()> {
        let outputs = read_to_string(&prepared.state_outputs_path).await?;

        // Clean up the temporary files
        std::fs::remove_file(&prepared.state_outputs_path).ok();
        std::fs::remove_file(&prepared.state_input_path).ok();

        // Update state
        let mut state_diff = HashMap::new();
        for line in outputs.lines() {
            // Check for empty lines
            if line.trim().is_empty() {
                continue;
            }

            // Determine if this is an append operation (@=) or a regular assignment (=)
            let (key, operation, value_str) = if let Some((k, v)) = line.split_once("@=") {
                serde_json::from_str::<serde_json::Value>(v)
                    .unwrap_or(serde_json::Value::String(v.to_string()));
                (k, DiffOperation::Append, v)
            } else if let Some((k, v)) = line.split_once('=') {
                serde_json::from_str::<serde_json::Value>(v)
                    .unwrap_or(serde_json::Value::String(v.to_string()));
                (k, DiffOperation::Update, v)
            } else {
                // Malformed line, log and skip
                warn!("Malformed state output line: {line}");
                continue;
            };

            // Try to parse value as JSON first, fall back to string if that fails
            let value = match serde_json::from_str::<serde_json::Value>(value_str) {
                Ok(json_value) => json_value,
                Err(_) => {
                    // Not valid JSON, treat as a plain string
                    serde_json::Value::String(value_str.to_string())
                }
            };

            // Add to state diff
            state_diff.insert(
                key.to_string(),
                FieldDiff {
                    operation,
                    value: Some(value),
                },
            );
        }

        if !self.workflow_run_config.skip_state_writes {
            self.state_adapter
                .lock()
                .await
                .apply_state_diff(&StateDiff {
                    workflow_run_id: task.workflow_run_id,
                    fields: state_diff,
                })
                .await?;
        }
        Ok(())
    }

    /// Update the status of a matrix master task
    async fn update_matrix_master_status(&self, master_task_id: Uuid) -> Result<()> {
        // Get the master task
        let master_task = self
            .state_adapter
            .lock()
            .await
            .get_task(master_task_id)
            .await?;

        // Get all child tasks
        let tasks = self
            .state_adapter
            .lock()
            .await
            .get_tasks(master_task.workflow_run_id)
            .await?;
        let child_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.master_task_id == Some(master_task_id))
            .collect();

        // If there are no child tasks: either shard state is not ready yet, or the matrix is
        // genuinely empty. Do not complete the master while `depends_on` nodes are still
        // running — otherwise the UI shows the matrix node "done" during shard evaluation.
        if child_tasks.is_empty() {
            let workflow_run = self.get_workflow_run(master_task.workflow_run_id).await?;
            if !workflow_node_dependencies_satisfied(
                &workflow_run.workflow,
                &tasks,
                &master_task.node_id,
            ) {
                debug!(
                    "No child tasks for matrix master {master_task_id} (node {}); dependencies not finished — leaving status as {:?}",
                    master_task.node_id, master_task.status
                );
                return Ok(());
            }

            debug!("No child tasks for master task {master_task_id} after dependencies satisfied; treating as empty matrix.");
            let final_status = TaskStatus::Completed;

            let mut fields = HashMap::new();
            fields.insert(
                "status".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(final_status)?),
                },
            );
            // Add ended_at if moving to Completed/Failed
            if final_status == TaskStatus::Completed || final_status == TaskStatus::Failed {
                fields.insert(
                    "ended_at".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::to_value(Utc::now())?),
                    },
                );
            }
            let task_diff = TaskDiff {
                task_id: master_task_id,
                fields,
            };
            self.state_adapter
                .lock()
                .await
                .apply_task_diff(&task_diff)
                .await?;
            return Ok(());
        }

        // Check status based on existing children
        let all_terminal = child_tasks.iter().all(|t| {
            t.status == TaskStatus::Completed
                || t.status == TaskStatus::Failed
                || t.status == TaskStatus::WontDo
        });

        // If all children are in a terminal state, determine the final master status
        if all_terminal {
            let any_failed = child_tasks.iter().any(|t| t.status == TaskStatus::Failed);
            // Consider WontDo: If some are WontDo and others Completed, is master Completed or Failed?
            // Let's say Failed if any child failed, otherwise Completed (even if some are WontDo).
            let final_status = if any_failed {
                TaskStatus::Failed
            } else {
                TaskStatus::Completed
            };

            debug!(
                "All child tasks for master {master_task_id} are terminal. Setting master status to: {final_status:?}"
            );

            let mut fields = HashMap::new();
            fields.insert(
                "status".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(final_status)?),
                },
            );
            fields.insert(
                "ended_at".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(Utc::now())?),
                },
            );
            let task_diff = TaskDiff {
                task_id: master_task_id,
                fields,
            };
            self.state_adapter
                .lock()
                .await
                .apply_task_diff(&task_diff)
                .await?;
            return Ok(());
        }

        // If not all children are terminal, determine intermediate status
        let any_failed = child_tasks.iter().any(|t| t.status == TaskStatus::Failed);
        let any_running = child_tasks.iter().any(|t| t.status == TaskStatus::Running);
        let any_awaiting = child_tasks
            .iter()
            .any(|t| t.status == TaskStatus::AwaitingTrigger);
        let any_pending = child_tasks.iter().any(|t| t.status == TaskStatus::Pending); // Added check for pending

        // Create a task diff to update the status
        let mut fields = HashMap::new();

        // Determine the new status based on priority: Failed > Awaiting > Running > Pending
        let new_status = if any_failed {
            TaskStatus::Failed
        } else if any_awaiting {
            TaskStatus::AwaitingTrigger
        } else if any_running {
            TaskStatus::Running
        } else if any_pending {
            TaskStatus::Pending // If some are pending and others completed/wontdo, master is still pending/running implicitly
        } else {
            // This case should ideally be covered by the 'all_terminal' check above,
            // but as a fallback, keep the current status.
            master_task.status
        };

        // Only apply diff if the status is actually changing
        if new_status != master_task.status {
            debug!(
                "Updating master task {} status from {:?} to {:?}",
                master_task_id, master_task.status, new_status
            );
            fields.insert(
                "status".to_string(),
                FieldDiff {
                    operation: DiffOperation::Update,
                    value: Some(serde_json::to_value(new_status)?),
                },
            );

            if matches!(
                new_status,
                TaskStatus::Pending | TaskStatus::Running | TaskStatus::AwaitingTrigger
            ) {
                fields.insert(
                    "ended_at".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::Value::Null),
                    },
                );
            } else if matches!(new_status, TaskStatus::Completed | TaskStatus::Failed) {
                fields.insert(
                    "ended_at".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update,
                        value: Some(serde_json::to_value(Utc::now())?),
                    },
                );
            }

            let task_diff = TaskDiff {
                task_id: master_task_id,
                fields,
            };

            // Apply the diff
            self.state_adapter
                .lock()
                .await
                .apply_task_diff(&task_diff)
                .await?;
        } else {
            debug!("Master task {master_task_id} status {new_status:?} remains unchanged.");
        }

        Ok(())
    }
}

impl Clone for Engine {
    fn clone(&self) -> Self {
        Self {
            state_adapter: Arc::clone(&self.state_adapter),
            scheduler: Scheduler::new(),
            workflow_run_config: self.workflow_run_config.clone(),
            execution_stats: Arc::clone(&self.execution_stats),
            metrics_context: self.metrics_context.clone(),
            file_writer: Arc::clone(&self.file_writer),
            task_completion_notify: Arc::clone(&self.task_completion_notify),
            structured_logger: self.structured_logger.clone(),
            output_heartbeat_callbacks: Arc::clone(&self.output_heartbeat_callbacks),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn unset(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                std::env::set_var(self.key, original);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn js_ast_grep_idle_timeout_uses_default_and_respects_env_override() {
        let _guard = EnvVarGuard::unset("CODEMOD_JS_AST_GREP_IDLE_TIMEOUT_MS");
        assert_eq!(
            js_ast_grep_idle_timeout(),
            Duration::from_millis(JS_AST_GREP_IDLE_TIMEOUT_MS_DEFAULT)
        );

        std::env::set_var("CODEMOD_JS_AST_GREP_IDLE_TIMEOUT_MS", "1234");
        assert_eq!(js_ast_grep_idle_timeout(), Duration::from_millis(1234));
    }

    #[test]
    fn record_unit_progress_updates_global_and_active_units() {
        let state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
        let before = state.lock().unwrap().global_last_progress_at;

        std::thread::sleep(Duration::from_millis(5));
        record_unit_progress(&state, "src/example.ts", StepPhase::ExecutionStarted);

        let snapshot = state.lock().unwrap();
        assert_eq!(snapshot.global_phase, StepPhase::ExecutionStarted);
        assert!(snapshot.global_last_progress_at > before);
        let unit = snapshot.active_units.get("src/example.ts").unwrap();
        assert_eq!(unit.phase, StepPhase::ExecutionStarted);
        assert!(unit.last_progress_at > before);
        assert!(snapshot.output_active_units.contains("src/example.ts"));
    }

    #[test]
    fn record_output_progress_refreshes_executing_units() {
        let state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
        record_unit_progress(&state, "src/example.ts", StepPhase::ExecutionStarted);
        let before = state
            .lock()
            .unwrap()
            .active_units
            .get("src/example.ts")
            .unwrap()
            .last_progress_at;

        std::thread::sleep(Duration::from_millis(5));
        record_output_progress(&state);

        let snapshot = state.lock().unwrap();
        assert_eq!(snapshot.global_phase, StepPhase::Output);
        let unit = snapshot.active_units.get("src/example.ts").unwrap();
        assert_eq!(unit.phase, StepPhase::Output);
        assert!(unit.last_progress_at > before);
    }

    #[test]
    fn finish_unit_progress_removes_active_unit() {
        let state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
        record_unit_progress(&state, "src/example.ts", StepPhase::ExecutionStarted);
        finish_unit_progress(&state, "src/example.ts", StepPhase::ExecutionFinished);

        let snapshot = state.lock().unwrap();
        assert_eq!(snapshot.global_phase, StepPhase::ExecutionFinished);
        assert!(!snapshot.active_units.contains_key("src/example.ts"));
        assert!(!snapshot.output_active_units.contains("src/example.ts"));
    }

    #[test]
    fn build_idle_timeout_message_uses_stalest_active_unit() {
        let now = Instant::now();
        let mut state = StepProgressState::new();
        state.global_last_progress_at = now - Duration::from_secs(90);
        state.global_phase = StepPhase::Output;
        state.active_units.insert(
            "src/fresh.ts".to_string(),
            UnitProgressState {
                last_progress_at: now - Duration::from_secs(10),
                phase: StepPhase::Output,
            },
        );
        state.active_units.insert(
            "src/stale.ts".to_string(),
            UnitProgressState {
                last_progress_at: now - Duration::from_secs(75),
                phase: StepPhase::ExecutionStarted,
            },
        );

        let message = build_js_ast_grep_idle_timeout_message(&state, Duration::from_secs(60));
        assert!(message.contains("src/stale.ts"));
        assert!(message.contains("execution started"));
        assert!(message.contains("active units: 2"));
    }

    #[tokio::test]
    async fn await_js_ast_grep_execution_task_returns_idle_timeout_error() {
        let progress_state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
        record_unit_progress(
            &progress_state,
            "src/stalled.ts",
            StepPhase::ExecutionStarted,
        );
        let idle_timed_out = Arc::new(AtomicBool::new(false));
        let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));

        let local = tokio::task::LocalSet::new();
        let idle_timed_out_for_task = Arc::clone(&idle_timed_out);
        let idle_failure_message_for_task = Arc::clone(&idle_failure_message);
        let progress_state_for_task = Arc::clone(&progress_state);
        let result = local
            .run_until(async move {
                let trigger = tokio::spawn({
                    let idle_timed_out = Arc::clone(&idle_timed_out_for_task);
                    let idle_failure_message = Arc::clone(&idle_failure_message_for_task);
                    async move {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        idle_timed_out.store(true, Ordering::Release);
                        if let Ok(mut message) = idle_failure_message.lock() {
                            *message = Some(
                                "No progress observed for 1s while processing src/stalled.ts (execution started, active units: 1)"
                                    .to_string(),
                            );
                        }
                    }
                });

                let execution_task = tokio::task::spawn_local(async move {
                    futures_util::future::pending::<
                        std::result::Result<
                            CodemodOutput,
                            codemod_sandbox::sandbox::errors::ExecutionError,
                        >,
                    >()
                    .await
                });

                let result = await_js_ast_grep_execution_task(
                    execution_task,
                    idle_timed_out_for_task,
                    idle_failure_message_for_task,
                    progress_state_for_task,
                    Duration::from_secs(1),
                    "src/stalled.ts",
                )
                .await;
                trigger.await.unwrap();
                result
            })
            .await;

        let error = result.expect_err("pending execution should time out");
        let message = error.to_string();
        assert!(message.contains("No progress observed"));
        assert!(message.contains("src/stalled.ts"));
    }

    /// Matrix master with `from_state` has no child tasks until state exists. While dependency
    /// nodes are not fully completed, the master must not be marked completed (regression guard).
    #[tokio::test]
    async fn update_matrix_master_with_no_children_skips_completion_until_depends_on_complete() {
        use butterflow_models::node::NodeType;
        use butterflow_models::runtime::{Runtime, RuntimeType};
        use butterflow_models::strategy::StrategyType;
        use butterflow_models::{Step, WorkflowState};
        use butterflow_state::mock_adapter::MockStateAdapter;

        let workflow_run_id = Uuid::new_v4();
        let workflow = Workflow {
            version: "1".to_string(),
            params: None,
            state: Some(WorkflowState::default()),
            templates: vec![],
            nodes: vec![
                Node {
                    id: "node1".to_string(),
                    name: "Node 1".to_string(),
                    description: None,
                    r#type: NodeType::Automatic,
                    depends_on: vec![],
                    trigger: None,
                    strategy: None,
                    runtime: Some(Runtime {
                        r#type: RuntimeType::Direct,
                        image: None,
                        working_dir: None,
                        user: None,
                        network: None,
                        options: None,
                    }),
                    steps: vec![Step {
                        id: Some("step-1".to_string()),
                        name: "Step 1".to_string(),
                        action: StepAction::RunScript("true".to_string()),
                        env: None,
                        condition: None,
                        commit: None,
                    }],
                    env: HashMap::new(),
                    branch_name: None,
                    pull_request: None,
                },
                Node {
                    id: "node2".to_string(),
                    name: "Node 2".to_string(),
                    description: None,
                    r#type: NodeType::Automatic,
                    depends_on: vec!["node1".to_string()],
                    trigger: None,
                    strategy: Some(Strategy {
                        r#type: StrategyType::Matrix,
                        values: None,
                        from_state: Some("files".to_string()),
                    }),
                    runtime: Some(Runtime {
                        r#type: RuntimeType::Direct,
                        image: None,
                        working_dir: None,
                        user: None,
                        network: None,
                        options: None,
                    }),
                    steps: vec![Step {
                        id: Some("step-2".to_string()),
                        name: "Step 1".to_string(),
                        action: StepAction::RunScript("true".to_string()),
                        env: None,
                        condition: None,
                        commit: None,
                    }],
                    env: HashMap::new(),
                    branch_name: None,
                    pull_request: None,
                },
            ],
        };

        let workflow_run = WorkflowRun {
            id: workflow_run_id,
            workflow,
            status: WorkflowStatus::Running,
            params: HashMap::new(),
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            bundle_path: None,
            capabilities: None,
            name: None,
            target_path: None,
        };

        let engine = Engine::with_state_adapter(
            Box::new(MockStateAdapter::new()),
            WorkflowRunConfig::default(),
        );

        engine
            .state_adapter
            .lock()
            .await
            .save_workflow_run(&workflow_run)
            .await
            .unwrap();

        let node1_task = Task::new(workflow_run_id, "node1".to_string(), false);
        let master_task = Task::new(workflow_run_id, "node2".to_string(), true);

        engine
            .state_adapter
            .lock()
            .await
            .save_task(&node1_task)
            .await
            .unwrap();
        engine
            .state_adapter
            .lock()
            .await
            .save_task(&master_task)
            .await
            .unwrap();

        engine
            .update_matrix_master_status(master_task.id)
            .await
            .unwrap();

        let master_after = engine
            .state_adapter
            .lock()
            .await
            .get_task(master_task.id)
            .await
            .unwrap();
        assert_eq!(
            master_after.status,
            TaskStatus::Pending,
            "master must stay non-terminal while dependency node tasks are not all completed"
        );
    }
}
