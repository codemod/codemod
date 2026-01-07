use butterflow_models::schema::resolve_values_with_default;
use codemod_ai::execute::{execute_ai_step, ExecuteAiStepConfig};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

use crate::config::{CapabilitiesSecurityCallback, WorkflowRunConfig};
use crate::execution::{CodemodExecutionConfig, PreRunCallback};
use crate::execution_stats::ExecutionStats;
use crate::file_ops::AsyncFileWriter;
use crate::utils::validate_workflow;
use chrono::Utc;
use codemod_sandbox::sandbox::engine::{
    extract_selector_with_quickjs, ExecutionResult, JssgExecutionOptions, SelectorEngineOptions,
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
    evaluate_condition, resolve_string_with_expression, DiffOperation, Error, FieldDiff, Node,
    Result, StateDiff, Strategy, Task, TaskDiff, TaskStatus, Workflow, WorkflowRun,
    WorkflowRunDiff, WorkflowStatus,
};
use butterflow_runners::direct_runner::DirectRunner;
#[cfg(feature = "docker")]
use butterflow_runners::docker_runner::DockerRunner;
#[cfg(feature = "podman")]
use butterflow_runners::podman_runner::PodmanRunner;
use butterflow_runners::Runner;
use butterflow_scheduler::Scheduler;
use butterflow_state::local_adapter::LocalStateAdapter;
use butterflow_state::StateAdapter;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::{
    sandbox::{engine::execution_engine::execute_codemod_with_quickjs, resolvers::OxcResolver},
    utils::project_discovery::find_tsconfig,
};
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;

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

/// Workflow engine
pub struct Engine {
    /// State adapter for persisting workflow state
    state_adapter: Arc<Mutex<Box<dyn StateAdapter>>>,

    scheduler: Scheduler,

    workflow_run_config: WorkflowRunConfig,

    pub execution_stats: Arc<ExecutionStats>,

    /// Async file writer for batched I/O operations
    file_writer: Arc<AsyncFileWriter>,

    /// Notification for when running tasks complete
    task_completion_notify: Arc<Notify>,
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
    pub capabilities_security_callback: Option<Arc<CapabilitiesSecurityCallback>>,
}

impl Engine {
    /// Create a new engine with a local state adapter
    pub fn new() -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> =
            Arc::new(Mutex::new(Box::new(LocalStateAdapter::new())));

