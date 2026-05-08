use std::collections::HashMap;
use std::sync::Arc;

use butterflow_models::{
    DiffOperation, FieldDiff, Result, Task, TaskDiff, TaskStatus, WorkflowRun, WorkflowRunDiff,
    WorkflowStatus,
};
use butterflow_state::StateAdapter;
use chrono::Utc;
use log::debug;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::workflow_runtime::{publish_event, WorkflowEvent};

#[derive(Clone)]
pub(crate) struct TaskStateService {
    state_adapter: Arc<Mutex<Box<dyn StateAdapter>>>,
}

impl TaskStateService {
    pub(crate) fn new(state_adapter: Arc<Mutex<Box<dyn StateAdapter>>>) -> Self {
        Self { state_adapter }
    }

    pub(crate) async fn append_task_log(
        &self,
        task_id: Uuid,
        message: impl Into<String>,
    ) -> Result<()> {
        let mut adapter = self.state_adapter.lock().await;
        let mut task = adapter.get_task(task_id).await?;
        let message = message.into();
        task.logs.push(message.clone());
        adapter.save_task(&task).await?;
        Self::emit_task_log_appended(task.workflow_run_id, task_id, message);
        Ok(())
    }

    pub(crate) async fn append_task_log_line(&self, task_id: Uuid, line: String) {
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            return;
        }

