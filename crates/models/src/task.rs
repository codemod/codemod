use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;
use uuid::Uuid;

/// Status of a task
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum TaskStatus {
    /// Task hasn't started execution yet
    Pending,

    /// Task is currently being executed
    Running,

    /// Task has completed successfully
    Completed,

    /// Task execution failed with an error
    Failed,

    /// Task is waiting for a manual trigger
    AwaitingTrigger,

    /// Task is blocked by dependencies
    Blocked,

    /// Task will not be executed
    WontDo,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskErrorDetails {
    ShellCommand {
        command: String,
        exit_code: i32,
        output: String,
    },
    AstGrep {
        message: String,
        help: Option<String>,
    },
}

/// Represents a task (runtime instance of a node)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Task {
    /// Unique identifier for the task
    pub id: Uuid,

    /// ID of the workflow run this task belongs to
    pub workflow_run_id: Uuid,

    /// ID of the node this task is an instance of
    pub node_id: String,

    /// Current status of the task
    pub status: TaskStatus,

    /// Whether or not this task is a master task for other matrix tasks.
    pub is_master: bool,

    /// For matrix tasks, the master task ID
    #[serde(default)]
    #[ts(optional=nullable)]
    pub master_task_id: Option<Uuid>,

    /// For matrix tasks, the matrix values
    #[serde(default)]
    #[ts(optional=nullable)]
    pub matrix_values: Option<HashMap<String, serde_json::Value>>,

    /// Start time of the task
    #[serde(default)]
    #[ts(optional=nullable)]
    pub started_at: Option<DateTime<Utc>>,

    /// End time of the task (if completed or failed)
    #[serde(default)]
    #[ts(optional=nullable)]
    pub ended_at: Option<DateTime<Utc>>,

    /// Error message (if failed)
    #[serde(default)]
    #[ts(optional=nullable)]
    pub error: Option<String>,

    /// Structured error details (if failed)
    #[serde(default)]
    #[ts(optional=nullable)]
    pub error_details: Option<TaskErrorDetails>,

    /// Logs from the task
    #[serde(default)]
    pub logs: Vec<String>,
}

impl Task {
    /// Create a new task
    pub fn new(workflow_run_id: Uuid, node_id: String, is_master: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            workflow_run_id,
            node_id,
            is_master,
            status: TaskStatus::Pending,
            master_task_id: None,
            matrix_values: None,
            started_at: None,
            ended_at: None,
            error: None,
            error_details: None,
            logs: Vec::new(),
        }
    }

    /// Create a new matrix task
    pub fn new_matrix(
        workflow_run_id: Uuid,
        node_id: String,
        master_task_id: Uuid,
        matrix_values: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            workflow_run_id,
            node_id,
            status: TaskStatus::Pending,
            master_task_id: Some(master_task_id),
            matrix_values: Some(matrix_values),
            started_at: None,
            ended_at: None,
            error: None,
            error_details: None,
            logs: Vec::new(),
            is_master: false,
        }
    }
}
