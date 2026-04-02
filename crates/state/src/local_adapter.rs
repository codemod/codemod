use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use butterflow_models::{
    DiffOperation, Error, Result, StateDiff, Task, TaskDiff, WorkflowRun, WorkflowRunDiff,
};

use crate::StateAdapter;

/// Local state adapter (stores state in local files)
pub struct LocalStateAdapter {
    /// Base directory for storing state
    base_dir: PathBuf,

    /// Workflow runs
    workflow_runs: HashMap<Uuid, WorkflowRun>,

    /// Tasks
    tasks: HashMap<Uuid, Task>,
}

impl Default for LocalStateAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalStateAdapter {
    /// Create a new local state adapter
    pub fn new() -> Self {
        // Get the data directory
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("butterflow");

        // Create the data directory if it doesn't exist
        fs::create_dir_all(&data_dir).unwrap_or_else(|e| {
            eprintln!("Failed to create data directory: {e}");
        });

        Self {
            base_dir: data_dir,
            workflow_runs: HashMap::new(),
            tasks: HashMap::new(),
        }
    }

    /// Create a new local state adapter with a custom base directory
    pub fn with_base_dir<P: AsRef<Path>>(base_dir: P) -> Self {
        // Create the base directory if it doesn't exist
        fs::create_dir_all(&base_dir).unwrap_or_else(|e| {
            eprintln!("Failed to create base directory: {e}");
        });

        Self {
            base_dir: base_dir.as_ref().to_path_buf(),
            workflow_runs: HashMap::new(),
            tasks: HashMap::new(),
        }
    }

    /// Get the path to a workflow run file
    fn workflow_run_path(&self, workflow_run_id: Uuid) -> PathBuf {
        self.base_dir
            .join("workflow_runs")
            .join(format!("{workflow_run_id}.json"))
    }

    /// Get the path to a task file
    fn task_path(&self, task_id: Uuid) -> PathBuf {
        self.base_dir.join("tasks").join(format!("{task_id}.json"))
    }

    /// Get the path to a workflow state file
    fn state_path(&self, workflow_run_id: Uuid) -> PathBuf {
        self.base_dir
            .join("state")
            .join(format!("{workflow_run_id}.json"))
    }

    /// Load a workflow run from disk
    fn load_workflow_run(&self, workflow_run_id: Uuid) -> Result<WorkflowRun> {
        let path = self.workflow_run_path(workflow_run_id);

        // Check if the file exists
        if !path.exists() {
            return Err(Error::Other(format!(
                "Workflow run {workflow_run_id} not found"
            )));
        }

        // Read the file
        let content = fs::read_to_string(path)?;

        // Parse the JSON
        let workflow_run = serde_json::from_str(&content)?;

        Ok(workflow_run)
    }

    /// Load a task from disk
    fn load_task(&self, task_id: Uuid) -> Result<Task> {
        let path = self.task_path(task_id);

        // Check if the file exists
        if !path.exists() {
            return Err(Error::Other(format!("Task {task_id} not found")));
        }

        // Read the file
        let content = fs::read_to_string(path)?;

        // Parse the JSON
        let task = serde_json::from_str(&content)?;

        Ok(task)
    }

    /// Load workflow state from disk
    fn load_state(&self, workflow_run_id: Uuid) -> Result<HashMap<String, Value>> {
        let path = self.state_path(workflow_run_id);

        // Check if the file exists
        if !path.exists() {
            return Ok(HashMap::new());
        }

        // Read the file
        let content = fs::read_to_string(path)?;

        // Parse the JSON
        let state = serde_json::from_str(&content)?;

        Ok(state)
    }
}

#[async_trait]
impl StateAdapter for LocalStateAdapter {
    async fn save_workflow_run(&mut self, workflow_run: &WorkflowRun) -> Result<()> {
        // Create the workflow_runs directory if it doesn't exist
        let dir = self.base_dir.join("workflow_runs");
        fs::create_dir_all(&dir)?;

        // Serialize the workflow run
        let json = serde_json::to_string_pretty(workflow_run)?;

        // Write to disk
        let path = self.workflow_run_path(workflow_run.id);
        fs::write(path, json)?;

        // Update in-memory cache
        self.workflow_runs
            .insert(workflow_run.id, workflow_run.clone());

        Ok(())
    }

