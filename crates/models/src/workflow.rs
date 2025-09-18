use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

use crate::node::Node;
use crate::template::Template;
use ts_rs::TS;

/// Simple schema system for workflow state and params validation
/// Root is always an object with properties
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, TS)]
pub struct SimpleSchema {
    /// Properties of the root object
    #[serde(flatten)]
    pub properties: HashMap<String, SimpleSchemaProperty>,
}

/// Represents a property in the schema with common metadata and type-specific fields
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct SimpleSchemaProperty {
    /// Human-readable name for this property
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Description of what this property represents
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The actual schema definition
    #[serde(flatten)]
    pub schema: SimpleSchemaType,
}

/// Represents the type-specific schema definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SimpleSchemaType {
    /// String type with optional oneOf and default
    String {
        /// Allows multiple schema alternatives for strings
        #[serde(rename = "oneOf")]
        one_of: Option<Vec<SimpleSchemaVariant>>,

        /// Default value for the property
        default: Option<String>,
    },

    /// Array type with required items schema
    Array {
        /// Defines the schema of array items
        items: Box<SimpleSchemaProperty>,

        /// Default value for the property
        default: Option<String>,
    },

    /// Object type with properties
    Object {
        /// Properties of the object
        properties: Option<HashMap<String, SimpleSchemaProperty>>,

        /// Default value for the property
        default: Option<String>,
    },

    /// Boolean type with optional default
    Boolean {
        /// Default value for the property
        default: Option<bool>,
    },
}

/// Represents a variant in a oneOf schema for strings
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct SimpleSchemaVariant {
    /// Type of this variant (always "string" for oneOf variants)
    #[serde(rename = "type")]
    pub schema_type: String,

    /// For string types with enumeration, the allowed values
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

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
    pub params: HashMap<String, String>,

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
