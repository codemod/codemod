pub mod error;
pub mod node;
pub mod runtime;
pub mod state_diff;
pub mod step;
pub mod strategy;
pub mod task;
pub mod template;
pub mod trigger;
pub mod variable;
pub mod workflow;

// Re-export types
pub use error::Error;
pub use node::Node;
pub use runtime::{Runtime, RuntimeType};
pub use state_diff::{DiffOperation, FieldDiff, StateDiff, TaskDiff, WorkflowRunDiff};
pub use step::{Step, TemplateUse};
pub use strategy::{Strategy, StrategyType};
pub use task::{Task, TaskStatus};
pub use template::{Template, TemplateInput, TemplateOutput};
pub use trigger::{Trigger, TriggerType};
pub use variable::resolve_variables;
pub use workflow::{Workflow, WorkflowRun, WorkflowState, WorkflowStatus};

pub type Result<T> = std::result::Result<T, Error>;
