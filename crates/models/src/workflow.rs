use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
