use butterflow_models::schema::resolve_values_with_default;
use codemod_ai::execute::{execute_ai_step, ExecuteAiStepConfig};
use futures_util::FutureExt;
use std::any::Any;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::io::{BufRead, BufReader};
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
    CapabilitiesSecurityCallback, InstallSkillExecutionRequest, InstallSkillExecutor,
    ShellCommandExecutionRequest, WorkflowRunConfig,
};
use crate::execution::{CodemodExecutionConfig, ProgressCallback};
use crate::execution_stats::ExecutionStats;
use crate::file_ops::AsyncFileWriter;
use crate::jssg_execution_service::{JssgExecutionRequest, JssgExecutionService};
use crate::managed_git_service::{ManagedGitService, WorktreeCleanup};
use crate::nested_codemod_service::NestedCodemodService;
use crate::progress_output::{
    append_buffered_diagnostic, append_buffered_log, flush_buffered_execution_output,
    BufferedExecutionOutput,
};
use crate::slog;
use crate::structured_log::{StdoutCaptureGuard, StepContext, StructuredLogger};
use crate::task_state_service::TaskStateService;
use crate::utils::validate_workflow;
use crate::workflow_runtime::{publish_event, WorkflowEvent};
use chrono::Utc;
use codemod_sandbox::sandbox::engine::CodemodOutput;
use codemod_sandbox::sandbox::runtime_module::{
    RuntimeEvent, RuntimeEventKind, RuntimeFailure, RuntimeFailureKind,
};
use codemod_sandbox::{scan_file_with_combined_scan, with_combined_scan};
use log::debug;
use std::path::Path;
use tokio::fs::read_to_string;
use tokio::sync::Mutex;
use tokio::time;
use uuid::Uuid;

use crate::registry::ResolvedPackage;
use crate::step_executor::{StepExecutionRequest, StepExecutor};
use butterflow_models::runtime::RuntimeType;

use butterflow_models::step::{UseAI, UseAstGrep, UseCodemod, UseJSAstGrep};
use butterflow_models::{
    evaluate_condition, resolve_string_list, resolve_string_with_expression, resolve_usize_value,
    DiffOperation, Error, FieldDiff, Node, Result, StateDiff, Strategy, Task, TaskErrorDetails,
    TaskExpressionContext, TaskStatus, Workflow, WorkflowRun, WorkflowStatus,
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
use codemod_sandbox::MetricsContext;

/// Guard that ensures task completion notification is sent even on panic/timeout
struct TaskCleanupGuard {
    notify: Arc<Notify>,
    sent: bool,
}

fn panic_payload_message(panic_payload: &(dyn Any + Send)) -> String {
    if let Some(message) = panic_payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic_payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "task thread panicked".to_string()
    }
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
            debug!("Sending task completion notification from cleanup guard");
            self.notify.notify_one();
        }
    }
}

pub(crate) struct ResolvedPullRequestConfig {
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) draft: bool,
    pub(crate) base: Option<String>,
    pub(crate) branch: String,
}

pub(crate) const PULL_REQUEST_METADATA_LOG_PREFIX: &str = "Pull request metadata: ";

pub(crate) fn pull_request_metadata_log_line(pr: &ResolvedPullRequestConfig) -> String {
    format!(
        "{PULL_REQUEST_METADATA_LOG_PREFIX}{}",
        serde_json::json!({
            "title": pr.title,
            "draft": pr.draft,
            "base": pr.base,
            "branch": pr.branch,
        })
    )
}

pub(crate) fn resolve_workflow_run_params(
    workflow_run: &WorkflowRun,
) -> HashMap<String, serde_json::Value> {
    workflow_run
        .workflow
        .params
        .as_ref()
        .map(|p| resolve_values_with_default(&p.schema, &workflow_run.params))
        .unwrap_or_else(|| workflow_run.params.clone())
}

pub(crate) fn block_on_runtime_handle<F>(handle: &tokio::runtime::Handle, future: F) -> F::Output
where
    F: Future,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        handle.block_on(future)
    }
}

pub(crate) async fn execute_install_skill_in_isolated_runtime(
    executor: Arc<dyn InstallSkillExecutor>,
    request: InstallSkillExecutionRequest,
) -> std::result::Result<String, anyhow::Error> {
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| anyhow::anyhow!("failed to build install-skill runtime: {error}"))?;
        rt.block_on(executor.execute(request))
    })
    .await
    .unwrap_or_else(|error| {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        Err(anyhow::anyhow!(
            "install-skill executor task failed to join: {error}"
        ))
    })
}

fn task_error_details(error: &Error) -> Option<TaskErrorDetails> {
    match error {
        Error::ShellCommandStepFailed {
            command,
            exit_code,
            output,
        } => Some(TaskErrorDetails::ShellCommand {
            command: command.clone(),
            exit_code: *exit_code,
            output: output.clone(),
        }),
        Error::AstGrepStepFailed { message, help } => Some(TaskErrorDetails::AstGrep {
            message: message.clone(),
            help: help.clone(),
        }),
        _ => None,
    }
}

fn ast_grep_step_help() -> Option<String> {
    Some("Fix the ast-grep rule or target file issue and rerun the workflow.".to_string())
}

pub(crate) struct PreparedStepExecution {
    pub(crate) env: HashMap<String, String>,
    pub(crate) state_outputs_path: PathBuf,
    pub(crate) state_input_path: PathBuf,
}

pub const JS_AST_GREP_IDLE_TIMEOUT_MS_DEFAULT: u64 = 60_000;

type ProgressHeartbeatCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepPhase {
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
pub struct UnitProgressState {
    pub last_progress_at: Instant,
    pub phase: StepPhase,
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
pub struct StepProgressState {
    pub global_last_progress_at: Instant,
    pub global_phase: StepPhase,
    pub active_units: HashMap<String, UnitProgressState>,
    pub output_active_units: HashSet<String>,
}

impl Default for StepProgressState {
    fn default() -> Self {
        Self::new()
    }
}

impl StepProgressState {
    pub fn new() -> Self {
        Self {
            global_last_progress_at: Instant::now(),
            global_phase: StepPhase::Starting,
            active_units: HashMap::new(),
            output_active_units: HashSet::new(),
        }
    }
}

pub fn js_ast_grep_idle_timeout() -> Duration {
    let override_ms = std::env::var("CODEMOD_JS_AST_GREP_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0);
    Duration::from_millis(override_ms.unwrap_or(JS_AST_GREP_IDLE_TIMEOUT_MS_DEFAULT))
}

pub fn select_shard_scan_eligible_files(
    modified_files: Vec<String>,
    selector_matched_files: Vec<String>,
) -> Vec<String> {
    if modified_files.is_empty() {
        selector_matched_files
    } else {
        modified_files
    }
}

pub(crate) fn should_manage_git_for_node(node: &Node, enable_managed_git: bool) -> bool {
    crate::git_ops::is_cloud_mode()
        || (enable_managed_git && (node.pull_request.is_some() || node.branch_name.is_some()))
}

pub fn record_unit_progress(
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

pub fn record_output_progress(state: &Arc<std::sync::Mutex<StepProgressState>>) {
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

pub fn finish_unit_progress(
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

pub fn build_js_ast_grep_idle_timeout_message(
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

pub(crate) fn format_runtime_event_log(event: &RuntimeEvent) -> Option<String> {
    if event.meta.as_deref() == Some("console") {
        return Some(event.message.clone());
    }

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

pub(crate) fn format_runtime_failure_message(failure: &RuntimeFailure) -> String {
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

pub async fn await_js_ast_grep_execution_task(
    execution_task: tokio::task::JoinHandle<
        std::result::Result<CodemodOutput, codemod_sandbox::sandbox::errors::ExecutionError>,
    >,
    idle_timed_out: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
    idle_failure_message: Arc<std::sync::Mutex<Option<String>>>,
    progress_state: Arc<std::sync::Mutex<StepProgressState>>,
    idle_timeout: Duration,
    relative_path: &str,
) -> Result<std::result::Result<CodemodOutput, codemod_sandbox::sandbox::errors::ExecutionError>> {
    let mut execution_task = std::pin::pin!(execution_task);
    let idle_signal = async {
        let notified = idle_notify.notified();
        tokio::pin!(notified);
        // Register the waker before checking the flag to avoid a missed wakeup
        // if the watchdog flips the flag between the load and the await.
        notified.as_mut().enable();
        if !idle_timed_out.load(Ordering::Acquire) {
            notified.await;
        }
    };
    tokio::pin!(idle_signal);

    tokio::select! {
        biased;
        result = &mut execution_task => {
            return result
                .map_err(|e| Error::StepExecution(format!("Codemod execution join failed: {e}")));
        }
        _ = &mut idle_signal => {}
    }

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
    Err(Error::Runtime(message))
}

pub(crate) fn log_step_output(logger: &StructuredLogger, output: &str) {
    if !logger.is_jsonl() {
        return;
    }

    for line in output.lines().filter(|line| !line.is_empty()) {
        logger.log("info", line);
    }
}

fn format_agent_stream_line(canonical: &str, stream: &str, line: String) -> String {
    if canonical == "codex" || canonical == "claude-code" {
        line
    } else {
        format!("[{stream}] {line}")
    }
}

fn should_stream_agent_output_live(quiet: bool, logger: &StructuredLogger) -> bool {
    !quiet && !logger.is_jsonl()
}

fn write_agent_stream_line_live(stream: &str, line: &str) {
    use std::io::Write;

    match stream {
        "stderr" => {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{line}");
            let _ = stderr.flush();
        }
        _ => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{line}");
            let _ = stdout.flush();
        }
    }
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
pub(crate) fn resolve_optional_glob_list(
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

/// Look up `_meta_files` from matrix values or workflow state and return it as
/// a list of file paths, if present. Matrix takes precedence over state.
///
/// This enables automatic scoping of ast-grep / js-ast-grep steps to the files
/// produced by a preceding shard step
pub(crate) fn auto_meta_files_include(
    state: &HashMap<String, serde_json::Value>,
    matrix_values: Option<&HashMap<String, serde_json::Value>>,
) -> Option<Vec<String>> {
    if let Some(files) = matrix_values
        .and_then(|m| m.get("_meta_files"))
        .and_then(butterflow_models::variable::value_to_string_vec)
    {
        return Some(files);
    }
    state
        .get("_meta_files")
        .and_then(butterflow_models::variable::value_to_string_vec)
}

/// Workflow engine
pub struct Engine {
    /// State adapter for persisting workflow state
    state_adapter: Arc<Mutex<Box<dyn StateAdapter>>>,

    task_state_service: TaskStateService,

    scheduler: Scheduler,

    workflow_run_config: WorkflowRunConfig,

    pub execution_stats: Arc<ExecutionStats>,

    /// Metrics context for tracking metrics across all JSSG steps
    pub metrics_context: MetricsContext,

    /// Async file writer for batched I/O operations
    file_writer: Arc<AsyncFileWriter>,

    /// Notification for when running tasks complete
    task_completion_notify: Arc<Notify>,

    /// Notification for changes that can make the scheduler's next decision different.
    scheduler_wake_notify: Arc<Notify>,

    /// Structured logger for JSONL output
    pub structured_logger: StructuredLogger,

    /// Optional per-task heartbeat callbacks invoked when captured output arrives.
    output_heartbeat_callbacks: Arc<std::sync::Mutex<HashMap<Uuid, ProgressHeartbeatCallback>>>,

    /// In-process cancel signals for steps that support cooperative cancellation
    /// (today: the js-ast-grep file loop). `cancel_workflow` flips every entry
    /// so the step can short-circuit without polling the state backend.
    step_cancel_signals: Arc<std::sync::Mutex<HashMap<Uuid, Arc<AtomicBool>>>>,
}

/// Represents a codemod dependency chain for cycle detection
#[derive(Debug, Clone)]
pub struct CodemodDependency {
    /// Source identifier (registry package or local path)
    pub(crate) source: String,
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
        let working_dir = &self.workflow_run_config.execution.target_path;
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

        // Only agents whose `build_agent_command` pass the prompt via stdin
        if canonical == "claude-code" || canonical == "codex" || canonical == "opencode" {
            slog!(logger, info, "{} prompt delivery: stdin pipe", canonical);
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::inherit());
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

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
        let stream_live =
            should_stream_agent_output_live(self.workflow_run_config.output.quiet, logger);
        let stdout_reader = child.stdout.take().map(|stdout| {
            let canonical = canonical.to_string();
            let logger = logger.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout)
                    .lines()
                    .map_while(|line: std::io::Result<String>| line.ok())
                    .filter(|line| !line.trim().is_empty())
                {
                    let formatted = format_agent_stream_line(&canonical, "stdout", line);
                    logger.log("info", &formatted);
                    if stream_live {
                        write_agent_stream_line_live("stdout", &formatted);
                    }
                }
            })
        });
        let stderr_reader = child.stderr.take().map(|stderr| {
            let canonical = canonical.to_string();
            let logger = logger.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr)
                    .lines()
                    .map_while(|line: std::io::Result<String>| line.ok())
                    .filter(|line| !line.trim().is_empty())
                {
                    let formatted = format_agent_stream_line(&canonical, "stderr", line);
                    logger.log("info", &formatted);
                    if stream_live {
                        write_agent_stream_line_live("stderr", &formatted);
                    }
                }
            })
        });
        if canonical == "claude-code" || canonical == "codex" || canonical == "opencode" {
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

        if let Some(reader) = stdout_reader {
            let _ = reader.join();
        }
        if let Some(reader) = stderr_reader {
            let _ = reader.join();
        }

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

    pub async fn create_pull_request_for_task(&self, task_id: Uuid) -> Result<Option<String>> {
        ManagedGitService::new(self)
            .create_pull_request_for_task(task_id)
            .await
    }

    fn emit_ai_instructions(
        &self,
        logger: &StructuredLogger,
        system_prompt: Option<&str>,
        resolved_prompt: &str,
    ) {
        if logger.is_jsonl() {
            logger.user_line("[AI INSTRUCTIONS]");
            if let Some(system_prompt) = system_prompt {
                logger.user_line(system_prompt);
            }
            logger.user_line(resolved_prompt);
            logger.user_line("[/AI INSTRUCTIONS]");
        } else if !self.workflow_run_config.output.quiet {
            logger.user_line("");
            logger.user_line("[AI INSTRUCTIONS]");
            logger.user_line("");
            if let Some(system_prompt) = system_prompt {
                logger.user_line(system_prompt);
                logger.user_line("");
            }
            logger.user_line(resolved_prompt);
            logger.user_line("");
            logger.user_line("[/AI INSTRUCTIONS]");
            logger.user_line("");
        }
    }

    /// Create a new engine with a local state adapter
    pub fn new() -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> =
            Arc::new(Mutex::new(Box::new(LocalStateAdapter::new())));
        let scheduler_wake_notify = Arc::new(Notify::new());

        Self {
            state_adapter: Arc::clone(&state_adapter),
            task_state_service: TaskStateService::new(Arc::clone(&state_adapter))
                .with_scheduler_wake_notify(Arc::clone(&scheduler_wake_notify)),
            scheduler: Scheduler::new(),
            workflow_run_config: WorkflowRunConfig::default(),
            execution_stats: Arc::new(ExecutionStats::default()),
            metrics_context: MetricsContext::new(),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
            scheduler_wake_notify,
            structured_logger: StructuredLogger::default()
                .with_text_step_headings(true)
                .with_text_log_fallthrough(true),
            output_heartbeat_callbacks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            step_cancel_signals: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Create a new engine with a local state adapter
    pub fn with_workflow_run_config(workflow_run_config: WorkflowRunConfig) -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> =
            Arc::new(Mutex::new(Box::new(LocalStateAdapter::new())));
        let scheduler_wake_notify = Arc::new(Notify::new());
        let structured_logger = StructuredLogger::new(workflow_run_config.output.output_format)
            .with_text_step_headings(!workflow_run_config.output.quiet)
            .with_text_log_fallthrough(!workflow_run_config.output.quiet);

        Self {
            state_adapter: Arc::clone(&state_adapter),
            task_state_service: TaskStateService::new(Arc::clone(&state_adapter))
                .with_scheduler_wake_notify(Arc::clone(&scheduler_wake_notify)),
            scheduler: Scheduler::new(),
            workflow_run_config,
            execution_stats: Arc::new(ExecutionStats::default()),
            metrics_context: MetricsContext::new(),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
            scheduler_wake_notify,
            structured_logger,
            output_heartbeat_callbacks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            step_cancel_signals: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Get a mutable reference to the workflow run config
    pub fn workflow_run_config_mut(&mut self) -> &mut WorkflowRunConfig {
        &mut self.workflow_run_config
    }

    pub(crate) fn workflow_run_config(&self) -> &WorkflowRunConfig {
        &self.workflow_run_config
    }

    pub(crate) fn state_adapter(&self) -> Arc<Mutex<Box<dyn StateAdapter>>> {
        Arc::clone(&self.state_adapter)
    }

    pub(crate) fn task_state_service(&self) -> TaskStateService {
        self.task_state_service.clone()
    }

    pub(crate) fn file_writer(&self) -> Arc<AsyncFileWriter> {
        Arc::clone(&self.file_writer)
    }

    /// Create a new engine with a custom state adapter
    pub fn with_state_adapter(
        state_adapter: Box<dyn StateAdapter>,
        workflow_run_config: WorkflowRunConfig,
    ) -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> = Arc::new(Mutex::new(state_adapter));
        let scheduler_wake_notify = Arc::new(Notify::new());
        let structured_logger = StructuredLogger::new(workflow_run_config.output.output_format)
            .with_text_step_headings(!workflow_run_config.output.quiet)
            .with_text_log_fallthrough(!workflow_run_config.output.quiet);

        Self {
            state_adapter: Arc::clone(&state_adapter),
            task_state_service: TaskStateService::new(Arc::clone(&state_adapter))
                .with_scheduler_wake_notify(Arc::clone(&scheduler_wake_notify)),
            scheduler: Scheduler::new(),
            workflow_run_config,
            execution_stats: Arc::new(ExecutionStats::default()),
            metrics_context: MetricsContext::new(),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
            scheduler_wake_notify,
            structured_logger,
            output_heartbeat_callbacks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            step_cancel_signals: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Enable or disable quiet mode (suppresses stdout/stderr when TUI is active)
    pub fn set_quiet(&mut self, quiet: bool) {
        self.workflow_run_config.output.quiet = quiet;
        self.structured_logger.set_text_step_headings(!quiet);
        self.structured_logger.set_text_log_fallthrough(!quiet);
    }

    pub fn set_text_log_fallthrough(&mut self, enabled: bool) {
        self.structured_logger.set_text_log_fallthrough(enabled);
    }

    pub(crate) fn emit_error(&self, message: String) {
        slog!(&self.structured_logger, error, "{message}");
    }

    fn emit_workflow_started(&self, workflow_run: &WorkflowRun) {
        publish_event(
            workflow_run.id,
            WorkflowEvent::WorkflowStarted {
                workflow_run: workflow_run.clone(),
                at: Utc::now(),
            },
        );
    }

    async fn emit_task_created(&self, task: &Task) {
        publish_event(
            task.workflow_run_id,
            WorkflowEvent::TaskCreated {
                task: task.clone(),
                at: Utc::now(),
            },
        );
    }

    /// Replace the progress callback used by workflow execution.
    pub fn set_progress_callback(&mut self, progress_callback: Arc<Option<ProgressCallback>>) {
        self.workflow_run_config.execution.progress_callback = progress_callback;
    }

    /// Set the human-readable name for this workflow run
    pub fn set_name(&mut self, name: Option<String>) {
        self.workflow_run_config.output.name = name;
    }

    /// Get the workflow file path
    pub fn get_workflow_file_path(&self) -> PathBuf {
        self.workflow_run_config
            .execution
            .workflow_file_path
            .clone()
    }

    /// Get the target path for this workflow run
    pub fn get_target_path(&self) -> PathBuf {
        self.workflow_run_config.execution.target_path.clone()
    }

    /// Check if the engine is in dry-run mode
    pub fn is_dry_run(&self) -> bool {
        self.workflow_run_config.execution.dry_run
    }

    /// Enable or disable dry-run mode
    pub fn set_dry_run(&mut self, dry_run: bool) {
        self.workflow_run_config.execution.dry_run = dry_run;
    }

    /// Get the current capabilities
    pub fn get_capabilities(&self) -> &Option<HashSet<LlrtSupportedModules>> {
        &self.workflow_run_config.execution.capabilities
    }

    /// Set the capabilities
    pub fn set_capabilities(&mut self, capabilities: Option<HashSet<LlrtSupportedModules>>) {
        self.workflow_run_config.execution.capabilities = capabilities;
    }

    /// Spawn a task asynchronously on a dedicated thread with its own runtime.
    ///
    /// Uses a dedicated multi-thread Tokio runtime per task thread so async
    /// work invoked from worker threads (for example network activity inside
    /// js-ast-grep codemods) can make progress reliably.
    async fn spawn_task_with_handle(&self, task_id: Uuid) -> Result<()> {
        let engine = self.clone();
        let panic_engine = self.clone();
        let task_completion_notify = Arc::clone(&self.task_completion_notify);
        let panic_task_completion_notify = Arc::clone(&self.task_completion_notify);

        let shared_worktree_cleanup: WorktreeCleanup =
            Arc::new(std::sync::Mutex::new(None::<(PathBuf, PathBuf)>));
        let shared_worktree_cleanup_for_task = Arc::clone(&shared_worktree_cleanup);
        let shared_worktree_cleanup_for_panic = Arc::clone(&shared_worktree_cleanup);

        let task_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build task runtime");

            rt.block_on(async move {
                let mut engine = engine;
                let mut cleanup_guard = TaskCleanupGuard::new(task_completion_notify.clone());

                let _ = engine
                    .append_task_log(task_id, "Task execution starting")
                    .await;

                let task = {
                    let adapter = engine.state_adapter.lock().await;
                    adapter.get_task(task_id).await.ok()
                };
                if let Some(task) = task {
                    let workflow_run = {
                        let adapter = engine.state_adapter.lock().await;
                        adapter.get_workflow_run(task.workflow_run_id).await.ok()
                    };
                    if let Some(workflow_run) = workflow_run {
                        if let Some(node) = workflow_run
                            .workflow
                            .nodes
                            .iter()
                            .find(|node| node.id == task.node_id)
                        {
                            if let Err(error) = ManagedGitService::prepare_task_worktree(
                                &mut engine,
                                task_id,
                                &task,
                                &workflow_run,
                                node,
                                &shared_worktree_cleanup_for_task,
                            )
                            .await
                            {
                                let message = error.to_string();
                                let _ = engine.append_task_log(task_id, &message).await;
                                engine.emit_error(format!(
                                    "Failed to prepare git worktree for task {}: {}",
                                    task_id, message
                                ));
                                let _ = engine.mark_task_as_failed(task_id, &message).await;
                                return;
                            }
                        }
                    }
                }

                let task_timeout = tokio::time::Duration::from_secs(45 * 60);
                let _ = engine
                    .append_task_log(task_id, "Pre-execution setup complete")
                    .await;
                let _ = engine
                    .append_task_log(task_id, "Entering execute_task")
                    .await;

                match tokio::time::timeout(task_timeout, engine.execute_task(task_id)).await {
                    Ok(Ok(())) => {
                        slog!(
                            &engine.structured_logger,
                            debug,
                            "Task {} completed successfully",
                            task_id
                        );
                        cleanup_guard.mark_sent();
                    }
                    Ok(Err(Error::Deferred(message))) => {
                        let _ = engine
                            .append_task_log(
                                task_id,
                                format!("Task returned to awaiting trigger: {message}"),
                            )
                            .await;
                        let _ = engine.mark_task_as_awaiting_trigger(task_id).await;
                        cleanup_guard.mark_sent();
                    }
                    Ok(Err(e)) => {
                        let needs_fallback_failure_mark =
                            match engine.state_adapter.lock().await.get_task(task_id).await {
                                Ok(current_task) => !matches!(
                                    current_task.status,
                                    TaskStatus::Failed
                                        | TaskStatus::Completed
                                        | TaskStatus::WontDo
                                        | TaskStatus::AwaitingTrigger
                                ),
                                Err(_) => true,
                            };

                        if needs_fallback_failure_mark {
                            let _ = engine
                                .append_task_log(
                                    task_id,
                                    format!("Task execution failed before completion: {}", e),
                                )
                                .await;
                            let _ = engine.mark_task_as_failed(task_id, &e.to_string()).await;
                        }
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

                ManagedGitService::new(&engine)
                    .cleanup_worktree(task_id, &shared_worktree_cleanup, false)
                    .await;
            });
        });

        std::thread::spawn(move || {
            if let Err(panic_payload) = task_thread.join() {
                let panic_message = panic_payload_message(panic_payload.as_ref());
                let cleanup_rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build panic cleanup runtime");
                cleanup_rt.block_on(async move {
                    let engine = panic_engine;
                    let message = format!("Task thread panicked: {panic_message}");
                    ManagedGitService::new(&engine)
                        .cleanup_worktree(task_id, &shared_worktree_cleanup_for_panic, true)
                        .await;
                    let _ = engine.append_task_log(task_id, &message).await;
                    let _ = engine.mark_task_as_failed(task_id, &message).await;
                    panic_task_completion_notify.notify_one();
                    engine.emit_error(format!("Task {task_id} panicked: {panic_message}"));
                });
            }
        });

        Ok(())
    }

    /// Mark a task as failed due to timeout or other issues
    async fn mark_task_as_failed(&self, task_id: Uuid, error_message: &str) -> Result<()> {
        let task = self
            .task_state_service()
            .mark_failed(task_id, error_message)
            .await?;
        self.update_parent_matrix_master_for_task(&task).await?;

        Ok(())
    }

    async fn mark_task_as_awaiting_trigger(&self, task_id: Uuid) -> Result<()> {
        let task = self
            .task_state_service()
            .mark_awaiting_trigger(task_id)
            .await?;
        self.update_parent_matrix_master_for_task(&task).await?;

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

    pub(crate) async fn append_task_log(
        &self,
        task_id: Uuid,
        message: impl Into<String>,
    ) -> Result<()> {
        self.task_state_service()
            .append_task_log(task_id, message)
            .await
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

    pub(crate) fn register_output_heartbeat(
        &self,
        task_id: Uuid,
        callback: ProgressHeartbeatCallback,
    ) {
        if let Ok(mut callbacks) = self.output_heartbeat_callbacks.lock() {
            callbacks.insert(task_id, callback);
        }
    }

    pub(crate) fn unregister_output_heartbeat(&self, task_id: Uuid) {
        if let Ok(mut callbacks) = self.output_heartbeat_callbacks.lock() {
            callbacks.remove(&task_id);
        }
    }

    pub(crate) fn register_step_cancel_signal(&self, task_id: Uuid, signal: Arc<AtomicBool>) {
        if let Ok(mut signals) = self.step_cancel_signals.lock() {
            signals.insert(task_id, signal);
        }
    }

    pub(crate) fn unregister_step_cancel_signal(&self, task_id: Uuid) {
        if let Ok(mut signals) = self.step_cancel_signals.lock() {
            signals.remove(&task_id);
        }
    }

    /// Flip every registered step cancel signal. Called by `cancel_workflow`
    /// so in-flight cooperative steps (e.g. js-ast-grep) short-circuit without
    /// polling the state backend.
    fn signal_step_cancellation(&self) {
        if let Ok(signals) = self.step_cancel_signals.lock() {
            for signal in signals.values() {
                signal.store(true, Ordering::Release);
            }
        }
    }

    fn wake_scheduler(&self) {
        self.scheduler_wake_notify.notify_waiters();
    }

    async fn update_parent_matrix_master_for_task(&self, task: &Task) -> Result<()> {
        if let Some(master_task_id) = task.master_task_id {
            self.task_state_service()
                .update_matrix_master_status(master_task_id)
                .await?;
        }
        Ok(())
    }

    /// Wait for all currently running tasks to complete.
    async fn wait_for_running_tasks_to_complete(&self, workflow_run_id: Uuid) -> Result<()> {
        loop {
            // Register before reading state so a completion signal cannot be
            // missed between the database check and the await below.
            let notified = self.task_completion_notify.notified();

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
                break;
            }

            slog!(
                &self.structured_logger,
                debug,
                "Waiting for {} running task(s) before scheduling workflow {}",
                running_tasks.len(),
                workflow_run_id
            );

            notified.await;
        }
        Ok(())
    }

    /// Create initial tasks for all nodes
    async fn create_initial_tasks(&self, workflow_run: &WorkflowRun) -> Result<()> {
        let mut tasks = self.scheduler.calculate_initial_tasks(workflow_run).await?;

        // Flatten matrix nodes: replace all master+children with a single
        // regular task per node so it runs exactly once.
        if self.workflow_run_config.execution.flatten_matrix_tasks {
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
            self.wake_scheduler();
            self.emit_task_created(&task).await;

            if task.is_master {
                self.task_state_service()
                    .update_matrix_master_status(task.id)
                    .await?;
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
        self.run_workflow_with_id(Uuid::new_v4(), workflow, params, bundle_path, capabilities)
            .await
    }

    pub async fn run_workflow_with_id(
        &self,
        workflow_run_id: Uuid,
        workflow: Workflow,
        params: HashMap<String, serde_json::Value>,
        bundle_path: Option<PathBuf>,
        capabilities: Option<&HashSet<LlrtSupportedModules>>,
    ) -> Result<Uuid> {
        validate_workflow(&workflow, bundle_path.as_deref().unwrap_or(Path::new("")))?;
        self.validate_codemod_dependencies(&workflow, &[]).await?;

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
            name: self.workflow_run_config.output.name.clone(),
            target_path: Some(self.workflow_run_config.execution.target_path.clone()),
        };

        self.state_adapter
            .lock()
            .await
            .save_workflow_run(&workflow_run)
            .await?;
        self.emit_workflow_started(&workflow_run);

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
        let mut parent_master_ids = HashSet::new();
        let mut triggered_task_ids = Vec::new();
        for task_id in task_ids {
            let task = self.state_adapter.lock().await.get_task(task_id).await?;

            // If the task is awaiting trigger we can trigger it
            // OR if it is in a terminal state, we can trigger it again
            if task.status == TaskStatus::AwaitingTrigger
                || task.status == TaskStatus::Completed
                || task.status == TaskStatus::Failed
            {
                if let Some(master_task_id) = task.master_task_id {
                    parent_master_ids.insert(master_task_id);
                }
                self.task_state_service().mark_running(task_id).await?;

                triggered = true;
                triggered_task_ids.push(task_id);
                slog!(
                    &self.structured_logger,
                    info,
                    "Triggered task {} ({})",
                    task_id,
                    task.node_id
                );
            } else {
                slog!(
                    &self.structured_logger,
                    warn,
                    "Task {task_id} is not awaiting trigger"
                );
            }
        }

        if !triggered {
            return Err(Error::Other("No tasks were triggered".to_string()));
        }

        for master_task_id in parent_master_ids {
            self.task_state_service()
                .update_matrix_master_status(master_task_id)
                .await?;
        }

        self.task_state_service()
            .mark_workflow_running(workflow_run_id)
            .await?;

        for task_id in triggered_task_ids {
            if let Err(e) = self.spawn_task_with_handle(task_id).await {
                self.emit_error(format!("Failed to spawn task {}: {}", task_id, e));
            }
        }

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
                self.task_state_service()
                    .mark_workflow_completed(workflow_run_id)
                    .await?;
                slog!(
                    &self.structured_logger,
                    info,
                    "Workflow run {workflow_run_id} is now complete"
                );
                return Ok(true);
            }

            // If we reached here, it means the workflow is still running but no tasks need triggers
            slog!(
                &self.structured_logger,
                info,
                "No tasks in workflow run {workflow_run_id} are awaiting triggers"
            );
            return Ok(false);
        }

        let mut triggered = false;
        let mut parent_master_ids = HashSet::new();
        let mut triggered_task_ids = Vec::new();
        for task in awaiting_tasks {
            if let Some(master_task_id) = task.master_task_id {
                parent_master_ids.insert(master_task_id);
            }
            self.task_state_service().mark_running(task.id).await?;

            triggered = true;
            triggered_task_ids.push(task.id);
            slog!(
                &self.structured_logger,
                info,
                "Triggered task {} ({})",
                task.id,
                task.node_id
            );
        }

        // If no tasks were triggered, it means they're all done or in progress
        // We don't need to error out, just return successfully
        if !triggered {
            return Ok(false);
        }

        for master_task_id in parent_master_ids {
            self.task_state_service()
                .update_matrix_master_status(master_task_id)
                .await?;
        }

        self.task_state_service()
            .mark_workflow_running(workflow_run_id)
            .await?;

        for task_id in triggered_task_ids {
            if let Err(e) = self.spawn_task_with_handle(task_id).await {
                self.emit_error(format!("Failed to spawn task {}: {}", task_id, e));
            }
        }

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

        // Flip in-process cancel signals first so cooperative steps (the
        // js-ast-grep file loop) stop taking new work before we spend time
        // writing cancellation state.
        self.signal_step_cancellation();

        // Get all tasks
        let tasks = self
            .state_adapter
            .lock()
            .await
            .get_tasks(workflow_run_id)
            .await?;

        // Cancel all running tasks
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Running) {
            let updated_task = self
                .task_state_service()
                .mark_failed(task.id, "Canceled by user")
                .await?;
            slog!(
                &self.structured_logger,
                info,
                "Canceled task {} ({})",
                task.id,
                task.node_id
            );
            self.update_parent_matrix_master_for_task(&updated_task)
                .await?;
        }

        self.task_state_service()
            .mark_workflow_canceled(workflow_run_id)
            .await?;

        self.task_completion_notify.notify_waiters();
        self.wake_scheduler();

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
        NestedCodemodService::new(&self.workflow_run_config.execution.registry_client)
            .validate_workflow_dependencies(workflow, dependency_chain)
            .await
    }

    /// Find if a codemod source creates a cycle in the dependency chain
    pub fn find_cycle_in_chain(
        &self,
        source: &str,
        dependency_chain: &[CodemodDependency],
    ) -> Option<String> {
        NestedCodemodService::find_cycle_in_chain(source, dependency_chain)
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
        // different cwd selected by the caller.
        if let Some(target_path) = &workflow_run.target_path {
            self.workflow_run_config.execution.target_path = target_path.clone();
        }

        self.task_state_service()
            .mark_workflow_running(workflow_run_id)
            .await?;

        slog!(
            &self.structured_logger,
            info,
            "Starting workflow run {workflow_run_id}"
        );

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
            // Register before reading state so a scheduler wake cannot be
            // missed between deciding there is no runnable work and awaiting.
            let scheduler_wake = self.scheduler_wake_notify.notified();

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
                && !self.workflow_run_config.execution.flatten_matrix_tasks
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
                    slog!(
                        &self.structured_logger,
                        debug,
                        "State changed, triggering matrix recompilation for workflow {workflow_run_id}"
                    );
                }

                state_changed
            } else {
                false
            };

            if should_recompile {
                slog!(
                    &self.structured_logger,
                    debug,
                    "Starting matrix task recompilation for workflow {workflow_run_id}"
                );
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
                slog!(
                    &self.structured_logger,
                    debug,
                    "Completed matrix task recompilation for workflow {workflow_run_id}"
                );
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

                if any_failed {
                    self.task_state_service()
                        .mark_workflow_failed(workflow_run_id)
                        .await?;
                } else {
                    self.task_state_service()
                        .mark_workflow_completed(workflow_run_id)
                        .await?;
                }

                slog!(
                    &self.structured_logger,
                    info,
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

            if self.workflow_run_config.execution.auto_trigger_manual_steps {
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

                        // A manually resumed task may already be Running by the time the
                        // workflow loop wakes back up. Do not overwrite that progress by
                        // re-marking it as AwaitingTrigger.
                        if task.status == TaskStatus::Running {
                            continue;
                        }
                    }

                    self.task_state_service()
                        .set_status(task_id, TaskStatus::AwaitingTrigger)
                        .await?;
                }

                for master_task_id in parent_master_ids {
                    self.task_state_service()
                        .update_matrix_master_status(master_task_id)
                        .await?;
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
            let any_pending = tasks_after_status_updates
                .iter()
                .any(|t| t.status == TaskStatus::Pending);

            // If there are tasks awaiting trigger and no runnable tasks and no running tasks,
            // then we need to pause the workflow and wait for manual triggers
            if awaiting_trigger && runnable_tasks.is_empty() && !any_running && !any_pending {
                self.task_state_service()
                    .mark_workflow_awaiting_trigger(workflow_run_id)
                    .await?;

                slog!(
                    &self.structured_logger,
                    info,
                    "Workflow run {workflow_run_id} is awaiting triggers"
                );

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
                let execution_result = std::panic::AssertUnwindSafe(self.execute_task(task_id))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|panic_payload| {
                        Err(Error::Runtime(format!(
                            "Task execution panicked: {}",
                            panic_payload_message(panic_payload.as_ref())
                        )))
                    });
                if let Err(e) = execution_result {
                    let current_task = self.state_adapter.lock().await.get_task(task_id).await?;
                    if matches!(
                        current_task.status,
                        TaskStatus::Failed
                            | TaskStatus::Completed
                            | TaskStatus::WontDo
                            | TaskStatus::AwaitingTrigger
                    ) {
                        continue;
                    }
                    if let Error::Deferred(message) = &e {
                        let _ = self
                            .append_task_log(
                                task_id,
                                format!("Task returned to awaiting trigger: {message}"),
                            )
                            .await;
                        let _ = self.mark_task_as_awaiting_trigger(task_id).await;
                        continue;
                    }

                    self.emit_error(format!("Task execution failed: {e}"));
                    let _ = self.mark_task_as_failed(task_id, &e.to_string()).await;
                }
            }

            // Only wait if no tasks were executed (to avoid busy waiting)
            if runnable_tasks_is_empty {
                slog!(
                    &self.structured_logger,
                    debug,
                    "Waiting for scheduler wake for workflow {workflow_run_id}"
                );
                scheduler_wake.await;
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
        slog!(
            &self.structured_logger,
            debug,
            "Starting matrix task recompilation for run {workflow_run_id}"
        );

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
            slog!(
                &self.structured_logger,
                debug,
                "Creating new matrix task for node '{}'",
                task.node_id
            );
            self.state_adapter.lock().await.save_task(&task).await?;
            self.wake_scheduler();
            self.emit_task_created(&task).await;
        }

        // Mark tasks as WontDo
        for task_id in changes.tasks_to_mark_wont_do {
            slog!(
                &self.structured_logger,
                debug,
                "Marking task {task_id} as WontDo"
            );
            self.task_state_service().mark_wont_do(task_id).await?;
        }

        for task_id in changes.tasks_to_reset_to_pending {
            slog!(
                &self.structured_logger,
                debug,
                "Resetting task {task_id} from Failed to Pending"
            );
            self.task_state_service().reset_to_pending(task_id).await?;
        }

        // Update master task status
        for master_task_id in changes.master_tasks_to_update {
            slog!(
                &self.structured_logger,
                debug,
                "Updating master task {master_task_id} status"
            );
            self.task_state_service()
                .update_matrix_master_status(master_task_id)
                .await?;
        }

        slog!(
            &self.structured_logger,
            debug,
            "Finished matrix task recompilation for run {workflow_run_id}"
        );
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

        let resolved_params = resolve_workflow_run_params(&workflow_run);

        let node = workflow_run
            .workflow
            .nodes
            .iter()
            .find(|n| n.id == task.node_id)
            .ok_or_else(|| Error::NodeNotFound(task.node_id.clone()))?;

        self.task_state_service().mark_running(task_id).await?;

        self.update_parent_matrix_master_for_task(&task).await?;

        slog!(
            &self.structured_logger,
            info,
            "Executing task {} ({})",
            task_id,
            node.id
        );

        // Workflows that declare git outputs should use the managed branch/commit/PR path
        // in both cloud and local runs.
        let manage_git = should_manage_git_for_node(
            node,
            self.workflow_run_config.managed_git.enable_managed_git,
        );
        // Always build task expression context so CODEMOD_TASK_* env vars are
        // available as `task.*` template variables regardless of mode.
        let task_expr_ctx = Some(crate::git_ops::build_task_expression_context(
            &task.id.to_string(),
        ));
        let managed_branch_name = if manage_git {
            ManagedGitService::new(self)
                .begin_task_branch(
                    &task,
                    node,
                    &resolved_params,
                    task_expr_ctx.as_ref().unwrap(),
                )
                .await?
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
                RuntimeType::Direct => Box::new(DirectRunner::with_quiet(
                    self.workflow_run_config.output.quiet,
                )),
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
            let step_start_time = std::time::Instant::now();

            let quiet_capture = self.workflow_run_config.output.quiet
                && self.workflow_run_config.output.capture_stdout_in_quiet_mode
                && !self.structured_logger.is_jsonl();

            // In JSONL mode, capture ALL stdout (fd 1) during step execution.
            // Any println!, console.log, etc. from child processes, AI agents,
            // or JS codemods will be intercepted and wrapped in JSONL with the
            // correct step context. The structured logger bypasses the capture
            // by writing directly to the saved real stdout fd.
            let _stdout_capture = if self.structured_logger.is_jsonl() {
                StdoutCaptureGuard::start(Some(&step_logger), None)
            } else if self.workflow_run_config.output.quiet
                && self.workflow_run_config.output.capture_stdout_in_quiet_mode
            {
                let output_heartbeat_callbacks = Arc::clone(&self.output_heartbeat_callbacks);
                let line_callback = quiet_capture.then(|| {
                    Arc::new(move |_line: String| {
                        let heartbeat = output_heartbeat_callbacks
                            .lock()
                            .ok()
                            .and_then(|callbacks| callbacks.get(&task_id).cloned());
                        if let Some(heartbeat) = heartbeat {
                            heartbeat();
                        }
                    }) as crate::structured_log::CapturedLineCallback
                });
                StdoutCaptureGuard::start(Some(&step_logger), line_callback)
            } else {
                None
            };

            let result = std::panic::AssertUnwindSafe(StepExecutor::new(self).execute(
                StepExecutionRequest {
                    runner: runner.as_ref(),
                    action: &step.action,
                    step_name: &step.name,
                    step_env: &step.env,
                    step_id: &step.id,
                    node,
                    task: &task,
                    params: &resolved_params,
                    state: &state,
                    workflow: &workflow_run.workflow,
                    bundle_path: &workflow_run.bundle_path,
                    dependency_chain: &[],
                    capabilities: &self.workflow_run_config.execution.capabilities,
                    task_expr_ctx: task_expr_ctx.as_ref(),
                    logger: &step_logger,
                },
            ))
            .catch_unwind()
            .await
            .unwrap_or_else(|panic_payload| {
                Err(Error::Runtime(format!(
                    "Step {} panicked: {}",
                    step.name,
                    panic_payload_message(panic_payload.as_ref())
                )))
            });
            // Drop the capture guard to restore stdout before emitting step_end.
            // This ensures all captured output is flushed and attributed to this step.
            drop(_stdout_capture);
            for line in step_logger.drain_logs() {
                let _ = self.append_task_log(task_id, line).await;
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

                    if manage_git {
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
                                &self.workflow_run_config.execution.target_path,
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
                Err(Error::Deferred(message)) => {
                    step_logger.step_end("deferred", step_start_time.elapsed().as_millis() as u64);
                    return Err(Error::Deferred(message));
                }
                Err(e) => {
                    step_logger.step_end("failure", step_start_time.elapsed().as_millis() as u64);
                    let failure_message = format!("Step {} failed: {}", step.name, e);
                    let error_details = task_error_details(&e);
                    let _ = self.append_task_log(task_id, failure_message.clone()).await;

                    let failed_task = self
                        .task_state_service()
                        .mark_failed_with_details(task_id, failure_message, error_details)
                        .await?;
                    self.update_parent_matrix_master_for_task(&failed_task)
                        .await?;

                    self.emit_error(format!(
                        "Task {} ({}) step {} failed: {}",
                        task_id, node.id, step.name, e
                    ));

                    return Err(e);
                }
            }
        }

        // Managed git mode: finalize — fallback commit if needed, then push + create PR
        if manage_git {
            ManagedGitService::new(self)
                .finalize_task(
                    task_id,
                    &task,
                    node,
                    &resolved_params,
                    managed_branch_name.as_ref(),
                    &mut had_commit_checkpoint,
                )
                .await?;
        }

        // Prepare environment variables
        let _ = self
            .append_task_log(task_id, "Marking task as completed")
            .await;
        slog!(
            &self.structured_logger,
            debug,
            "Task {task_id} finished all steps; preparing completion state"
        );
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

        let completed_task = self.task_state_service().mark_completed(task_id).await?;

        slog!(
            &self.structured_logger,
            info,
            "Task {task_id} ({}) completed",
            node.id
        );

        // If this is a matrix task, update the master task status
        if let Some(master_task_id) = completed_task.master_task_id {
            slog!(
                &self.structured_logger,
                debug,
                "Updating matrix master task {master_task_id} after task {task_id} completed"
            );
            self.task_state_service()
                .update_matrix_master_status(master_task_id)
                .await?;
        }

        // Notify that a task has completed (for event-driven waiting)
        slog!(
            &self.structured_logger,
            debug,
            "Notifying task completion listeners for task {task_id}"
        );
        self.task_completion_notify.notify_one();
        self.wake_scheduler();

        Ok(())
    }

    pub async fn execute_ast_grep_step(
        &self,
        id: String,
        ast_grep: &UseAstGrep,
        logger: &StructuredLogger,
    ) -> Result<()> {
        let bundle_path = self.workflow_run_config.execution.bundle_path.clone();
        let progress_callback = self.workflow_run_config.execution.progress_callback.clone();

        let config_path = bundle_path.join(&ast_grep.config_file);

        if !config_path.exists() {
            let message = format!("AST grep config file not found: {}", config_path.display());
            let buffered_output =
                Arc::new(std::sync::Mutex::new(BufferedExecutionOutput::default()));
            append_buffered_diagnostic(
                &buffered_output,
                "ast-grep config".to_string(),
                message.clone(),
            );
            flush_buffered_execution_output(&buffered_output, &progress_callback, &id);
            if let Ok(task_id) = Uuid::parse_str(&id) {
                let _ = self.append_task_log(task_id, &message).await;
            }
            return Err(Error::AstGrepStepFailed {
                message,
                help: ast_grep_step_help(),
            });
        }

        if let Some(pre_run_callback) = self.workflow_run_config.execution.pre_run_callback.as_ref()
        {
            pre_run_callback(
                &self.workflow_run_config.execution.target_path,
                self.workflow_run_config.execution.dry_run,
                &self.workflow_run_config,
            )
            .map_err(|error| Error::Other(format!("Pre-run check failed: {error}")))?;
        }

        let config_path_clone = config_path.clone();

        let scan_result = with_combined_scan(
            &config_path_clone.to_string_lossy(),
            |combined_scan_with_rule| {
                let rule_refs = combined_scan_with_rule.rule_refs.clone();
                let languages = rule_refs.iter().map(|r| r.language).collect::<Vec<_>>();

                let execution_config = CodemodExecutionConfig {
                    pre_run_callback: None,
                    progress_callback: self.workflow_run_config.execution.progress_callback.clone(),
                    target_path: Some(self.workflow_run_config.execution.target_path.clone()),
                    base_path: ast_grep.base_path.as_deref().map(PathBuf::from),
                    include_globs: ast_grep.include.as_deref().map(|v| v.to_vec()),
                    explicit_files: None,
                    exclude_globs: ast_grep.exclude.as_deref().map(|v| v.to_vec()),
                    dry_run: self.workflow_run_config.execution.dry_run,
                    languages: Some(languages.iter().map(|l| l.to_string()).collect()),
                    threads: ast_grep.max_threads,
                    capabilities: None,
                };

                // Clone variables needed in the closure
                let progress_task_id = id.clone();
                let callback_task_id = id.clone();
                let file_writer = Arc::clone(&self.file_writer);
                let runtime_handle = tokio::runtime::Handle::current();
                let logger = logger.clone();
                let progress_callback =
                    self.workflow_run_config.execution.progress_callback.clone();
                let target_path_for_logs = self.workflow_run_config.execution.target_path.clone();
                let buffered_execution_output =
                    Arc::new(std::sync::Mutex::new(BufferedExecutionOutput::default()));
                let buffered_execution_output_for_closure = Arc::clone(&buffered_execution_output);
                let attempted_file_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let succeeded_file_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let failed_file_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let first_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));
                let attempted_file_count_for_closure = Arc::clone(&attempted_file_count);
                let succeeded_file_count_for_closure = Arc::clone(&succeeded_file_count);
                let failed_file_count_for_closure = Arc::clone(&failed_file_count);
                let first_failure_message_for_closure = Arc::clone(&first_failure_message);

                let execute_result = execution_config.execute_with_task_id(
                    &progress_task_id,
                    move |path, config| {
                        // Only process files, not directories
                        if !path.is_file() {
                            return;
                        }
                        attempted_file_count_for_closure.fetch_add(1, Ordering::Relaxed);

                        let execution_title = path
                            .strip_prefix(&target_path_for_logs)
                            .unwrap_or(path)
                            .display()
                            .to_string();

                        let record_failure = |message: String| {
                            failed_file_count_for_closure.fetch_add(1, Ordering::Relaxed);
                            append_buffered_diagnostic(
                                &buffered_execution_output_for_closure,
                                execution_title.clone(),
                                message.clone(),
                            );
                            if let Ok(mut first_failure_message) =
                                first_failure_message_for_closure.lock()
                            {
                                if first_failure_message.is_none() {
                                    *first_failure_message = Some(format!(
                                        "Failed to process {execution_title}: {message}"
                                    ));
                                }
                            }
                        };

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
                                    append_buffered_log(
                                        &buffered_execution_output_for_closure,
                                        execution_title.clone(),
                                        format!("Found {} matches", matches.len()),
                                    );
                                }
                                if file_modified {
                                    if let Some(new_content) = new_content {
                                        // Use async file writing to avoid blocking the thread
                                        let write_result =
                                            block_on_runtime_handle(&runtime_handle, async {
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
                                            record_failure(format!(
                                                "Failed to write modified file {}: {e}",
                                                path.display()
                                            ));
                                            return;
                                        }
                                    }
                                    self.execution_stats
                                        .files_modified
                                        .fetch_add(1, Ordering::Relaxed);
                                    append_buffered_log(
                                        &buffered_execution_output_for_closure,
                                        execution_title.clone(),
                                        if config.dry_run {
                                            "Would modify file".to_string()
                                        } else {
                                            "Modified file".to_string()
                                        },
                                    );
                                } else {
                                    self.execution_stats
                                        .files_unmodified
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                succeeded_file_count_for_closure.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(e) => {
                                slog!(logger, error, "{e}");
                                self.execution_stats
                                    .files_with_errors
                                    .fetch_add(1, Ordering::Relaxed);
                                record_failure(e.to_string());
                            }
                        };

                        if let Some(callback) = self
                            .workflow_run_config
                            .execution
                            .progress_callback
                            .as_ref()
                        {
                            let callback = callback.callback.clone();
                            callback(
                                &callback_task_id,
                                &path.to_string_lossy(),
                                "next",
                                Some(&1),
                                &0,
                            );
                        }
                    },
                );

                if let Err(error) = execute_result {
                    flush_buffered_execution_output(
                        &buffered_execution_output,
                        &progress_callback,
                        &id,
                    );
                    return Err(error);
                }

                flush_buffered_execution_output(
                    &buffered_execution_output,
                    &progress_callback,
                    &id,
                );

                let attempted_files = attempted_file_count.load(Ordering::Relaxed);
                let failed_files = failed_file_count.load(Ordering::Relaxed);
                let succeeded_files = succeeded_file_count.load(Ordering::Relaxed);
                if attempted_files > 0 && failed_files == attempted_files && succeeded_files == 0 {
                    if let Some(message) = first_failure_message
                        .lock()
                        .ok()
                        .and_then(|message| message.clone())
                    {
                        return Err(Box::new(std::io::Error::other(message)));
                    }
                }

                Ok(())
            },
        );

        if let Err(error) = scan_result {
            let message = error.to_string();
            let buffered_output =
                Arc::new(std::sync::Mutex::new(BufferedExecutionOutput::default()));
            append_buffered_diagnostic(
                &buffered_output,
                config_path
                    .strip_prefix(&bundle_path)
                    .unwrap_or(&config_path)
                    .display()
                    .to_string(),
                message.clone(),
            );
            flush_buffered_execution_output(&buffered_output, &progress_callback, &id);
            if let Ok(task_id) = Uuid::parse_str(&id) {
                let _ = self.append_task_log(task_id, &message).await;
            }
            return Err(Error::AstGrepStepFailed {
                message,
                help: ast_grep_step_help(),
            });
        }

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
        selector_matched_files_collector: Option<Arc<std::sync::Mutex<Vec<PathBuf>>>>,
        task_expr_ctx: Option<&TaskExpressionContext>,
    ) -> Result<()> {
        JssgExecutionService::new(self)
            .execute(JssgExecutionRequest {
                id,
                step_id,
                js_ast_grep,
                params,
                matrix_input,
                capabilities_data,
                bundle_path,
                workflow_run_id,
                initial_state,
                logger,
                modified_files_collector,
                selector_matched_files_collector,
                task_expr_ctx,
            })
            .await
    }

    /// Execute an AI agent step
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_ai_step(
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
            debug,
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
        if let Some(ref agent_name) = self.workflow_run_config.interaction.agent {
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
        if !self.workflow_run_config.interaction.no_interactive {
            if let Some(ref callback) = self
                .workflow_run_config
                .interaction
                .agent_selection_callback
            {
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
                            if !self.workflow_run_config.output.quiet {
                                logger.user_line("");
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
            working_dir: self.workflow_run_config.execution.target_path.clone(),
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
    pub(crate) async fn execute_codemod_step(
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

        let resolved =
            NestedCodemodService::new(&self.workflow_run_config.execution.registry_client)
                .resolve(&codemod.source, dependency_chain)
                .await?;

        slog!(
            logger,
            info,
            "Resolved codemod package: {} -> {}",
            codemod.source,
            resolved.package.package_dir.display()
        );

        self.run_codemod_workflow_with_chain(
            &resolved.package,
            &resolved.workflow,
            codemod,
            step_env,
            node,
            task,
            params,
            state,
            bundle_path,
            &resolved.dependency_chain,
            capabilities,
            logger,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_codemod_workflow_with_chain(
        &self,
        resolved_package: &ResolvedPackage,
        codemod_workflow: &Workflow,
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
        let runner: Box<dyn Runner> = Box::new(DirectRunner::with_quiet(
            self.workflow_run_config.output.quiet,
        ));

        // Build task expression context for variable resolution in codemod steps
        let codemod_task_expr_ctx =
            crate::git_ops::build_task_expression_context(&task.id.to_string());
        let codemod_bundle_path = Some(resolved_package.package_dir.clone());

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

                StepExecutor::new(self)
                    .execute(StepExecutionRequest {
                        runner: runner.as_ref(),
                        action: &step.action,
                        step_name: &step.name,
                        step_env: &step.env,
                        step_id: &step.id,
                        node,
                        task,
                        params,
                        state,
                        workflow: codemod_workflow,
                        bundle_path: &codemod_bundle_path,
                        dependency_chain,
                        capabilities,
                        task_expr_ctx: Some(&codemod_task_expr_ctx),
                        logger,
                    })
                    .await?;
            }
        }

        slog!(logger, info, "Codemod workflow completed successfully");
        Ok(())
    }

    /// Execute a shard step — evaluate file shards and write results to workflow state.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_shard_step(
        &self,
        shard_config: &butterflow_models::step::UseShard,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        task_expr_ctx: Option<&TaskExpressionContext>,
        logger: &StructuredLogger,
    ) -> Result<()> {
        use crate::shard::evaluate_builtin_shards;
        use butterflow_models::step::ShardMethod;

        let target_path = self.workflow_run_config.execution.target_path.clone();

        // If a js-ast-grep config is set, pre-scan to find only files with matches
        let eligible_files = if let Some(js_ast_grep) = &shard_config.js_ast_grep {
            Some(
                self.scan_eligible_files_with_jssg(
                    shard_config,
                    js_ast_grep,
                    &target_path,
                    task.id,
                    task.workflow_run_id,
                    logger,
                )
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
            ShardMethod::Builtin(builtin) => {
                // Resolve max_files_per_shard / min_shard_size expressions
                // using the already-resolved params (with defaults applied).
                let resolved_method = crate::shard::ResolvedBuiltinShardMethod {
                    r#type: builtin.r#type,
                    max_files_per_shard: resolve_usize_value(
                        &builtin.max_files_per_shard,
                        params,
                        state,
                        task.matrix_values.as_ref(),
                        task_expr_ctx,
                    )?,
                    min_shard_size: builtin
                        .min_shard_size
                        .as_ref()
                        .map(|v| {
                            resolve_usize_value(
                                v,
                                params,
                                state,
                                task.matrix_values.as_ref(),
                                task_expr_ctx,
                            )
                        })
                        .transpose()?,
                };
                evaluate_builtin_shards(
                    shard_config,
                    &target_path,
                    eligible_files.as_deref(),
                    previous_shards.as_ref(),
                    &resolved_method,
                )
                .map_err(|e| Error::Runtime(format!("Shard evaluation failed: {e}")))?
            }
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
        self.wake_scheduler();

        Ok(())
    }

    /// Dry-run the js-ast-grep step via `execute_js_ast_grep_step` to find which files
    /// would be modified. Returns relative file paths (relative to target_path).
    async fn scan_eligible_files_with_jssg(
        &self,
        shard_config: &butterflow_models::step::UseShard,
        js_ast_grep: &butterflow_models::step::UseJSAstGrep,
        target_path: &Path,
        task_id: Uuid,
        workflow_run_id: Uuid,
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
        let selector_match_collector: Arc<std::sync::Mutex<Vec<PathBuf>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let capabilities = self
            .workflow_run_config
            .execution
            .capabilities
            .as_ref()
            .map(|v| v.clone().into_iter().collect());

        self.execute_js_ast_grep_step(
            task_id.to_string(),
            "shard-scan".to_string(),
            &dry_run_config,
            None,
            None,
            &CapabilitiesData {
                capabilities,
                capabilities_security_callback: self
                    .workflow_run_config
                    .execution
                    .capabilities_security_callback
                    .as_ref()
                    .map(|callback| callback.clone()),
            },
            &None,
            Some(workflow_run_id),
            None,
            logger,
            Some(collector.clone()),
            Some(selector_match_collector.clone()),
            None,
        )
        .await?;

        let modified_paths = Arc::try_unwrap(collector)
            .map(|mutex| mutex.into_inner().unwrap())
            .unwrap_or_else(|arc| arc.lock().unwrap().clone());
        let selector_matched_paths = Arc::try_unwrap(selector_match_collector)
            .map(|mutex| mutex.into_inner().unwrap())
            .unwrap_or_else(|arc| arc.lock().unwrap().clone());

        let modified_files: Vec<String> = modified_paths
            .into_iter()
            .filter_map(|p| {
                p.strip_prefix(target_path)
                    .ok()
                    .map(|rel| rel.to_string_lossy().to_string())
            })
            .collect();
        let selector_matched_files: Vec<String> = selector_matched_paths
            .into_iter()
            .filter_map(|p| {
                p.strip_prefix(target_path)
                    .ok()
                    .map(|rel| rel.to_string_lossy().to_string())
            })
            .collect();
        let used_selector_fallback =
            modified_files.is_empty() && !selector_matched_files.is_empty();
        let eligible = select_shard_scan_eligible_files(modified_files, selector_matched_files);

        if used_selector_fallback {
            slog!(
                logger,
                info,
                "Shard scan found selector matches but no dry-run edits; using selector-matched files"
            );
        }

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

        let effective_bundle_path = &self.workflow_run_config.execution.bundle_path;
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
            capabilities: self.workflow_run_config.execution.capabilities.clone(),
            target_directory: Some(target_path),
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
    pub(crate) async fn execute_run_script_step(
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

        if self
            .workflow_run_config
            .interaction
            .shell_command_approval_callback
            .is_none()
            && !logger.is_jsonl()
            && !self.workflow_run_config.output.quiet
        {
            logger.transient_user_line("");
            logger.transient_user_line(&notice);
            logger.transient_user_line("");
        }

        if let Some(approval_callback) = &self
            .workflow_run_config
            .interaction
            .shell_command_approval_callback
        {
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
        let workflow_run_id = task.workflow_run_id;
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
                current_task.logs.push(line.clone());
                let _ = adapter.save_task(&current_task).await;
                publish_event(
                    workflow_run_id,
                    WorkflowEvent::TaskLogAppended {
                        workflow_run_id,
                        task_id,
                        line,
                        at: Utc::now(),
                    },
                );
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
            .await
            .map_err(|error| {
                if let Error::ShellCommandFailed { exit_code, output } = error {
                    Error::ShellCommandStepFailed {
                        command: resolved_command.clone(),
                        exit_code,
                        output,
                    }
                } else {
                    Error::StepExecution(format!("Shell command failed: {error}"))
                }
            });
        drop(output_callback);
        let _ = log_persist_task.await;

        self.finalize_step_execution(task, output?, prepared).await
    }

    pub(crate) fn prepare_step_execution(
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
        if self.workflow_run_config.interaction.no_interactive {
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

    pub(crate) async fn finalize_step_execution(
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
                slog!(
                    &self.structured_logger,
                    warn,
                    "Malformed state output line: {line}"
                );
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

        if !self.workflow_run_config.execution.skip_state_writes {
            self.state_adapter
                .lock()
                .await
                .apply_state_diff(&StateDiff {
                    workflow_run_id: task.workflow_run_id,
                    fields: state_diff,
                })
                .await?;
            self.wake_scheduler();
        }
        Ok(())
    }
}

impl Clone for Engine {
    fn clone(&self) -> Self {
        Self {
            state_adapter: Arc::clone(&self.state_adapter),
            task_state_service: self.task_state_service.clone(),
            scheduler: Scheduler::new(),
            workflow_run_config: self.workflow_run_config.clone(),
            execution_stats: Arc::clone(&self.execution_stats),
            metrics_context: self.metrics_context.clone(),
            file_writer: Arc::clone(&self.file_writer),
            task_completion_notify: Arc::clone(&self.task_completion_notify),
            scheduler_wake_notify: Arc::clone(&self.scheduler_wake_notify),
            structured_logger: self.structured_logger.clone(),
            output_heartbeat_callbacks: Arc::clone(&self.output_heartbeat_callbacks),
            step_cancel_signals: Arc::clone(&self.step_cancel_signals),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        InstallSkillExecutionRequest, InstallSkillExecutor, SelectionPrompt, SelectionPromptOption,
    };
    use anyhow::Result as AnyhowResult;
    use butterflow_models::step::{SemanticAnalysisConfig, SemanticAnalysisMode};
    use serial_test::serial;
    use std::sync::Arc;
    use tokio::sync::mpsc;

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
    #[serial]
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
    fn shard_scan_falls_back_to_selector_matches_when_dry_run_finds_no_edits() {
        let eligible = select_shard_scan_eligible_files(
            Vec::new(),
            vec!["src/a.ts".to_string(), "src/b.ts".to_string()],
        );

        assert_eq!(eligible, vec!["src/a.ts", "src/b.ts"]);
    }

    #[test]
    fn shard_scan_prefers_modified_files_when_available() {
        let eligible = select_shard_scan_eligible_files(
            vec!["src/changed.ts".to_string()],
            vec!["src/selector-only.ts".to_string()],
        );

        assert_eq!(eligible, vec!["src/changed.ts"]);
    }

    #[tokio::test]
    async fn dry_run_js_ast_grep_does_not_persist_shared_state() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let temp_path = temp_dir.path();
        std::fs::create_dir_all(temp_path.join("src")).unwrap();
        std::fs::write(
            temp_path.join("stateful-codemod.js"),
            r#"
import { setState } from "codemod:workflow";

export default function transform(ast) {
  setState("preScanMutation", "leaked");
  return null;
}
"#,
        )
        .unwrap();
        std::fs::write(temp_path.join("src/app.js"), "const value = 1;\n").unwrap();

        let workflow_run_id = Uuid::new_v4();
        let config = WorkflowRunConfig {
            execution: crate::config::WorkflowExecutionSettings {
                bundle_path: temp_path.to_path_buf(),
                target_path: temp_path.to_path_buf(),
                ..WorkflowRunConfig::default().execution
            },
            ..WorkflowRunConfig::default()
        };
        let engine = Engine::with_state_adapter(
            Box::new(LocalStateAdapter::with_base_dir(
                temp_path.join("state-store"),
            )),
            config,
        );

        engine
            .execute_js_ast_grep_step(
                "test-node".to_string(),
                "test-step".to_string(),
                &UseJSAstGrep {
                    js_file: "stateful-codemod.js".to_string(),
                    base_path: Some("src".to_string()),
                    include: Some(vec!["**/*.js".to_string()]),
                    exclude: None,
                    max_threads: None,
                    dry_run: Some(true),
                    language: Some("javascript".to_string()),
                    capabilities: None,
                    semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                        SemanticAnalysisMode::File,
                    )),
                },
                None,
                None,
                &CapabilitiesData {
                    capabilities: None,
                    capabilities_security_callback: None,
                },
                &None,
                Some(workflow_run_id),
                None,
                &StructuredLogger::default(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let state = engine
            .state_adapter
            .lock()
            .await
            .get_state(workflow_run_id)
            .await
            .unwrap();
        assert!(
            !state.contains_key("preScanMutation"),
            "dry-run shard scans must not persist codemod workflow state"
        );
    }

    #[test]
    #[serial]
    fn managed_git_mode_is_enabled_for_local_pull_request_nodes() {
        let _guard = EnvVarGuard::unset("BUTTERFLOW_STATE_BACKEND");
        let node = Node {
            id: "apply-transforms".to_string(),
            name: "Apply AST Transformations".to_string(),
            description: None,
            r#type: butterflow_models::node::NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![],
            env: HashMap::new(),
            branch_name: None,
            pull_request: Some(butterflow_models::step::PullRequestConfig {
                title: "PR".to_string(),
                body: None,
                draft: Some(true),
                base: None,
            }),
        };

        assert!(should_manage_git_for_node(&node, true));
        assert!(!should_manage_git_for_node(&node, false));
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

    struct PromptingInstallSkillExecutor;

    #[async_trait::async_trait]
    impl InstallSkillExecutor for PromptingInstallSkillExecutor {
        async fn execute(&self, request: InstallSkillExecutionRequest) -> AnyhowResult<String> {
            let callback = request
                .selection_prompt_callback
                .as_ref()
                .expect("selection callback should be configured");
            let selection = callback(SelectionPrompt {
                title: "Choose install scope".to_string(),
                options: vec![
                    SelectionPromptOption {
                        value: "project".to_string(),
                        label: "project".to_string(),
                    },
                    SelectionPromptOption {
                        value: "user".to_string(),
                        label: "user".to_string(),
                    },
                ],
                default_index: 0,
            })?
            .expect("selection should be provided");

            Ok(format!("installed {selection}"))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn install_skill_isolated_runtime_unblocks_current_thread_prompt_flow() {
        let (selection_tx, mut selection_rx) =
            mpsc::unbounded_channel::<std::sync::mpsc::SyncSender<Option<String>>>();
        let callback = Arc::new(move |_prompt: SelectionPrompt| {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            selection_tx
                .send(tx)
                .expect("selection request should reach the test task");
            rx.recv()
                .map_err(|error| anyhow::anyhow!("selection response channel closed: {error}"))
        });
        let request = InstallSkillExecutionRequest {
            install_skill: butterflow_models::step::UseInstallSkill {
                package: "debarrel".to_string(),
                path: None,
                harness: None,
                scope: None,
                force: None,
            },
            no_interactive: false,
            quiet: true,
            bundle_path: None,
            target_path: PathBuf::from("."),
            env: HashMap::new(),
            output_format: crate::structured_log::OutputFormat::Text,
            selection_prompt_callback: Some(callback),
        };

        let execution = tokio::spawn(async move {
            execute_install_skill_in_isolated_runtime(
                Arc::new(PromptingInstallSkillExecutor),
                request,
            )
            .await
        });

        let responder = tokio::time::timeout(Duration::from_secs(5), selection_rx.recv())
            .await
            .expect("selection request should be emitted")
            .expect("selection responder should be provided");
        responder
            .send(Some("user".to_string()))
            .expect("selection response should be delivered");

        let output = tokio::time::timeout(Duration::from_secs(5), execution)
            .await
            .expect("isolated install-skill execution should finish")
            .expect("join handle should complete")
            .expect("install-skill execution should succeed");

        assert_eq!(output, "installed user");
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

    #[test]
    fn pull_request_metadata_log_line_omits_body() {
        let line = pull_request_metadata_log_line(&ResolvedPullRequestConfig {
            title: "Create PR".to_string(),
            body: Some("secret body".to_string()),
            draft: true,
            base: Some("main".to_string()),
            branch: "codemod-branch".to_string(),
        });

        assert!(line.starts_with(PULL_REQUEST_METADATA_LOG_PREFIX));
        assert!(line.contains(r#""title":"Create PR""#));
        assert!(line.contains(r#""branch":"codemod-branch""#));
        assert!(!line.contains("secret body"));
        assert!(!line.contains(r#""body""#));
    }

    #[test]
    fn codex_agent_output_is_not_tagged_as_stderr() {
        assert_eq!(
            format_agent_stream_line("codex", "stderr", "hello".to_string()),
            "hello"
        );
        assert_eq!(
            format_agent_stream_line("claude-code", "stdout", "hello".to_string()),
            "hello"
        );
        assert_eq!(
            format_agent_stream_line("aider", "stderr", "hello".to_string()),
            "[stderr] hello"
        );
    }

    #[test]
    fn live_agent_streaming_is_enabled_only_for_non_quiet_text_runs() {
        let text_logger = StructuredLogger::default();
        let jsonl_logger = StructuredLogger::new(crate::structured_log::OutputFormat::Jsonl);

        assert!(should_stream_agent_output_live(false, &text_logger));
        assert!(!should_stream_agent_output_live(true, &text_logger));
        assert!(!should_stream_agent_output_live(false, &jsonl_logger));
    }

    #[tokio::test]
    async fn pull_request_config_uses_defaulted_workflow_params() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let engine = Engine::with_state_adapter(
            Box::new(LocalStateAdapter::with_base_dir(
                temp_dir.path().join("state"),
            )),
            WorkflowRunConfig {
                managed_git: crate::config::ManagedGitSettings {
                    enable_managed_git: true,
                    ..WorkflowRunConfig::default().managed_git
                },
                ..WorkflowRunConfig::default()
            },
        );

        let node = Node {
            id: "apply".to_string(),
            name: "Apply".to_string(),
            description: None,
            r#type: butterflow_models::node::NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![],
            env: HashMap::new(),
            branch_name: Some("codemod-${{ params.target }}".to_string()),
            pull_request: Some(butterflow_models::step::PullRequestConfig {
                title: "Update ${{ params.target }}".to_string(),
                body: Some("Body ${{ params.target }}".to_string()),
                draft: Some(true),
                base: None,
            }),
        };
        let workflow_run_id = Uuid::new_v4();
        let task = Task::new(workflow_run_id, node.id.clone(), false);
        let workflow_run = WorkflowRun {
            id: workflow_run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: Some(butterflow_models::workflow::WorkflowParams {
                    schema: butterflow_models::SimpleSchema {
                        properties: HashMap::from([(
                            "target".to_string(),
                            butterflow_models::SimpleSchemaProperty {
                                name: None,
                                description: None,
                                schema: butterflow_models::SimpleSchemaType::String {
                                    one_of: None,
                                    default: Some("default-target".to_string()),
                                    multi_line: None,
                                    secret: None,
                                },
                            },
                        )]),
                    },
                }),
                templates: vec![],
                nodes: vec![node.clone()],
            },
            status: WorkflowStatus::Running,
            params: HashMap::new(),
            tasks: vec![task.id],
            started_at: Utc::now(),
            ended_at: None,
            bundle_path: None,
            capabilities: None,
            name: None,
            target_path: None,
        };
        let resolved_params = resolve_workflow_run_params(&workflow_run);

        let pr = ManagedGitService::new(&engine)
            .resolve_pull_request_config(&task, &node, &resolved_params)
            .unwrap()
            .expect("managed git node should resolve PR metadata");

        assert_eq!(pr.branch, "codemod-default-target");
        assert_eq!(pr.title, "Update default-target");
        assert_eq!(pr.body.as_deref(), Some("Body default-target"));
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
        let idle_notify = Arc::new(Notify::new());
        let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));

        let local = tokio::task::LocalSet::new();
        let idle_timed_out_for_task = Arc::clone(&idle_timed_out);
        let idle_notify_for_task = Arc::clone(&idle_notify);
        let idle_failure_message_for_task = Arc::clone(&idle_failure_message);
        let progress_state_for_task = Arc::clone(&progress_state);
        let result = local
            .run_until(async move {
                let trigger = tokio::spawn({
                    let idle_timed_out = Arc::clone(&idle_timed_out_for_task);
                    let idle_notify = Arc::clone(&idle_notify_for_task);
                    let idle_failure_message = Arc::clone(&idle_failure_message_for_task);
                    async move {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        idle_timed_out.store(true, Ordering::Release);
                        idle_notify.notify_waiters();
                        if let Ok(mut message) = idle_failure_message.lock() {
                            *message = Some(
                                "No progress observed for 1s while processing src/stalled.ts (execution started, active units: 1)"
                                    .to_string(),
                            );
                        }
                        idle_notify.notify_waiters();
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
                    idle_notify_for_task,
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
}