        let mut adapter = self.state_adapter.lock().await;
        let Ok(mut task) = adapter.get_task(task_id).await else {
            return;
        };
        task.logs.push(line.clone());
        if adapter.save_task(&task).await.is_ok() {
            Self::emit_task_log_appended(task.workflow_run_id, task_id, line);
        }
    }

    pub(crate) async fn mark_running(&self, task_id: Uuid) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            Self::update_json(TaskStatus::Running)?,
        );
        fields.insert("started_at".to_string(), Self::update_json(Utc::now())?);
        fields.insert("ended_at".to_string(), Self::update_null());
        fields.insert("error".to_string(), Self::update_null());
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn mark_awaiting_trigger(&self, task_id: Uuid) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            Self::update_json(TaskStatus::AwaitingTrigger)?,
        );
        fields.insert("ended_at".to_string(), Self::update_null());
        fields.insert("started_at".to_string(), Self::update_null());
        fields.insert("error".to_string(), Self::update_null());
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn mark_failed(
        &self,
        task_id: Uuid,
        error_message: impl Into<String>,
    ) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert("status".to_string(), Self::update_json(TaskStatus::Failed)?);
        fields.insert("ended_at".to_string(), Self::update_json(Utc::now())?);
        fields.insert(
            "error".to_string(),
            FieldDiff {
                operation: DiffOperation::Add,
                value: Some(serde_json::to_value(error_message.into())?),
            },
        );
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn mark_completed(&self, task_id: Uuid) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            Self::update_json(TaskStatus::Completed)?,
        );
        fields.insert("ended_at".to_string(), Self::update_json(Utc::now())?);
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn mark_wont_do(&self, task_id: Uuid) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert("status".to_string(), Self::update_json(TaskStatus::WontDo)?);
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn reset_to_pending(&self, task_id: Uuid) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert(
            "status".to_string(),
            Self::update_json(TaskStatus::Pending)?,
        );
        fields.insert("error".to_string(), Self::update_null());
        fields.insert("ended_at".to_string(), Self::update_null());
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn set_status(&self, task_id: Uuid, status: TaskStatus) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert("status".to_string(), Self::update_json(status)?);
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn set_status_with_ended_at(
        &self,
        task_id: Uuid,
        status: TaskStatus,
        ended_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<Task> {
        let mut fields = HashMap::new();
        fields.insert("status".to_string(), Self::update_json(status)?);
        fields.insert(
            "ended_at".to_string(),
            match ended_at {
                Some(value) => Self::update_json(value)?,
                None => Self::update_null(),
            },
        );
        self.apply_task_fields(task_id, fields).await
    }

    pub(crate) async fn update_matrix_master_status(&self, master_task_id: Uuid) -> Result<()> {
        let adapter = self.state_adapter.lock().await;
        let master_task = adapter.get_task(master_task_id).await?;
        let tasks = adapter.get_tasks(master_task.workflow_run_id).await?;
        drop(adapter);

        let child_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|task| task.master_task_id == Some(master_task_id))
            .collect();

        if child_tasks.is_empty() {
            debug!(
                "No child tasks found for master task {master_task_id}; marking master completed"
            );
            self.set_status_with_ended_at(master_task_id, TaskStatus::Completed, Some(Utc::now()))
                .await?;
            return Ok(());
        }

        let all_terminal = child_tasks.iter().all(|task| {
            matches!(
                task.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::WontDo
            )
        });

        if all_terminal {
            let final_status = if child_tasks
                .iter()
                .any(|task| task.status == TaskStatus::Failed)
            {
                TaskStatus::Failed
            } else {
                TaskStatus::Completed
            };

            debug!(
                "All child tasks for master task {master_task_id} are terminal; setting master status to {final_status:?}"
            );
            self.set_status_with_ended_at(master_task_id, final_status, Some(Utc::now()))
                .await?;
            return Ok(());
        }

        let new_status = if child_tasks
            .iter()
            .any(|task| task.status == TaskStatus::Failed)
        {
            TaskStatus::Failed
        } else if child_tasks
            .iter()
            .any(|task| task.status == TaskStatus::AwaitingTrigger)
        {
            TaskStatus::AwaitingTrigger
        } else if child_tasks
            .iter()
            .any(|task| task.status == TaskStatus::Running)
        {
            TaskStatus::Running
        } else if child_tasks
            .iter()
            .any(|task| task.status == TaskStatus::Pending)
        {
            TaskStatus::Pending
        } else {
            master_task.status
        };

        if new_status == master_task.status {
            debug!("Master task {master_task_id} status {new_status:?} remains unchanged");
            return Ok(());
        }

        debug!(
            "Updating master task {} status from {:?} to {:?}",
            master_task_id, master_task.status, new_status
        );

        let ended_at = match new_status {
            TaskStatus::Pending
            | TaskStatus::Running
            | TaskStatus::AwaitingTrigger
            | TaskStatus::Blocked => None,
            TaskStatus::Completed | TaskStatus::Failed => Some(Utc::now()),
            TaskStatus::WontDo => None,
        };
        self.set_status_with_ended_at(master_task_id, new_status, ended_at)
            .await?;

        Ok(())
    }

    pub(crate) async fn mark_workflow_running(&self, workflow_run_id: Uuid) -> Result<WorkflowRun> {
        self.set_workflow_status(workflow_run_id, WorkflowStatus::Running, None)
            .await
    }

    pub(crate) async fn mark_workflow_awaiting_trigger(
        &self,
        workflow_run_id: Uuid,
    ) -> Result<WorkflowRun> {
        self.set_workflow_status(workflow_run_id, WorkflowStatus::AwaitingTrigger, None)
            .await
    }

    pub(crate) async fn mark_workflow_completed(
        &self,
        workflow_run_id: Uuid,
    ) -> Result<WorkflowRun> {
        self.set_workflow_status(workflow_run_id, WorkflowStatus::Completed, Some(Utc::now()))
            .await
    }

    pub(crate) async fn mark_workflow_failed(&self, workflow_run_id: Uuid) -> Result<WorkflowRun> {
        self.set_workflow_status(workflow_run_id, WorkflowStatus::Failed, Some(Utc::now()))
            .await
    }

    pub(crate) async fn mark_workflow_canceled(
        &self,
        workflow_run_id: Uuid,
    ) -> Result<WorkflowRun> {
        self.set_workflow_status(workflow_run_id, WorkflowStatus::Canceled, Some(Utc::now()))
            .await
    }

    pub(crate) async fn set_workflow_status(
        &self,
        workflow_run_id: Uuid,
        status: WorkflowStatus,
        ended_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<WorkflowRun> {
        let mut fields = HashMap::new();
        fields.insert("status".to_string(), Self::update_json(status)?);
        if let Some(ended_at) = ended_at {
            fields.insert("ended_at".to_string(), Self::update_json(ended_at)?);
        }

        let workflow_run_diff = WorkflowRunDiff {
            workflow_run_id,
            fields,
        };
        let mut adapter = self.state_adapter.lock().await;
        adapter.apply_workflow_run_diff(&workflow_run_diff).await?;
        let workflow_run = adapter.get_workflow_run(workflow_run_id).await?;
        drop(adapter);
        Self::publish_workflow_status_changed(workflow_run_id, workflow_run.status);
        Ok(workflow_run)
    }

    pub(crate) async fn apply_task_fields(
        &self,
        task_id: Uuid,
        fields: HashMap<String, FieldDiff>,
    ) -> Result<Task> {
        let task_diff = TaskDiff { task_id, fields };
        let mut adapter = self.state_adapter.lock().await;
        adapter.apply_task_diff(&task_diff).await?;
        let task = adapter.get_task(task_id).await?;
        drop(adapter);
        Self::publish_task_updated(task.clone());
        Ok(task)
    }

    fn publish_task_updated(task: Task) {
        publish_event(
            task.workflow_run_id,
            WorkflowEvent::TaskUpdated {
                task,
                at: Utc::now(),
            },
        );
    }

    fn publish_workflow_status_changed(workflow_run_id: Uuid, status: WorkflowStatus) {
        publish_event(
            workflow_run_id,
            WorkflowEvent::WorkflowStatusChanged {
                workflow_run_id,
                status,
                at: Utc::now(),
            },
        );
    }

    fn emit_task_log_appended(workflow_run_id: Uuid, task_id: Uuid, line: String) {
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

    fn update_json(value: impl serde::Serialize) -> Result<FieldDiff> {
        Ok(FieldDiff {
            operation: DiffOperation::Update,
            value: Some(serde_json::to_value(value)?),
        })
    }

    fn update_null() -> FieldDiff {
        FieldDiff {
            operation: DiffOperation::Update,
            value: Some(serde_json::Value::Null),
        }
    }
}
