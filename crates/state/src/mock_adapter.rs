use std::collections::HashMap;

use uuid::Uuid;

use butterflow_models::{Result, Task, WorkflowRun};

use crate::StateAdapter;

// Mock state adapter for testing
pub struct MockStateAdapter {
    workflow_runs: HashMap<Uuid, WorkflowRun>,
    tasks: HashMap<Uuid, Task>,
    state: HashMap<String, serde_json::Value>,
}

impl Default for MockStateAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MockStateAdapter {
    pub fn new() -> Self {
        Self {
            workflow_runs: HashMap::new(),
            tasks: HashMap::new(),
            state: HashMap::new(),
        }
    }
}

#[async_trait::async_trait]
impl StateAdapter for MockStateAdapter {
    async fn save_workflow_run(&mut self, workflow_run: &WorkflowRun) -> Result<()> {
        self.workflow_runs
            .insert(workflow_run.id, workflow_run.clone());
        Ok(())
    }

    async fn apply_workflow_run_diff(
        &mut self,
        diff: &butterflow_models::WorkflowRunDiff,
    ) -> Result<()> {
        let mut workflow_run = self.get_workflow_run(diff.workflow_run_id).await?;

        for (field, field_diff) in &diff.fields {
            match field_diff.operation {
                butterflow_models::DiffOperation::Add
                | butterflow_models::DiffOperation::Update
                | butterflow_models::DiffOperation::Append => {
                    if let Some(value) = &field_diff.value {
                        let mut workflow_run_value = serde_json::to_value(&workflow_run)?;
                        if let serde_json::Value::Object(obj) = &mut workflow_run_value {
                            obj.insert(field.clone(), value.clone());
                        }
                        workflow_run = serde_json::from_value(workflow_run_value)?;
                    }
                }
                butterflow_models::DiffOperation::Remove => {
                    let mut workflow_run_value = serde_json::to_value(&workflow_run)?;
                    if let serde_json::Value::Object(obj) = &mut workflow_run_value {
                        obj.remove(field);
                    }
                    workflow_run = serde_json::from_value(workflow_run_value)?;
                }
            }
        }

        self.save_workflow_run(&workflow_run).await
    }

    async fn get_workflow_run(&self, workflow_run_id: Uuid) -> Result<WorkflowRun> {
        self.workflow_runs
            .get(&workflow_run_id)
            .cloned()
            .ok_or_else(|| {
                butterflow_models::Error::Other(format!("Workflow run {workflow_run_id} not found"))
            })
    }

    async fn list_workflow_runs(&self, limit: usize) -> Result<Vec<WorkflowRun>> {
        let mut runs: Vec<WorkflowRun> = self.workflow_runs.values().cloned().collect();
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(runs.into_iter().take(limit).collect())
    }

    async fn save_task(&mut self, task: &Task) -> Result<()> {
        self.tasks.insert(task.id, task.clone());

        if let Some(workflow_run) = self.workflow_runs.get_mut(&task.workflow_run_id) {
            if !workflow_run.tasks.contains(&task.id) {
                workflow_run.tasks.push(task.id);
            }
        }

        Ok(())
    }

    async fn apply_task_diff(&mut self, diff: &butterflow_models::TaskDiff) -> Result<()> {
        let mut task = self.get_task(diff.task_id).await?;

        for (field, field_diff) in &diff.fields {
            match field_diff.operation {
                butterflow_models::DiffOperation::Add
                | butterflow_models::DiffOperation::Update
                | butterflow_models::DiffOperation::Append => {
                    if let Some(value) = &field_diff.value {
                        let mut task_value = serde_json::to_value(&task)?;
                        if let serde_json::Value::Object(obj) = &mut task_value {
                            obj.insert(field.clone(), value.clone());
                        }
                        task = serde_json::from_value(task_value)?;
                    }
                }
                butterflow_models::DiffOperation::Remove => {
                    let mut task_value = serde_json::to_value(&task)?;
                    if let serde_json::Value::Object(obj) = &mut task_value {
                        obj.remove(field);
                    }
                    task = serde_json::from_value(task_value)?;
                }
            }
        }

        self.save_task(&task).await
    }

    async fn get_task(&self, task_id: Uuid) -> Result<Task> {
        self.tasks
            .get(&task_id)
            .cloned()
            .ok_or_else(|| butterflow_models::Error::Other(format!("Task {task_id} not found")))
    }

    async fn get_tasks(&self, workflow_run_id: Uuid) -> Result<Vec<Task>> {
        Ok(self
            .tasks
            .values()
            .filter(|t| t.workflow_run_id == workflow_run_id)
            .cloned()
            .collect())
    }

    async fn update_state(
        &mut self,
        _workflow_run_id: Uuid,
        state: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        self.state = state;
        Ok(())
    }

    async fn apply_state_diff(&mut self, diff: &butterflow_models::StateDiff) -> Result<()> {
        let mut state = self.get_state(diff.workflow_run_id).await?;

        for (field, field_diff) in &diff.fields {
            match field_diff.operation {
                butterflow_models::DiffOperation::Add
                | butterflow_models::DiffOperation::Update => {
                    if let Some(value) = &field_diff.value {
                        state.insert(field.clone(), value.clone());
                    }
                }
                butterflow_models::DiffOperation::Remove => {
                    state.remove(field);
                }
                butterflow_models::DiffOperation::Append => {
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

        self.update_state(diff.workflow_run_id, state).await
    }

    async fn get_state(
        &self,
        _workflow_run_id: Uuid,
    ) -> Result<HashMap<String, serde_json::Value>> {
        Ok(self.state.clone())
    }
}
