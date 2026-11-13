//! `codemod(source, { params })`: run another codemod package (a registry
//! name or a local path) as part of this step. The child workflow runs in the
//! same engine with its own step history, so resuming the parent also resumes
//! the child.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{block_on, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::events::{Event, EventSink};
use crate::run::{execute, Ending};
use butterflow_core::nested_codemod_run::{
    NestedCodemodRun, NestedCodemodRunEvent, NestedCodemodRunOutcome,
};
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

    let mut nested_run: Option<NestedCodemodRun> = None;
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
        let package = block_on(registry.resolve_package(&request.source, None, false, None))
            .map_err(|e| TaskError::Failed(format!("cannot resolve '{}': {e}", request.source)))?;
        nested_run = package
            .registry_metadata
            .as_ref()
            .map(|metadata| NestedCodemodRun {
                codemod_name: metadata.package_web_path.clone(),
                package_version: package.version.clone(),
                execution_id: uuid::Uuid::new_v4().to_string(),
                // The first entry is the run's own workflow, not a dependency.
                dependency_path: run
                    .chain
                    .iter()
                    .skip(1)
                    .map(|identity| {
                        if identity.starts_with('/') {
                            "<local>".to_string()
                        } else {
                            identity.clone()
                        }
                    })
                    .chain(std::iter::once(request.source.clone()))
                    .collect(),
            });
        package.package_dir
    };
    let workflow_path = butterflow_core::package_workflows::locate_workflow(&package_dir, None)
        .map_err(|e| TaskError::Failed(format!("cannot load '{}': {e}", request.source)))?;
    let workflow = crate::engine::load_workflow(&workflow_path)
        .map_err(|e| TaskError::Failed(format!("cannot load '{}': {e}", request.source)))?;
    task.log(format!("running {}", request.source));

    let child = Arc::new(
        run.child(
            task.position,
            package_dir,
            request
                .params
                .map(Value::Object)
                .unwrap_or_else(|| run.params.clone()),
            workflow.built.callbacks.clone(),
            Arc::new(NestedEvents {
                parent: Arc::clone(&run.events),
                position: task.position,
            }),
            identity,
        ),
    );
    let observer = run.services.nested_observer.clone();
    if let (Some(observer), Some(nested)) = (&observer, &nested_run) {
        block_on(observer.record(NestedCodemodRunEvent::Started(nested.clone())));
    }
    let started = std::time::Instant::now();
    let ending = execute(&workflow.built, child).map_err(|e| TaskError::Failed(e.to_string()))?;
    if let (Some(observer), Some(nested)) = (&observer, nested_run) {
        let outcome = match &ending {
            Ending::Succeeded(_) => NestedCodemodRunOutcome::Succeeded,
            Ending::Failed(error) => NestedCodemodRunOutcome::Failed {
                error_message: error.clone(),
            },
            Ending::Canceled => NestedCodemodRunOutcome::Failed {
                error_message: "canceled".into(),
            },
            Ending::AwaitingApproval(message) => NestedCodemodRunOutcome::Failed {
                error_message: format!("waiting for approval: {message}"),
            },
            Ending::Stopped(_) => NestedCodemodRunOutcome::Succeeded,
        };
        block_on(observer.record(NestedCodemodRunEvent::Completed {
            run: nested,
            outcome,
            duration_ms: started.elapsed().as_millis(),
        }));
    }
    match ending {
        Ending::Succeeded(output) => Ok(TaskOutcome::new(json!({ "output": output }))),
        Ending::Failed(error) => Err(TaskError::Failed(format!(
            "codemod '{}' failed: {error}",
            request.source
        ))),
        Ending::Canceled => Err(TaskError::Canceled),
        Ending::AwaitingApproval(message) => Err(TaskError::AwaitingApproval(message)),
        // Nested runs have no test plan.
        Ending::Stopped(_) => Ok(TaskOutcome::new(json!({ "output": null }))),
    }
}

fn is_local(source: &str) -> bool {
    source.starts_with("./") || source.starts_with("../") || source.starts_with('/')
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
                (StepStatus::Canceled, _) => format!("■ {label}: canceled"),
                (StepStatus::AwaitingApproval, _) => format!("‖ {label}: waiting for approval"),
            },
            Event::StepLog { line, .. } => format!("  {line}"),
            Event::RunLog { line } => line,
            Event::StepAgent {
                agent, activity, ..
            } => {
                self.parent.emit(Event::StepAgent {
                    position,
                    agent,
                    activity,
                });
                return;
            }
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
