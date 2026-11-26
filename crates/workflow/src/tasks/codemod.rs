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
use butterflow_core::registry::ResolvedPackage;
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
        require_preview_if_restricted(&package, &request.source, run.dry_run)?;
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

    let mut child = run.child(
        task.position,
        task.alongside,
        workflow_path.clone(),
        package_dir,
        request
            .params
            .map(Value::Object)
            .unwrap_or_else(|| run.params.clone()),
        workflow.built.callbacks.clone(),
        Arc::new(NestedEvents {
            parent: Arc::clone(&run.events),
        }),
        identity,
        Arc::clone(&task.cancel),
    );
    child.trusted = nested_trust(run.trusted, local);
    let child = Arc::new(child);
    let observer = run.services.nested_observer.clone();
    if let (Some(observer), Some(nested)) = (&observer, &nested_run) {
        block_on(observer.record(NestedCodemodRunEvent::Started(nested.clone())));
    }
    let started = std::time::Instant::now();
    let child_run_id = child.run_id.clone();
    let ending = execute(&workflow.built, child).map_err(|e| TaskError::Failed(e.to_string()))?;
    if let (Some(observer), Some(nested)) = (&observer, nested_run) {
        let outcome = match &ending {
            Ending::Succeeded(_) => NestedCodemodRunOutcome::Succeeded,
            Ending::Failed(error) | Ending::FailedWithOutput { error, .. } => {
                NestedCodemodRunOutcome::Failed {
                    error_message: error.clone(),
                }
            }
            Ending::Canceled | Ending::Paused => NestedCodemodRunOutcome::Failed {
                error_message: "canceled".into(),
            },
            Ending::Rejected(message) => NestedCodemodRunOutcome::Failed {
                error_message: format!("rejected: {message}"),
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
        Ending::Succeeded(output) => {
            // Files that failed in the codemod's partial steps fail this step.
            let (failed, total) = run
                .store
                .load_steps(&child_run_id)
                .unwrap_or_default()
                .iter()
                .filter(|step| step.status == StepStatus::Partial)
                .fold((0, 0), |(failed, total), step| {
                    let files = step.files.map_or(0, |files| files.total);
                    let failed_here = step.failed_files();
                    (failed + failed_here, total + files.max(failed_here))
                });
            Ok(TaskOutcome::new(json!({ "output": output })).with_failures(failed, total))
        }
        Ending::Failed(error) | Ending::FailedWithOutput { error, .. } => Err(TaskError::Failed(
            format!("codemod '{}' failed: {error}", request.source),
        )),
        Ending::Canceled | Ending::Paused => Err(TaskError::Canceled),
        Ending::Rejected(message) => Err(TaskError::Rejected(message)),
        Ending::AwaitingApproval(message) => Err(TaskError::AwaitingApproval(message)),
        // Nested runs have no test plan.
        Ending::Stopped(_) => Ok(TaskOutcome::new(json!({ "output": null }))),
    }
}

fn require_preview_if_restricted(
    package: &ResolvedPackage,
    source: &str,
    dry_run: bool,
) -> Result<(), TaskError> {
    if package.dry_run_only && !dry_run {
        return Err(TaskError::Failed(format!(
            "codemod '{source}' is preview-only; rerun the workflow with --dry-run"
        )));
    }
    Ok(())
}

/// Whether a nested codemod's steps are the person's own: only a local one
/// in a workflow that is. A package from the registry never is, wherever it
/// is nested, and what a registry package nests is the package's.
fn nested_trust(parent: bool, local: bool) -> bool {
    parent && local
}

fn is_local(source: &str) -> bool {
    source.starts_with("./") || source.starts_with("../") || source.starts_with('/')
}

/// Passes on a child workflow's step events, which name the step running it
/// as their parent (see `RunContext::child`); views of the run's own steps
/// show them as that step's (`Event::top_level`). The child's run events
/// are the parent's business: a nested run shares its parent's dry run
/// overlay, so its changes are in the parent's diff.
struct NestedEvents {
    parent: Arc<dyn EventSink>,
}

impl EventSink for NestedEvents {
    fn emit(&self, event: Event) {
        if !matches!(
            event,
            Event::RunStarted { .. } | Event::RunDiff { .. } | Event::RunFinished { .. }
        ) {
            self.parent.emit(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterflow_core::registry::PackageSpec;

    #[test]
    fn only_a_local_codemod_in_ones_own_workflow_is_trusted() {
        assert!(nested_trust(true, true));
        assert!(
            !nested_trust(true, false),
            "a registry package nested in one's own"
        );
        assert!(!nested_trust(false, true), "what a registry package nests");
        assert!(!nested_trust(false, false));
    }

    #[test]
    fn nested_registry_preview_restriction_rejects_apply_but_allows_dry_run() {
        let mut package = ResolvedPackage {
            spec: PackageSpec {
                scope: None,
                name: "child".into(),
                version: None,
            },
            version: "1.0.0".into(),
            package_dir: "unused".into(),
            dry_run_only: true,
            registry_metadata: None,
        };
        assert!(matches!(
            require_preview_if_restricted(&package, "child", false),
            Err(TaskError::Failed(message)) if message.contains("child") && message.contains("--dry-run")
        ));
        assert!(require_preview_if_restricted(&package, "child", true).is_ok());
        package.dry_run_only = false;
        assert!(require_preview_if_restricted(&package, "child", false).is_ok());
    }
}
