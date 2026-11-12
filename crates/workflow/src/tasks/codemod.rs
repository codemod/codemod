//! `codemod(source, { params })`: run another codemod package (a registry
//! name or a local path) as part of this step. The child workflow runs in the
//! same engine with its own step history, so resuming the parent also resumes
//! the child.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};

use super::{block_on, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::events::{Event, EventSink};
use crate::run::{execute, Ending, RunContext};
use butterflow_state::run_store::StepStatus;

#[derive(Deserialize)]
pub(crate) struct Request {
    source: String,
    /// Defaults to this workflow's params.
    params: Option<serde_json::Map<String, Value>>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let run = task.run;
    let local = is_local(&request.source);
    let identity = if local {
        let path = run.bundle_dir.join(&request.source);
        std::fs::canonicalize(&path)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    } else {
        request.source.clone()
    };
    if run.chain.contains(&identity) {
        let mut cycle = run.chain.clone();
        cycle.push(identity);
        return Err(TaskError::Failed(format!(
            "codemod dependency cycle: {}",
            cycle.join(" -> ")
        )));
    }

    let package_dir = if local {
        let path = run.bundle_dir.join(&request.source);
        match &run.services.registry {
            Some(registry) => {
                registry
                    .resolve_local_package_at(&path)
                    .map_err(|e| {
                        TaskError::Failed(format!("cannot load '{}': {e}", request.source))
                    })?
                    .package_dir
            }
            None => path,
        }
    } else {
        let registry = run.services.registry.as_ref().ok_or_else(|| {
            TaskError::Failed("running registry codemods is not available here".into())
        })?;
        block_on(registry.resolve_package(&request.source, None, false, None))
            .map_err(|e| TaskError::Failed(format!("cannot resolve '{}': {e}", request.source)))?
            .package_dir
    };
    let workflow_path = find_workflow(&package_dir)?;
    let workflow = crate::engine::load_workflow(&workflow_path)
        .map_err(|e| TaskError::Failed(format!("cannot load '{}': {e}", request.source)))?;
    task.log(format!("running {}", request.source));

    let mut chain = run.chain.clone();
    chain.push(identity);
    let child = Arc::new(RunContext {
        run_id: format!("{}~{}", run.run_id, task.position),
        target: run.target.clone(),
        bundle_dir: package_dir,
        params: request
            .params
            .map(Value::Object)
            .unwrap_or_else(|| run.params.clone()),
        dry_run: run.dry_run,
        callbacks: workflow.built.callbacks.clone(),
        store: Arc::clone(&run.store),
        events: Arc::new(NestedEvents {
            parent: Arc::clone(&run.events),
            position: task.position,
        }),
        interaction: Arc::clone(&run.interaction),
        services: Arc::clone(&run.services),
        cancel: Arc::clone(&run.cancel),
        commit_lock: Mutex::new(()),
        chain,
    });
    match execute(&workflow.built, child).map_err(|e| TaskError::Failed(e.to_string()))? {
        Ending::Succeeded(output) => Ok(TaskOutcome::new(json!({ "output": output }))),
        Ending::Failed(error) => Err(TaskError::Failed(format!(
            "codemod '{}' failed: {error}",
            request.source
        ))),
        Ending::Canceled => Err(TaskError::Canceled),
        Ending::AwaitingApproval(message) => Err(TaskError::AwaitingApproval(message)),
    }
}

fn is_local(source: &str) -> bool {
    source.starts_with("./") || source.starts_with("../") || source.starts_with('/')
}

/// `workflow.ts`, or a YAML workflow, in a package directory.
pub(crate) fn find_workflow(package_dir: &Path) -> Result<PathBuf, TaskError> {
    ["workflow.ts", "workflow.yaml", "workflow.yml"]
        .iter()
        .map(|name| package_dir.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            TaskError::Failed(format!(
                "no workflow.ts or workflow.yaml in {}",
                package_dir.display()
            ))
        })
}

/// Reports a child workflow's steps as log lines of the parent step.
struct NestedEvents {
    parent: Arc<dyn EventSink>,
    position: u64,
}

impl EventSink for NestedEvents {
    fn emit(&self, event: Event) {
        let position = self.position;
        let line = match event {
            Event::StepStarted { label, .. } => format!("▸ {label}"),
            Event::StepReplayed { label, .. } => format!("↺ {label}"),
            Event::StepFinished {
                label,
                status,
                error,
                ..
            } => match (status, error) {
                (StepStatus::Succeeded, _) => format!("✓ {label}"),
                (StepStatus::Failed, Some(error)) => format!("✗ {label}: {error}"),
                (StepStatus::Failed, None) => format!("✗ {label}"),
            },
            Event::StepLog { line, .. } => format!("  {line}"),
            Event::StepProgress {
                processed, total, ..
            } => {
                self.parent.emit(Event::StepProgress {
                    position,
                    processed,
                    total,
                });
                return;
            }
            Event::RunStarted { .. } | Event::RunFinished { .. } => return,
        };
        self.parent.emit(Event::StepLog { position, line });
    }
}
