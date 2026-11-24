//! Code-first workflow engine.
//!
//! A workflow is a TypeScript module whose default export runs steps:
//! `step(task)` marks durable work, `forEachFile` runs a callback per file,
//! and the other tasks (`shell`, `astGrep`, `codemod`, ...) wrap the existing
//! executors. Each step's outcome is recorded in a [`RunStore`]; resuming a
//! run replays finished steps. `workflow.yaml` runs through [`compat`], which
//! translates it into the same TypeScript.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use butterflow_core::config::InstallSkillExecutor;
use butterflow_core::diff::FileDiff;
use butterflow_core::execution_stats::ExecutionStats;
use butterflow_core::nested_codemod_run::NestedCodemodRunObserver;
use butterflow_core::registry::RegistryClient;
use codemod_llrt_capabilities::types::LlrtSupportedModules;

pub mod build;
mod callback_file;
pub mod compat;
mod deps_host;
mod engine;
pub mod events;
mod expr;
mod hosted;
mod imported;
pub mod interaction;
mod js;
pub mod params;
mod run;
mod tasks;

pub use build::{
    build, inspect, rewrite_for_node, BuildError, Built, Inspection, InstalledSkill, FLOW_MODULE,
};
pub use butterflow_core::package_workflows::{
    declared_workflows, locate_workflow, DeclaredWorkflow,
};
pub use butterflow_state::run_store::{
    LocalStore, MemoryStore, RunRecord, RunSettings, RunStatus, RunStore, StepRecord, StepStatus,
};
pub use callback_file::{run_callback_file, CallbackFileRequest, CallbackFileRun};
pub use engine::{
    cancel_run, is_typescript, load_workflow, send_event, Canceled, Engine, EngineError,
    LoadedWorkflow, RunReport, StartRequest, StopHandle,
};
pub use hosted::{HostedRun, OpenRequest, SelectedFiles};
pub use js::orchestrator_version;
pub use run::{Ending, RunError, TestPlan};

/// Services tasks call into, supplied by the host application.
#[derive(Default)]
pub struct Services {
    /// Runtime capabilities granted to transforms up front.
    pub capabilities: HashSet<LlrtSupportedModules>,
    /// Resolves `codemod(...)` packages from the registry.
    pub registry: Option<RegistryClient>,
    /// Runs `installSkill(...)` steps.
    pub install_skill: Option<Arc<dyn InstallSkillExecutor>>,
    /// Whether `branch`, `commit` and `pullRequest` steps touch git.
    pub managed_git: bool,
    /// Preferred coding agent for `agent(...)` steps (`--agent`).
    pub agent: Option<String>,
    /// Skip `shard(...)` steps, returning no shards (pro dry runs).
    pub skip_shard_steps: bool,
    /// Release metadata for `registry.npm(...)` / `registry.cargo(...)`: one
    /// shared, cached client for the run, or a fake in tests.
    pub package_registry: Option<Arc<dyn codemod_deps::Registry>>,
    /// Told when a `codemod(...)` step runs a registry package.
    pub nested_observer: Option<Arc<dyn NestedCodemodRunObserver>>,
    /// Diffs and counters collected while the run progresses.
    pub summary: Arc<Summary>,
}

/// What a run changed, for reports.
#[derive(Default)]
pub struct Summary {
    diffs: Mutex<Vec<FileDiff>>,
    pub stats: ExecutionStats,
}

impl Summary {
    pub fn diffs(&self) -> Vec<FileDiff> {
        self.diffs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn add_diff(&self, diff: FileDiff) {
        self.diffs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(diff);
    }

    pub(crate) fn add_modified(&self, count: usize) {
        self.stats
            .files_modified
            .fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn add_unmodified(&self, count: usize) {
        self.stats
            .files_unmodified
            .fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn add_errors(&self, count: usize) {
        self.stats
            .files_with_errors
            .fetch_add(count, Ordering::Relaxed);
    }
}
