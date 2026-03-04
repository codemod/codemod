use uuid::Uuid;

/// Actions that can be performed in the TUI
#[derive(Debug, Clone)]
pub enum Action {
    /// Navigate to the task list for a specific workflow run
    NavigateToTaskList(Uuid),
    /// Navigate back to the run list
    NavigateToRunList,
    /// View logs for a specific task (workflow_run_id, task_id)
    ViewLogs(Uuid, Uuid),
    /// Trigger a specific task (workflow_run_id, task_id)
    TriggerTask(Uuid, Uuid),
    /// Trigger all awaiting tasks in a workflow run
    TriggerAll(Uuid),
    /// Retry a failed task (workflow_run_id, task_id)
    RetryFailed(Uuid, Uuid),
    /// Cancel a workflow run
    CancelWorkflow(Uuid),
    /// Navigate to the settings screen for a specific workflow run
    NavigateToSettings(Uuid),
    /// Navigate back from settings to the task list
    NavigateBackFromSettings,
    /// Quit the TUI
    Quit,
}
