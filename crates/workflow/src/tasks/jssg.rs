//! `jssg(path, options)`: run a JSSG transform file over the target.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;

use butterflow_core::jssg_runner::{run_jssg, JssgRunOutcome, JssgRunRequest, SemanticMode};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use serde::Deserialize;
use serde_json::{json, Value};

use super::changes::{self, bundle_path, relative, PlannedChange};
use super::{Globs, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::{Approval, Decision};

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    pub path: String,
    pub include: Option<Globs>,
    pub exclude: Option<Globs>,
    /// Directory under the target to restrict the run to.
    pub base_path: Option<String>,
    /// Process exactly these files (relative to the target).
    pub files: Option<Vec<String>>,
    pub language: Option<String>,
    /// Merged over the run's params; reaches the transform as `options.params`.
    #[serde(default)]
    pub params: serde_json::Map<String, Value>,
    pub matrix: Option<serde_json::Map<String, Value>>,
    /// Capabilities the transform needs (`fs`, `fetch`, `child_process`).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// `"file"`, `"workspace"`, or `{ mode: "workspace", root }`.
    pub semantic_analysis: Option<Value>,
    pub threads: Option<usize>,
    /// Run the transform but do not apply its changes.
    #[serde(default)]
    pub dry_run: bool,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let dry_run = request.dry_run || task.run.dry_run;
    let outcome = execute(task, &request, dry_run)?;

    let planned = plan(task, &outcome)?;
    let applied = changes::commit(task, &planned, dry_run)?;
    let summary = &task.run.services.summary;
    summary.add_unmodified(outcome.unmodified);
    summary.add_errors(outcome.failures.len());
    summary.merge_metrics(outcome.metrics.clone());
    for warning in &outcome.warnings {
        task.log(warning.clone());
    }

    let failures: Vec<Value> = outcome
        .failures
        .iter()
        .map(|(path, error)| {
            json!({
                "path": relative(&task.run.target, path).unwrap_or_else(|| path.display().to_string()),
                "error": error,
            })
        })
        .collect();
    if !outcome.failures.is_empty() && outcome.changes.is_empty() && outcome.unmodified == 0 {
        return Err(TaskError::Failed(format!(
            "the transform failed on every file; first error: {}",
            outcome.failures[0].1
        )));
    }
    let state = outcome.state.clone();
    Ok(TaskOutcome::new(json!({
        "changes": applied,
        "unmodified": outcome.unmodified,
        "failures": failures,
    }))
    .with_state(state))
}

/// Run the transform without applying anything.
pub(crate) fn execute(
    task: &TaskContext<'_>,
    request: &Request,
    dry_run: bool,
) -> Result<JssgRunOutcome, TaskError> {
    let run = task.run;
    let script = bundle_path(task, &request.path, "transform")?;
    let capabilities = capabilities(task, &request.capabilities)?;

    let mut params: HashMap<String, Value> = match &run.params {
        Value::Object(map) => map.clone().into_iter().collect(),
        _ => HashMap::new(),
    };
    params.extend(request.params.clone());

    let base_path = request
        .base_path
        .as_deref()
        .map(|base| {
            let base = base.trim().trim_start_matches("./");
            if changes::is_safe_relative(base) {
                Ok(PathBuf::from(base))
            } else {
                Err(format!("basePath '{base}' must stay inside the target"))
            }
        })
        .transpose()?;
    // File-scope analysis by default, as `codemod jssg run` and `jssg test`
    // have: without it, `node.definition()` and friends find nothing.
    let semantic = match &request.semantic_analysis {
        None => SemanticMode::File,
        Some(Value::String(mode)) if mode == "none" => SemanticMode::None,
        Some(Value::String(mode)) if mode == "file" => SemanticMode::File,
        Some(Value::String(mode)) if mode == "workspace" => {
            SemanticMode::Workspace(run.target.clone())
        }
        Some(Value::Object(detail)) => match detail.get("mode").and_then(Value::as_str) {
            Some("file") => SemanticMode::File,
            Some("workspace") => SemanticMode::Workspace(
                detail
                    .get("root")
                    .and_then(Value::as_str)
                    .map(|root| run.target.join(root))
                    .unwrap_or_else(|| run.target.clone()),
            ),
            _ => {
                return Err(TaskError::Failed(
                    "semanticAnalysis.mode must be 'file' or 'workspace'".into(),
                ))
            }
        },
        Some(other) => {
            return Err(TaskError::Failed(format!(
                "semanticAnalysis must be 'file', 'workspace', 'none' or an object, not {other}"
            )))
        }
    };

    run_jssg(JssgRunRequest {
        script_path: script,
        target: run.target.clone(),
        base_path,
        include: request.include.clone().map(Globs::into_vec),
        exclude: request.exclude.clone().map(Globs::into_vec),
        explicit_files: request
            .files
            .as_ref()
            .map(|files| files.iter().map(|file| run.target.join(file)).collect()),
        language: request
            .language
            .clone()
            .unwrap_or_else(|| "typescript".into()),
        params,
        matrix: request
            .matrix
            .clone()
            .map(|matrix| matrix.into_iter().collect()),
        capabilities: Some(capabilities),
        semantic,
        threads: request.threads,
        dry_run,
        initial_state: task.state.clone().into_iter().collect(),
        cancel: Some(run.cancel.clone()),
        progress: Some(task.walk_progress()),
    })
    .map_err(TaskError::Failed)
}

fn plan(task: &TaskContext<'_>, outcome: &JssgRunOutcome) -> Result<Vec<PlannedChange>, String> {
    let target = &task.run.target;
    let mut planned = Vec::new();
    for change in &outcome.changes {
        let path = relative(target, &change.path).ok_or_else(|| {
            format!(
                "the transform changed '{}', outside the target",
                change.path.display()
            )
        })?;
        let rename_to = change
            .rename_to
            .as_ref()
            .map(|to| {
                relative(target, to).ok_or_else(|| {
                    format!(
                        "the transform renamed '{path}' to '{}', outside the target",
                        to.display()
                    )
                })
            })
            .transpose()?
            .filter(|to| *to != path);
        planned.push(PlannedChange {
            path,
            original: change.original.clone(),
            text: change.content.clone(),
            rename_to,
            delete: false,
        });
    }
    Ok(planned)
}

/// Granted capabilities plus the ones the step asks for; anything not
/// granted up front needs approval.
pub(crate) fn capabilities(
    task: &TaskContext<'_>,
    requested: &[String],
) -> Result<HashSet<LlrtSupportedModules>, TaskError> {
    let granted = &task.run.services.capabilities;
    let mut result = granted.clone();
    let mut missing = Vec::new();
    for name in requested {
        let module = LlrtSupportedModules::from_str(name)
            .map_err(|_| TaskError::Failed(format!("unknown capability '{name}'")))?;
        if !granted.contains(&module) {
            missing.push((name.clone(), module));
        }
    }
    if missing.is_empty() {
        return Ok(result);
    }
    let names: Vec<String> = missing.iter().map(|(name, _)| name.clone()).collect();
    match task.run.interaction.approve(&Approval::Capabilities {
        modules: names.clone(),
    }) {
        Decision::Approved => result.extend(missing.into_iter().map(|(_, module)| module)),
        Decision::Denied | Decision::Deferred => {
            task.log(format!("capabilities not granted: {}", names.join(", ")))
        }
    }
    Ok(result)
}
