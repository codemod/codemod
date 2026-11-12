use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use butterflow_models::{Result, StateDiff, Task, TaskDiff, WorkflowRun, WorkflowRunDiff};

pub mod cloud_adapter;
pub mod local_adapter;
pub mod mock_adapter;
pub mod run_store;

/// State adapter trait for persisting workflow state
#[async_trait]
pub trait StateAdapter: Send + Sync {
    /// Save a workflow run
    async fn save_workflow_run(&mut self, workflow_run: &WorkflowRun) -> Result<()>;

    /// Apply a diff to a workflow run
    async fn apply_workflow_run_diff(&mut self, diff: &WorkflowRunDiff) -> Result<()>;

    /// Get a workflow run
    async fn get_workflow_run(&self, workflow_run_id: Uuid) -> Result<WorkflowRun>;

    /// List workflow runs
    async fn list_workflow_runs(&self, limit: usize) -> Result<Vec<WorkflowRun>>;

    /// Save a task
    async fn save_task(&mut self, task: &Task) -> Result<()>;

    /// Apply a diff to a task
    async fn apply_task_diff(&mut self, diff: &TaskDiff) -> Result<()>;

    /// Get a task
    async fn get_task(&self, task_id: Uuid) -> Result<Task>;

    /// Get all tasks for a workflow run
    async fn get_tasks(&self, workflow_run_id: Uuid) -> Result<Vec<Task>>;

    /// Update workflow state
    async fn update_state(
        &mut self,
        workflow_run_id: Uuid,
        state: HashMap<String, Value>,
    ) -> Result<()>;

    /// Apply a diff to workflow state
    async fn apply_state_diff(&mut self, diff: &StateDiff) -> Result<()>;

    /// Get workflow state
    async fn get_state(&self, workflow_run_id: Uuid) -> Result<HashMap<String, Value>>;
}
