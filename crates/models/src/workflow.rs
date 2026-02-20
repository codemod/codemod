use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use uuid::Uuid;

use codemod_llrt_capabilities::types::LlrtSupportedModules;

use crate::node::Node;
use crate::template::Template;
use crate::SimpleSchema;
use ts_rs::TS;

/// Represents a workflow definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct Workflow {
    /// Version of the workflow format
    pub version: String,

    /// State schema definition
    #[serde(default)]
    #[ts(optional=nullable)]
    pub state: Option<WorkflowState>,

    /// Params schema definition
    #[serde(default)]
    #[ts(optional=nullable)]
    pub params: Option<WorkflowParams>,

    // Why using as="Option<Vec<Template>>" -> https://github.com/Aleph-Alpha/ts-rs/issues/175
    /// Templates for reusable components
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<Template>>")]
    pub templates: Vec<Template>,

    /// Nodes in the workflow
    pub nodes: Vec<Node>,
}

/// Represents the state schema for a workflow
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, TS)]
pub struct WorkflowState {
    /// Object schema definition (root is always an object)
    #[serde(default)]
    pub schema: SimpleSchema,
}

/// Represents the params schema for a workflow
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, TS)]
pub struct WorkflowParams {
    /// Object schema definition (root is always an object)
    #[serde(default)]
    pub schema: SimpleSchema,
}

/// Represents a workflow run
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowRun {
    /// Unique identifier for the workflow run
    pub id: Uuid,

    /// The workflow definition
    pub workflow: Workflow,

    /// Current status of the workflow run
    pub status: WorkflowStatus,

    /// Parameters passed to the workflow
    pub params: HashMap<String, serde_json::Value>,

    /// Tasks created for this workflow run
    pub tasks: Vec<Uuid>,

    /// Start time of the workflow run
    pub started_at: DateTime<Utc>,

    /// End time of the workflow run (if completed or failed)
    #[serde(default)]
    #[ts(optional=nullable)]
    pub ended_at: Option<DateTime<Utc>>,

    /// The absolute path to the root directory of the workflow bundle
    #[ts(optional=nullable)]
    pub bundle_path: Option<PathBuf>,

    /// The target path where the workflow was run
    #[serde(default)]
    #[ts(optional=nullable)]
    pub target_path: Option<PathBuf>,

    /// Capabilities used in the workflow run
    #[serde(default)]
    #[ts(optional, as = "Option<HashSet<LlrtSupportedModules>>")]
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
}

/// Status of a workflow run
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum WorkflowStatus {
    /// Workflow is pending execution
    Pending,

    /// Workflow is currently running
    Running,

    /// Workflow has completed successfully
    Completed,

    /// Workflow has failed
    Failed,

    /// Workflow is paused waiting for manual triggers
    AwaitingTrigger,

    /// Workflow has been canceled
    Canceled,
}