    async fn apply_workflow_run_diff(&mut self, diff: &WorkflowRunDiff) -> Result<()> {
        // Get the workflow run
        let mut workflow_run = self.get_workflow_run(diff.workflow_run_id).await?;

        // Apply the diff
        for (field, field_diff) in &diff.fields {
            match field_diff.operation {
                DiffOperation::Add | DiffOperation::Update | DiffOperation::Append => {
                    if let Some(value) = &field_diff.value {
                        // Convert the workflow run to a JSON value
                        let mut workflow_run_value = serde_json::to_value(&workflow_run)?;

                        // Update the field
                        if let serde_json::Value::Object(obj) = &mut workflow_run_value {
                            obj.insert(field.clone(), value.clone());
                        }

                        // Convert back to a workflow run
                        workflow_run = serde_json::from_value(workflow_run_value)?;
                    }
                }
                DiffOperation::Remove => {
                    // Convert the workflow run to a JSON value
                    let mut workflow_run_value = serde_json::to_value(&workflow_run)?;

                    // Remove the field
                    if let serde_json::Value::Object(obj) = &mut workflow_run_value {
                        obj.remove(field);
                    }

                    // Convert back to a workflow run
                    workflow_run = serde_json::from_value(workflow_run_value)?;
                }
            }
        }

        // Save the updated workflow run
        self.save_workflow_run(&workflow_run).await
    }

    async fn get_workflow_run(&self, workflow_run_id: Uuid) -> Result<WorkflowRun> {
        // Check if the workflow run is in the cache
        if let Some(workflow_run) = self.workflow_runs.get(&workflow_run_id) {
            return Ok(workflow_run.clone());
        }

        // Load from disk
        self.load_workflow_run(workflow_run_id)
    }

    async fn list_workflow_runs(&self, limit: usize) -> Result<Vec<WorkflowRun>> {
        // Create the workflow_runs directory if it doesn't exist
        let dir = self.base_dir.join("workflow_runs");
        fs::create_dir_all(&dir)?;

        // List all files in the directory
        let mut entries = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .map(|ext| ext == "json")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        // Sort by modification time (newest first)
        entries.sort_by(|a, b| {
            let a_time = a
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or_else(|_| std::time::SystemTime::now());
            let b_time = b
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or_else(|_| std::time::SystemTime::now());
            b_time.cmp(&a_time)
        });

        // Load the workflow runs
        let mut workflow_runs = Vec::new();
        for entry in entries.iter().take(limit) {
            let path = entry.path();
            let file_name = path.file_stem().unwrap().to_string_lossy();
            if let Ok(workflow_run_id) = Uuid::parse_str(&file_name) {
                if let Ok(workflow_run) = self.load_workflow_run(workflow_run_id) {
                    workflow_runs.push(workflow_run);
                }
            }
        }

        Ok(workflow_runs)
    }

    async fn save_task(&mut self, task: &Task) -> Result<()> {
        // Create the tasks directory if it doesn't exist
        let dir = self.base_dir.join("tasks");
        fs::create_dir_all(&dir)?;

        // Serialize the task
        let json = serde_json::to_string_pretty(task)?;

        // Write to disk
        let path = self.task_path(task.id);
        fs::write(path, json)?;

        // Update in-memory cache
        self.tasks.insert(task.id, task.clone());

        // Keep a per-run task index on the workflow run so callers do not need
        // to scan the entire task store on every refresh.
        if let Ok(mut workflow_run) = self.get_workflow_run(task.workflow_run_id).await {
            if !workflow_run.tasks.contains(&task.id) {
                workflow_run.tasks.push(task.id);
                self.save_workflow_run(&workflow_run).await?;
            }
        }

        Ok(())
    }

    async fn get_task(&self, task_id: Uuid) -> Result<Task> {
        // Check if the task is in the cache
        if let Some(task) = self.tasks.get(&task_id) {
            return Ok(task.clone());
        }

        // Load from disk
        self.load_task(task_id)
    }

