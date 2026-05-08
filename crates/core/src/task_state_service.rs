use std::collections::HashMap;
use std::sync::Arc;

use butterflow_models::{
    DiffOperation, FieldDiff, Result, Task, TaskDiff, TaskStatus, WorkflowRun, WorkflowRunDiff,
    WorkflowStatus,
};
use butterflow_state::StateAdapter;
use chrono::Utc;
use log::debug;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::workflow_runtime::{publish_event, WorkflowEvent};

#[derive(Clone)]
pub(crate) struct TaskStateService {
    state_adapter: Arc<Mutex<Box<dyn StateAdapter>>>,
    scheduler_wake_notify: Option<Arc<Notify>>,
}

impl TaskStateService {
    pub(crate) fn new(state_adapter: Arc<Mutex<Box<dyn StateAdapter>>>) -> Self {
        Self {
            state_adapter,
            scheduler_wake_notify: None,
        }
    }

    pub(crate) fn with_scheduler_wake_notify(mut self, notify: Arc<Notify>) -> Self {
        self.scheduler_wake_notify = Some(notify);
        self
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
        self.notify_scheduler();
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
        self.notify_scheduler();
        Ok(task)
    }

    fn notify_scheduler(&self) {
        if let Some(notify) = &self.scheduler_wake_notify {
            notify.notify_waiters();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use butterflow_models::{Workflow, WorkflowStatus};
    use butterflow_state::mock_adapter::MockStateAdapter;

    fn workflow_run(workflow_run_id: Uuid) -> WorkflowRun {
        WorkflowRun {
            id: workflow_run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: Vec::new(),
                nodes: Vec::new(),
            },
            status: WorkflowStatus::Pending,
            params: HashMap::new(),
            tasks: Vec::new(),
            started_at: Utc::now(),
            ended_at: None,
            bundle_path: None,
            capabilities: None,
            name: None,
            target_path: None,
        }
    }

    fn task(workflow_run_id: Uuid, node_id: &str, status: TaskStatus) -> Task {
        let mut task = Task::new(workflow_run_id, node_id.to_string(), false);
        task.status = status;
        task
    }

    async fn setup_service(
        workflow_run: WorkflowRun,
        tasks: Vec<Task>,
    ) -> (TaskStateService, Arc<Mutex<Box<dyn StateAdapter>>>) {
        let adapter: Arc<Mutex<Box<dyn StateAdapter>>> =
            Arc::new(Mutex::new(Box::new(MockStateAdapter::new())));
        {
            let mut locked_adapter = adapter.lock().await;
            locked_adapter
                .save_workflow_run(&workflow_run)
                .await
                .unwrap();
            for task in tasks {
                locked_adapter.save_task(&task).await.unwrap();
            }
        }
        (TaskStateService::new(Arc::clone(&adapter)), adapter)
    }

    #[tokio::test]
    async fn task_lifecycle_transitions_update_status_metadata() {
        let workflow_run_id = Uuid::new_v4();
        let workflow_run = workflow_run(workflow_run_id);
        let mut initial_task = task(workflow_run_id, "node", TaskStatus::Failed);
        initial_task.started_at = Some(Utc::now());
        initial_task.ended_at = Some(Utc::now());
        initial_task.error = Some("previous failure".to_string());
        let task_id = initial_task.id;
        let (service, adapter) = setup_service(workflow_run, vec![initial_task]).await;

        let running = service.mark_running(task_id).await.unwrap();
        assert_eq!(running.status, TaskStatus::Running);
        assert!(running.started_at.is_some());
        assert!(running.ended_at.is_none());
        assert!(running.error.is_none());

        let completed = service.mark_completed(task_id).await.unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
        assert!(completed.ended_at.is_some());

        let failed = service.mark_failed(task_id, "boom").await.unwrap();
        assert_eq!(failed.status, TaskStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("boom"));
        assert!(failed.ended_at.is_some());

        let awaiting = service.mark_awaiting_trigger(task_id).await.unwrap();
        assert_eq!(awaiting.status, TaskStatus::AwaitingTrigger);
        assert!(awaiting.started_at.is_none());
        assert!(awaiting.ended_at.is_none());
        assert!(awaiting.error.is_none());

        let failed = service.mark_failed(task_id, "retry failed").await.unwrap();
        assert_eq!(failed.error.as_deref(), Some("retry failed"));

        let pending = service.reset_to_pending(task_id).await.unwrap();
        assert_eq!(pending.status, TaskStatus::Pending);
        assert!(pending.ended_at.is_none());
        assert!(pending.error.is_none());

        let persisted_task = adapter.lock().await.get_task(task_id).await.unwrap();
        assert_eq!(persisted_task.status, TaskStatus::Pending);
        assert!(persisted_task.ended_at.is_none());
        assert!(persisted_task.error.is_none());
    }

    #[tokio::test]
    async fn matrix_master_status_tracks_child_priority_and_terminal_state() {
        let workflow_run_id = Uuid::new_v4();
        let workflow_run = workflow_run(workflow_run_id);
        let master_task = Task::new(workflow_run_id, "matrix-node".to_string(), true);
        let master_task_id = master_task.id;
        let child_one = Task::new_matrix(
            workflow_run_id,
            "matrix-node".to_string(),
            master_task_id,
            HashMap::from([("index".to_string(), serde_json::json!(1))]),
        );
        let child_one_id = child_one.id;
        let child_two = Task::new_matrix(
            workflow_run_id,
            "matrix-node".to_string(),
            master_task_id,
            HashMap::from([("index".to_string(), serde_json::json!(2))]),
        );
        let child_two_id = child_two.id;
        let (service, adapter) =
            setup_service(workflow_run, vec![master_task, child_one, child_two]).await;

        service
            .update_matrix_master_status(master_task_id)
            .await
            .unwrap();
        let master = adapter.lock().await.get_task(master_task_id).await.unwrap();
        assert_eq!(master.status, TaskStatus::Pending);
        assert!(master.ended_at.is_none());

        service.mark_running(child_one_id).await.unwrap();
        service
            .update_matrix_master_status(master_task_id)
            .await
            .unwrap();
        let master = adapter.lock().await.get_task(master_task_id).await.unwrap();
        assert_eq!(master.status, TaskStatus::Running);
        assert!(master.ended_at.is_none());

        service.mark_awaiting_trigger(child_two_id).await.unwrap();
        service
            .update_matrix_master_status(master_task_id)
            .await
            .unwrap();
        let master = adapter.lock().await.get_task(master_task_id).await.unwrap();
        assert_eq!(master.status, TaskStatus::AwaitingTrigger);
        assert!(master.ended_at.is_none());

        service
            .mark_failed(child_one_id, "failed shard")
            .await
            .unwrap();
        service
            .update_matrix_master_status(master_task_id)
            .await
            .unwrap();
        let master = adapter.lock().await.get_task(master_task_id).await.unwrap();
        assert_eq!(master.status, TaskStatus::Failed);
        assert!(master.ended_at.is_some());

        service.mark_completed(child_one_id).await.unwrap();
        service.mark_completed(child_two_id).await.unwrap();
        service
            .update_matrix_master_status(master_task_id)
            .await
            .unwrap();
        let master = adapter.lock().await.get_task(master_task_id).await.unwrap();
        assert_eq!(master.status, TaskStatus::Completed);
        assert!(master.ended_at.is_some());
    }

    #[tokio::test]
    async fn state_transitions_wake_scheduler_waiters() {
        let workflow_run_id = Uuid::new_v4();
        let workflow_run = workflow_run(workflow_run_id);
        let initial_task = task(workflow_run_id, "node", TaskStatus::Pending);
        let task_id = initial_task.id;
        let (service, _) = setup_service(workflow_run, vec![initial_task]).await;
        let wake_notify = Arc::new(Notify::new());
        let service = service.with_scheduler_wake_notify(Arc::clone(&wake_notify));

        let notified = wake_notify.notified();
        service.mark_running(task_id).await.unwrap();

        tokio::time::timeout(std::time::Duration::from_millis(100), notified)
            .await
            .expect("task state transition should wake scheduler waiters");
    }
}
