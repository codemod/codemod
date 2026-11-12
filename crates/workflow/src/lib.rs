//! Code-first workflow engine.
//!
//! A workflow is a TypeScript module whose default export runs steps:
//! `step(task)` marks durable work, `forEachFile` runs a callback per file,
//! and the other tasks (`shell`, `jssg`, `codemod`, ...) wrap the existing
//! executors. Each step's outcome is recorded in a [`RunStore`]; resuming a
//! run replays finished steps. `workflow.yaml` runs through [`compat`], which
//! translates it into the same TypeScript.

pub mod build;
pub mod compat;
mod engine;
pub mod events;
mod expr;
pub mod interaction;
mod run;
mod tasks;

pub use build::{build, BuildError, Built, FLOW_MODULE};
pub use butterflow_state::run_store::{
    LocalStore, MemoryStore, RunRecord, RunStatus, RunStore, StepRecord, StepStatus,
};
pub use engine::{
    is_typescript, load_workflow, Engine, EngineError, LoadedWorkflow, RunReport, StartRequest,
};
pub use run::{Ending, RunError};

/// Services tasks call into, supplied by the host application.
#[derive(Default)]
pub struct Services {}