        Self {
            state_adapter: Arc::clone(&state_adapter),
            scheduler: Scheduler::new(),
            workflow_run_config: WorkflowRunConfig::default(),
            execution_stats: Arc::new(ExecutionStats::default()),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
        }
    }

    /// Create a new engine with a local state adapter
    pub fn with_workflow_run_config(workflow_run_config: WorkflowRunConfig) -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> =
            Arc::new(Mutex::new(Box::new(LocalStateAdapter::new())));

        Self {
            state_adapter: Arc::clone(&state_adapter),
            scheduler: Scheduler::new(),
            workflow_run_config,
            execution_stats: Arc::new(ExecutionStats::default()),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
        }
    }

    /// Create a new engine with a custom state adapter
    pub fn with_state_adapter(
        state_adapter: Box<dyn StateAdapter>,
        workflow_run_config: WorkflowRunConfig,
    ) -> Self {
        let state_adapter: Arc<Mutex<Box<dyn StateAdapter>>> = Arc::new(Mutex::new(state_adapter));

        Self {
            state_adapter: Arc::clone(&state_adapter),
            scheduler: Scheduler::new(),
            workflow_run_config,
            execution_stats: Arc::new(ExecutionStats::default()),
            file_writer: Arc::new(AsyncFileWriter::new()),
            task_completion_notify: Arc::new(Notify::new()),
        }
    }

    /// Get the workflow file path
    pub fn get_workflow_file_path(&self) -> PathBuf {
        self.workflow_run_config.workflow_file_path.clone()
    }

    /// Spawn a task asynchronously
    async fn spawn_task_with_handle(&self, task_id: Uuid) -> Result<()> {
        let engine = self.clone();
        let task_completion_notify = Arc::clone(&self.task_completion_notify);

        let runtime_handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime_handle.block_on(async move {
                // Always ensure task completion notification is sent, even on panic or hang
                let mut cleanup_guard = TaskCleanupGuard::new(task_completion_notify.clone());

                // Add timeout to prevent infinite hanging
                let task_timeout = tokio::time::Duration::from_secs(45 * 60); // 5 minutes timeout for AI tasks

                match tokio::time::timeout(task_timeout, engine.execute_task(task_id)).await {
                    Ok(Ok(())) => {
                        debug!("Task {} completed successfully", task_id);
                        // Mark guard as sent since execute_task already sent notification
                        cleanup_guard.mark_sent();
                    }
                    Ok(Err(e)) => {
                        error!("Task {} execution failed: {}", task_id, e);
                        // Add error to task logs
                        if let Ok(mut current_task) =
                            engine.state_adapter.lock().await.get_task(task_id).await
                        {
                            current_task
                                .logs
                                .push(format!("Task execution failed: {}", e));
                            let _ = engine
                                .state_adapter
                                .lock()
                                .await
                                .save_task(&current_task)
                                .await;
                        }
                    }
                    Err(_) => {
                        error!(
                            "Task {} timed out after {} seconds",
                            task_id,
                            task_timeout.as_secs()
                        );
                        // Mark task as failed due to timeout
                        if let Err(e) = engine.mark_task_as_failed(task_id, "Task timed out").await
                        {
                            error!("Failed to mark task {} as failed: {}", task_id, e);
                        } else {
                            // Add timeout error to task logs
                            if let Ok(mut current_task) =
                                engine.state_adapter.lock().await.get_task(task_id).await
                            {
                                current_task.logs.push(format!(
                                    "Task timed out after {} seconds",
                                    task_timeout.as_secs()
                                ));
                                let _ = engine
                                    .state_adapter
                                    .lock()
                                    .await
                                    .save_task(&current_task)
                                    .await;
                            }
                        }
                        // Let cleanup guard send notification for timeout case
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

        // Add error to task logs
        let mut current_task = self.state_adapter.lock().await.get_task(task_id).await?;
        current_task.logs.push(format!("Error: {}", error_message));
        self.state_adapter
            .lock()
            .await
            .save_task(&current_task)
            .await?;

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
        let tasks = self.scheduler.calculate_initial_tasks(workflow_run).await?;

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
        target_path: Option<PathBuf>,
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
            target_path,
            tasks: Vec::new(),
            started_at: Utc::now(),
            ended_at: None,
            capabilities: capabilities.cloned(),
        };

        self.state_adapter
            .lock()
            .await
            .save_workflow_run(&workflow_run)
            .await?;

        let engine = self.clone();
        let runtime_handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime_handle.block_on(async move {
                if let Err(e) = engine.execute_workflow(workflow_run_id).await {
                    error!("Workflow execution failed: {e}");
                }
            });
        });

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
                let task_diff = TaskDiff { task_id, fields };

                self.state_adapter
                    .lock()
                    .await
                    .apply_task_diff(&task_diff)
                    .await?;

                if let Err(e) = self.spawn_task_with_handle(task_id).await {
                    error!("Failed to spawn task {}: {}", task_id, e);
                }

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

        let engine = self.clone();
        let runtime_handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime_handle.block_on(async move {
                if let Err(e) = engine.execute_workflow(workflow_run_id).await {
                    error!("Workflow execution failed: {e}");
                }
            });
        });

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
                error!("Failed to spawn task {}: {}", task_id, e);
            }

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

        let engine = self.clone();
        let runtime_handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime_handle.block_on(async move {
                if let Err(e) = engine.execute_workflow(workflow_run_id).await {
                    error!("Workflow execution failed: {e}");
                }
            });
        });
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
    async fn execute_workflow(&self, workflow_run_id: Uuid) -> Result<()> {
        // Get the workflow run
        let workflow_run = self
            .state_adapter
            .lock()
            .await
            .get_workflow_run(workflow_run_id)
            .await?;

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
            let should_recompile = if has_matrix_strategies {
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
                    error!(
                        "Failed during matrix task recompilation for run {workflow_run_id}: {e}"
                    );
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
            for task_id in tasks_to_await_trigger {
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

            let runnable_tasks = runnable_tasks_result.runnable_tasks;

            // Check if any tasks are awaiting trigger
            let awaiting_trigger = tasks_after_recompilation
                .iter()
                .any(|t| t.status == TaskStatus::AwaitingTrigger);
            let any_running = tasks_after_recompilation
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
                let task = tasks_after_recompilation
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
                    error!("Task execution failed: {e}");
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

        info!("Executing task {} ({})", task_id, node.id);

        // Create a runner for this task
        let runner: Box<dyn Runner> = match node
            .runtime
            .as_ref()
            .map(|r| r.r#type)
            .unwrap_or(RuntimeType::Direct)
        {
            RuntimeType::Direct => Box::new(DirectRunner::new()),
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

        // Execute each step in the node
        for step in &node.steps {
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
                )
                .unwrap_or_default();

                if !should_execute {
                    info!(
                        "Skipping step '{}' - condition not met: {}",
                        step.name, condition
                    );
                    continue;
                }
            }

            let result = self
                .execute_step_action(
                    runner.as_ref(),
                    &step.action,
                    &step.env,
                    &step.id,
                    &step.name,
                    node,
                    &task,
                    &resolved_params,
                    &state,
                    &workflow_run.workflow,
                    &workflow_run.bundle_path,
                    &[],
                    &self.workflow_run_config.capabilities,
                )
                .await;

            match result {
                Ok(_) => {}
                Err(e) => {
                    // Get current task to add error to logs
                    let mut current_task =
                        self.state_adapter.lock().await.get_task(task_id).await?;
                    let error_msg = format!("Step {} failed: {}", step.name, e);
                    current_task.logs.push(error_msg.clone());
                    self.state_adapter
                        .lock()
                        .await
                        .save_task(&current_task)
                        .await?;

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
                            value: Some(serde_json::to_value(error_msg.clone())?),
                        },
                    );
                    let task_diff = TaskDiff { task_id, fields };

                    // Apply the diff
                    self.state_adapter
                        .lock()
                        .await
                        .apply_task_diff(&task_diff)
                        .await?;

                    error!(
                        "Task {} ({}) step {} failed: {}",
                        task_id, node.id, step.name, e
                    );

                    return Err(e);
                }
            }
        }

        // Prepare environment variables
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
        self.state_adapter
            .lock()
            .await
            .apply_task_diff(&task_diff)
            .await?;

        info!("Task {} ({}) completed", task_id, node.id);

        // If this is a matrix task, update the master task status
        if let Some(master_task_id) = task.master_task_id {
            self.update_matrix_master_status(master_task_id).await?;
        }

        // Notify that a task has completed (for event-driven waiting)
        self.task_completion_notify.notify_one();

        Ok(())
    }

    /// Execute a specific step action with dependency chain tracking for cycle detection
    #[allow(clippy::too_many_arguments)]
    async fn execute_step_action(
        &self,
        runner: &dyn Runner,
        action: &StepAction,
        step_env: &Option<HashMap<String, String>>,
        step_id: &Option<String>,
        step_name: &str,
        node: &Node,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        workflow: &Workflow,
        bundle_path: &Option<PathBuf>,
        dependency_chain: &[CodemodDependency],
        capabilities: &Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<()> {
        match action {
            StepAction::RunScript(run) => {
                self.execute_run_script_step(
                    runner,
                    run,
                    step_env,
                    node,
                    task,
                    params,
                    state,
                    bundle_path,
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
                        &template_step.env,
                        &template_step.id,
                        &template_step.name,
                        node,
                        task,
                        &combined_params,
                        state,
                        workflow,
                        bundle_path,
                        dependency_chain,
                        capabilities,
                    ))
                    .await?;
                }
                Ok(())
            }
            StepAction::AstGrep(ast_grep) => {
                self.execute_ast_grep_step(node.id.clone(), step_name, ast_grep, task)
                    .await
            }
            StepAction::JSAstGrep(js_ast_grep) => {
                self.execute_js_ast_grep_step(
                    node.id.clone(),
                    step_id.clone().unwrap_or_default(),
                    step_name,
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
                            .as_ref()
                            .map(|callback| Arc::new(callback.clone())),
                    },
                    bundle_path,
                    task,
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
                ))
                .await
            }
            StepAction::AI(ai_config) => {
                self.execute_ai_step(ai_config, step_env, node, task, params, state)
                    .await
            }
        }
    }

    pub async fn execute_ast_grep_step(
        &self,
        id: String,
        step_name: &str,
        ast_grep: &UseAstGrep,
        task: &Task,
    ) -> Result<()> {
        let bundle_path = self.workflow_run_config.bundle_path.clone();

        let config_path = bundle_path.join(&ast_grep.config_file);

        if !config_path.exists() {
            let error_msg = format!("AST grep config file not found: {}", config_path.display());
            return Err(Error::StepExecution(error_msg));
        }

        if let Some(pre_run_callback) = self.workflow_run_config.pre_run_callback.as_ref() {
            pre_run_callback(
                &self.workflow_run_config.target_path,
                self.workflow_run_config.dry_run,
            );
        }

        // Format execution summary
        let execution_summary = format!(
            "\x1b[32mStep {} completed\x1b[0m:\n\r{}",
            step_name, self.execution_stats
        );
        let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
        current_task.logs.push(execution_summary);
        self.state_adapter
            .lock()
            .await
            .save_task(&current_task)
            .await?;

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

                let _ = execution_config.execute(|path, config| {
                    // Only process files, not directories
                    if !path.is_file() {
                        return;
                    }

                    info!("Executing AST grep on file: {}", path.display());

                    // Execute ast-grep on this file
                    match scan_file_with_combined_scan(
                        path,
                        &combined_scan_with_rule.combined_scan,
                        !config.dry_run, // apply_fixes = !dry_run
                    ) {
                        Ok((matches, file_modified, new_content)) => {
                            if !matches.is_empty() {
                                info!("Found {} matches in {}", matches.len(), path.display());
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
                                        let error_msg = format!(
                                            "Failed to write modified file {}: {}",
                                            path.display(),
                                            e
                                        );
                                        error!("{}", error_msg);
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
                            let error_msg = format!("AST grep execution error: {}", e);
                            error!("{}", error_msg);
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
        step_name: &str,
        js_ast_grep: &UseJSAstGrep,
        params: Option<HashMap<String, serde_json::Value>>,
        matrix_input: Option<HashMap<String, serde_json::Value>>,
        capabilities_data: &CapabilitiesData,
        bundle_path: &Option<PathBuf>,
        task: &Task,
    ) -> Result<()> {
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
            let error_msg = format!(
                "JavaScript file '{}' does not exist",
                js_file_path.display()
            );
            // Add error to task logs
            let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
            current_task.logs.push(error_msg.clone());
            self.state_adapter
                .lock()
                .await
                .save_task(&current_task)
                .await?;
            return Err(Error::StepExecution(error_msg));
        }

        let script_base_dir = js_file_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let tsconfig_path = find_tsconfig(&script_base_dir);

        let resolver = Arc::new(
            OxcResolver::new(script_base_dir.clone(), tsconfig_path)
                .map_err(|e| Error::Other(format!("Failed to create resolver: {}", e)))?,
        );

        let capabilities_security_callback_clone =
            capabilities_data.capabilities_security_callback.clone();
        let pre_run_callback = PreRunCallback {
            callback: Arc::new(Box::new(move |_, _, config: &CodemodExecutionConfig| {
                if let Some(callback) = &capabilities_security_callback_clone {
                    callback(config).unwrap_or_else(|e| {
                        error!("Failed to check capabilities: {e}");
                        std::process::exit(1);
                    });
                }
            })),
        };
        let config = CodemodExecutionConfig {
            pre_run_callback: Some(pre_run_callback),
            progress_callback: self.workflow_run_config.progress_callback.clone(),
            target_path: Some(target_path.clone()),
            base_path: None,
            include_globs: js_ast_grep.include.as_deref().map(|v| v.to_vec()),
            exclude_globs: js_ast_grep.exclude.as_deref().map(|v| v.to_vec()),
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
            lang_str.parse().map_err(|e| {
                Error::StepExecution(format!("Invalid language '{lang_str}': {}", e))
            })?
        } else {
            // Parse TypeScript as default
            "typescript".parse().map_err(|e| {
                Error::StepExecution(format!("Failed to parse default language: {}", e))
            })?
        };

        // Create console log collector for selector extraction
        let selector_log_collector = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let selector_log_collector_clone = Arc::clone(&selector_log_collector);

        let selector_config = match extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &js_file_path,
            language,
            resolver: Arc::clone(&resolver),
            capabilities: capabilities_data
                .capabilities
                .as_ref()
                .map(|v| v.clone().into_iter().collect()),
            console_log_collector: Some(Box::new(move |message| {
                selector_log_collector_clone.lock().unwrap().push(message);
            })),
        })
        .await
        {
            Ok(config) => config,
            Err(e) => {
                let error_msg = format!("Failed to extract selector: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::StepExecution(error_msg));
            }
        };

        // Append selector extraction logs to task
        let selector_logs = selector_log_collector.lock().unwrap().clone();
        if !selector_logs.is_empty() {
            let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
            for log in selector_logs {
                current_task.logs.push(log);
            }
            self.state_adapter
                .lock()
                .await
                .save_task(&current_task)
                .await?;
        }

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
                                debug!(
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

        // Create console log collector to capture console.log/warn/error output
        let console_log_collector = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        // Capture variables for use in parallel threads
        let runtime_handle = tokio::runtime::Handle::current();
        let js_file_path_clone = js_file_path.clone();
        let resolver_clone = resolver.clone();
        let id_clone = Arc::new(id);
        let progress_callback = self.workflow_run_config.progress_callback.clone();
        let file_writer = Arc::clone(&self.file_writer);
        let selector_config = selector_config.map(Arc::new);
        let console_log_collector_clone = Arc::clone(&console_log_collector);

        // Execute the codemod on each file using the config's multi-threading
        config
            .execute(move |file_path, config| {
                // Only process files
                if !file_path.is_file() {
                    return;
                }

                // Read file content synchronously
                let content = match std::fs::read_to_string(file_path) {
                    Ok(content) => content,
                    Err(e) => {
                        warn!("Failed to read file {}: {}", file_path.display(), e);
                        return;
                    }
                };

                // Execute the async codemod using the captured runtime handle
                std::env::set_var("CODEMOD_STEP_ID", &step_id);
                let console_log_collector_for_callback = Arc::clone(&console_log_collector_clone);
                let execution_result = runtime_handle.block_on(async {
                    execute_codemod_with_quickjs(JssgExecutionOptions {
                        script_path: &js_file_path_clone,
                        resolver: resolver_clone.clone(),
                        language,
                        file_path,
                        content: &content,
                        selector_config: selector_config.clone(),
                        params: params.clone(),
                        matrix_values: matrix_input.clone(),
                        capabilities: config.capabilities.clone(),
                        semantic_provider: semantic_provider.clone(),
                        console_log_collector: Some(Box::new(move |message| {
                            console_log_collector_for_callback
                                .lock()
                                .unwrap()
                                .push(message);
                        })),
                    })
                    .await
                });

                match execution_result {
                    Ok(execution_output) => {
                        match execution_output {
                            ExecutionResult::Modified(ref new_content) => {
                                if config.dry_run {
                                    self.execution_stats
                                        .files_modified
                                        .fetch_add(1, Ordering::Relaxed);

                                    debug!("Would modify file (dry run): {}", file_path.display());
                                } else {
                                    // Use async file writing to avoid blocking the thread
                                    let write_result = runtime_handle.block_on(async {
                                        file_writer
                                            .write_file(
                                                file_path.to_path_buf(),
                                                new_content.clone(),
                                            )
                                            .await
                                    });

                                    if let Err(e) = write_result {
                                        let error_msg = format!(
                                            "Failed to write modified file {}: {}",
                                            file_path.display(),
                                            e
                                        );
                                        error!("{}", error_msg);
                                        // Note: Cannot access task logs here as we're in a closure
                                        // The error is already logged via error! macro
                                        self.execution_stats
                                            .files_with_errors
                                            .fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        debug!("Modified file: {}", file_path.display());
                                        if let Some(ref provider) = semantic_provider {
                                            let _ = provider
                                                .notify_file_processed(file_path, new_content);
                                        }
                                        self.execution_stats
                                            .files_modified
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            ExecutionResult::Unmodified | ExecutionResult::Skipped => {
                                self.execution_stats
                                    .files_unmodified
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => {
                        let error_msg = format!(
                            "Failed to execute codemod on {}: {:?}",
                            file_path.display(),
                            e
                        );
                        error!("{}", error_msg);
                        // Note: Cannot access task logs here as we're in a closure
                        // The error is already logged via error! macro
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
            .map_err(|e| Error::StepExecution(e.to_string()))?;

        // Get the current task and append execution summary logs
        let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;

        // Collect console logs from JavaScript execution
        let console_logs = console_log_collector.lock().unwrap().clone();
        for log in console_logs {
            current_task.logs.push(log.clone());
        }

        // Format execution summary
        let execution_summary = format!(
            "\x1b[32mStep {} completed\x1b[0m:\n\r{}",
            step_name, self.execution_stats
        );
        current_task.logs.push(execution_summary);

        // Save the updated task
        self.state_adapter
            .lock()
            .await
            .save_task(&current_task)
            .await?;

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
    ) -> Result<()> {
        // Resolve the prompt with parameters, state, and matrix values
        // TODO: Load step outputs from STEP_OUTPUTS file and pass here
        let resolved_prompt = resolve_string_with_expression(
            &ai_config.prompt,
            params,
            state,
            task.matrix_values.as_ref(),
            None, // step outputs
        )?;

        info!("Executing AI agent step with prompt: {}", resolved_prompt);

        // Configure LLM settings
        let api_key = match ai_config
            .api_key
            .clone()
            .or_else(|| std::env::var("LLM_API_KEY").ok())
        {
            Some(key) => key,
            None => {
                let error_msg =
                    "AI API key not provided and not found in environment variables (LLM_API_KEY)"
                        .to_string();
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::StepExecution(error_msg));
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
            enable_lakeview: ai_config.enable_lakeview,
            prompt: resolved_prompt,
            working_dir: self.workflow_run_config.target_path.clone(),
            llm_protocol: llm_provider,
        };

        let output = match execute_ai_step(config).await {
            Ok(output) => output,
            Err(e) => {
                let error_msg = format!("AI step execution failed: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::StepExecution(error_msg));
            }
        };

        println!("AI agent output:\n{}", output.data.unwrap_or_default());
        info!("AI agent step completed successfully");
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
    ) -> Result<()> {
        info!("Executing codemod step: {}", codemod.source);

        // Check for runtime cycles before execution
        if let Some(cycle_start) = self.find_cycle_in_chain(&codemod.source, dependency_chain) {
            let chain_str = dependency_chain
                .iter()
                .map(|d| d.source.as_str())
                .collect::<Vec<_>>()
                .join(" → ");

            let error_msg = format!(
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
            );
            // Add error to task logs
            let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
            current_task.logs.push(error_msg.clone());
            self.state_adapter
                .lock()
                .await
                .save_task(&current_task)
                .await?;
            return Err(Error::Other(error_msg));
        }

        // Resolve the package (local path or registry package)
        let resolved_package = match self
            .workflow_run_config
            .registry_client
            .resolve_package(&codemod.source, None, false, None)
            .await
        {
            Ok(pkg) => pkg,
            Err(e) => {
                let error_msg = format!("Failed to resolve package: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::Other(error_msg));
            }
        };

        info!(
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
    ) -> Result<()> {
        let workflow_path = resolved_package.package_dir.join("workflow.yaml");

        if !workflow_path.exists() {
            let error_msg = format!(
                "Workflow file not found in codemod package: {}",
                workflow_path.display()
            );
            // Add error to task logs
            let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
            current_task.logs.push(error_msg.clone());
            self.state_adapter
                .lock()
                .await
                .save_task(&current_task)
                .await?;
            return Err(Error::Other(error_msg));
        }

        // Load the codemod workflow
        let workflow_content = match std::fs::read_to_string(&workflow_path) {
            Ok(content) => content,
            Err(e) => {
                let error_msg = format!("Failed to read workflow file: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::Other(error_msg));
            }
        };

        let codemod_workflow: Workflow = match serde_yaml::from_str(&workflow_content) {
            Ok(workflow) => workflow,
            Err(e) => {
                let error_msg = format!("Failed to parse workflow YAML: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::Other(error_msg));
            }
        };

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

        info!(
            "Running codemod workflow: {} with {} parameters",
            resolved_package.spec.name,
            codemod_params.len()
        );

        // Execute the codemod workflow synchronously by running its steps directly
        // This avoids the recursive engine execution cycle
        info!("Executing codemod workflow steps directly");

        // Create a direct runner for executing the codemod steps
        let runner: Box<dyn Runner> = Box::new(DirectRunner::new());

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
                    )?;
                    if !should_execute {
                        info!(
                            "Skipping codemod step '{}' - condition not met: {}",
                            step.name, condition
                        );
                        continue;
                    }
                }

                Box::pin(self.execute_step_action(
                    runner.as_ref(),
                    &step.action,
                    &step.env,
                    &step.id,
                    &step.name,
                    node,
                    task, // Use the current task context
                    params,
                    state,
                    &codemod_workflow,
                    &Some(resolved_package.package_dir.clone()),
                    dependency_chain,
                    capabilities,
                ))
                .await?;
            }
        }

        info!("Codemod workflow completed successfully");
        Ok(())
    }

    /// Execute a single RunScript step
    #[allow(clippy::too_many_arguments)]
    async fn execute_run_script_step(
        &self,
        runner: &dyn Runner,
        run: &str,
        step_env: &Option<HashMap<String, String>>,
        node: &Node,
        task: &Task,
        params: &HashMap<String, serde_json::Value>,
        state: &HashMap<String, serde_json::Value>,
        bundle_path: &Option<PathBuf>,
    ) -> Result<()> {
        // Start with a copy of the parent process's environment
        let mut env: HashMap<String, String> = std::env::vars().collect();

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

        env.insert(
            String::from("CODEMOD_STATE"),
            serde_json::to_string(state).unwrap_or("".to_string()),
        );

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
        match File::create(&state_outputs_path) {
            Ok(_) => {}
            Err(e) => {
                let error_msg = format!("Failed to create state outputs file: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::Other(error_msg));
            }
        }

        if let Some(bundle_path) = bundle_path {
            env.insert(
                String::from("CODEMOD_PATH"),
                bundle_path.to_str().unwrap_or("").to_string(),
            );
        }

        env.insert(
            String::from("CODEMOD_TARGET_PATH"),
            self.workflow_run_config
                .target_path
                .to_str()
                .unwrap_or("")
                .to_string(),
        );

        let canonical_path = match state_outputs_path.canonicalize() {
            Ok(path) => path,
            Err(e) => {
                let error_msg = format!("Failed to canonicalize state outputs path: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(Error::Other(error_msg));
            }
        };
        env.insert(
            String::from("STATE_OUTPUTS"),
            canonical_path
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

        // Resolve variables
        // TODO: Load step outputs from STEP_OUTPUTS file and pass here
        let resolved_command =
            resolve_string_with_expression(run, params, state, task.matrix_values.as_ref(), None)?;

        // Execute the command
        let output = match runner.run_command(&resolved_command, &env).await {
            Ok(output) => output,
            Err(e) => {
                let error_msg = format!("Failed to execute command: {}", e);
                // Add error to task logs
                let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;
                current_task.logs.push(error_msg.clone());
                self.state_adapter
                    .lock()
                    .await
                    .save_task(&current_task)
                    .await?;
                return Err(e);
            }
        };

        // Get the current task
        let mut current_task = self.state_adapter.lock().await.get_task(task.id).await?;

        // Append to the logs
        current_task.logs.push(output.clone());

        // Save the updated task
        self.state_adapter
            .lock()
            .await
            .save_task(&current_task)
            .await?;

        println!("Step output:");
        println!("{output}");

        let outputs = read_to_string(&state_outputs_path).await?;

        // Clean up the temporary file
        std::fs::remove_file(&state_outputs_path).ok();

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

        self.state_adapter
            .lock()
            .await
            .apply_state_diff(&StateDiff {
                workflow_run_id: task.workflow_run_id,
                fields: state_diff,
            })
            .await?;
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

        // If there are no child tasks (e.g., state was empty or cleared), the master should reflect that.
        if child_tasks.is_empty() {
            debug!("No child tasks found for master task {master_task_id}, setting status to Completed (or Pending if master just created).");
            let final_status = if master_task.status == TaskStatus::Pending {
                // If the master was just created and has no children yet (empty state)
                // Keep it Pending until state potentially provides children.
                // Or should it be Completed? Let's try Completed.
                TaskStatus::Completed // Or Pending? Needs careful consideration. Let's assume Completed for empty state.
            } else {
                TaskStatus::Completed // If children existed and were removed, it's Completed.
            };

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

            // Clear ended_at if moving away from a terminal state (e.g., Failed -> Running if retried, although retry isn't implemented here)
            // Or add ended_at if moving *to* Failed from a non-terminal state
            if new_status == TaskStatus::Failed
                && !matches!(
                    master_task.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::WontDo
                )
            {
                fields.insert(
                    "ended_at".to_string(),
                    FieldDiff {
                        operation: DiffOperation::Update, // Add or update ended_at
                        value: Some(serde_json::to_value(Utc::now())?),
                    },
                );
            } else if matches!(
                master_task.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::WontDo
            ) && new_status != TaskStatus::Failed
            {
                // If moving from terminal (except Failed) to non-terminal, clear ended_at? Or is this impossible?
                // For now, only add ended_at when entering Failed/Completed.
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
            file_writer: Arc::clone(&self.file_writer),
            task_completion_notify: Arc::clone(&self.task_completion_notify),
        }
    }
}