    async fn get_tasks(&self, workflow_run_id: Uuid) -> Result<Vec<Task>> {
        if let Ok(workflow_run) = self.get_workflow_run(workflow_run_id).await {
            if !workflow_run.tasks.is_empty() {
                let mut tasks = Vec::with_capacity(workflow_run.tasks.len());
                for task_id in workflow_run.tasks {
                    if let Ok(task) = self.get_task(task_id).await {
                        tasks.push(task);
                    }
                }
                return Ok(tasks);
            }
        }

        // Backward-compatible fallback for legacy runs that do not have their
        // task index populated yet.
        let dir = self.base_dir.join("tasks");
        fs::create_dir_all(&dir)?;

        let mut tasks = Vec::new();
        for entry in fs::read_dir(dir)?.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
            {
                continue;
            }

            let file_name = path.file_stem().unwrap().to_string_lossy();
            if let Ok(task_id) = Uuid::parse_str(&file_name) {
                if let Ok(task) = self.load_task(task_id) {
                    if task.workflow_run_id == workflow_run_id {
                        tasks.push(task);
                    }
                }
            }
        }

        Ok(tasks)
    }

    async fn update_state(
        &mut self,
        workflow_run_id: Uuid,
        state: HashMap<String, Value>,
    ) -> Result<()> {
        // Create the state directory if it doesn't exist
        let dir = self.base_dir.join("state");
        fs::create_dir_all(&dir)?;

        // Serialize the state
        let json = serde_json::to_string_pretty(&state)?;

        // Write to disk
        let path = self.state_path(workflow_run_id);
        fs::write(path, json)?;

        Ok(())
    }

    async fn apply_task_diff(&mut self, diff: &TaskDiff) -> Result<()> {
        // Get the task
        let mut task = self.get_task(diff.task_id).await?;

        // Apply the diff
        for (field, field_diff) in &diff.fields {
            match field_diff.operation {
                DiffOperation::Add | DiffOperation::Update | DiffOperation::Append => {
                    if let Some(value) = &field_diff.value {
                        // Convert the task to a JSON value
                        let mut task_value = serde_json::to_value(&task)?;

                        // Update the field
                        if let serde_json::Value::Object(obj) = &mut task_value {
                            obj.insert(field.clone(), value.clone());
                        }

                        // Convert back to a task
                        task = serde_json::from_value(task_value)?;
                    }
                }
                DiffOperation::Remove => {
                    // Convert the task to a JSON value
                    let mut task_value = serde_json::to_value(&task)?;

                    // Remove the field
                    if let serde_json::Value::Object(obj) = &mut task_value {
                        obj.remove(field);
                    }

                    // Convert back to a task
                    task = serde_json::from_value(task_value)?;
                }
            }
        }

        // Save the updated task
        self.save_task(&task).await
    }

    async fn apply_state_diff(&mut self, diff: &StateDiff) -> Result<()> {
        // Get the current state
        let mut state = self.get_state(diff.workflow_run_id).await?;

        // Apply the diff
        for (field, field_diff) in &diff.fields {
            match field_diff.operation {
                DiffOperation::Add | DiffOperation::Update => {
                    if let Some(value) = &field_diff.value {
                        state.insert(field.clone(), value.clone());
                    }
                }
                DiffOperation::Remove => {
                    state.remove(field);
                }
                DiffOperation::Append => {
                    // TODO: We may want reconsider this when we add state validation
                    if let Some(new_value) = &field_diff.value {
                        if let Some(existing) = state.get_mut(field) {
                            // If the existing value is an array, append to it
                            if let serde_json::Value::Array(arr) = existing {
                                arr.push(new_value.clone());
                            } else {
                                // If the existing value is not an array, replace it with a new array containing both values
                                let old_value = existing.clone();
                                *existing =
                                    serde_json::Value::Array(vec![old_value, new_value.clone()]);
                            }
                        } else {
                            // Field doesn't exist yet, create a new array with just this value
                            state.insert(
                                field.clone(),
                                serde_json::Value::Array(vec![new_value.clone()]),
                            );
                        }
                    }
                }
            }
        }

        // Save the updated state
        self.update_state(diff.workflow_run_id, state).await
    }

    async fn get_state(&self, workflow_run_id: Uuid) -> Result<HashMap<String, Value>> {
        self.load_state(workflow_run_id)
    }
}
