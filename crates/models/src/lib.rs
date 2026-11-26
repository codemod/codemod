pub mod error;
pub mod node;
pub mod runtime;
pub mod schema;
pub mod step;
pub mod strategy;
pub mod template;
pub mod trigger;
pub mod variable;
pub mod workflow;

// Re-export types
pub use error::Error;
pub use node::Node;
pub use runtime::{Runtime, RuntimeType};
pub use schema::{SimpleSchema, SimpleSchemaProperty, SimpleSchemaType, SimpleSchemaVariant};
pub use step::{CommitConfig, PullRequestConfig, Step, TemplateUse};
pub use strategy::{Strategy, StrategyType};
pub use template::{Template, TemplateInput, TemplateOutput};
pub use trigger::{Trigger, TriggerType};
pub use variable::{
    evaluate_condition, resolve_expressions, resolve_string_list, resolve_string_with_expression,
    resolve_usize_value, TaskExpressionContext,
};
pub use workflow::{Workflow, WorkflowState};

pub type Result<T> = std::result::Result<T, Error>;
