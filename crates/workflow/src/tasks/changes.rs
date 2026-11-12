//! Applying file changes: every task that edits files plans its changes,
//! then commits them here in one go, after conflict checks. Diffs and
//! counters go to the run summary.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use butterflow_core::diff::{generate_unified_diff, DiffConfig, DiffMetadata};
use serde_json::{json, Value};

use super::TaskContext;

/// What one file ends up as.
#[derive(Debug, Clone)]
pub(crate) struct PlannedChange {
    /// Relative to the target, with `/` separators.
    pub path: String,
    pub original: String,
    pub text: String,
    pub rename_to: Option<String>,
    pub delete: bool,
}

impl PlannedChange {
    pub fn edit(path: String, original: String, text: String) -> Self {
        Self {
            path,
            original,
            text,
            rename_to: None,
            delete: false,
        }
    }
}

/// `path` relative to `target`, with `/` separators; `None` if outside it.
pub(crate) fn relative(target: &Path, path: &Path) -> Option<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        target.join(path)
    };
    let target = fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let path = fs::canonicalize(&path)
        .or_else(|_| {
            // A rename target does not exist yet: canonicalize its parent.
            let parent = path.parent().map(fs::canonicalize).transpose()?;
            Ok::<_, std::io::Error>(match (parent, path.file_name()) {
                (Some(parent), Some(name)) => parent.join(name),
                _ => path.clone(),
            })
        })
        .unwrap_or(path);
    path.strip_prefix(&target)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn is_safe_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// Resolve a path from the workflow (a transform, a rule file, a script)
/// against the workflow's directory, refusing paths that leave it.
pub(crate) fn bundle_path(
    task: &TaskContext<'_>,
    path: &str,
    what: &str,
) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    let relative = trimmed.strip_prefix("./").unwrap_or(trimmed);
    if !is_safe_relative(relative) {
        return Err(format!(
            "{what} '{path}' must be a relative path inside the workflow's directory"
        ));
    }
    Ok(task.run.bundle_dir.join(relative))
}

/// Check the plan for conflicts, then apply it unless `dry_run`. Either every
/// change is applied or none is. Returns `{ written, renamed, deleted }`.
pub(crate) fn commit(
    task: &TaskContext<'_>,
    planned: &[PlannedChange],
    dry_run: bool,
) -> Result<Value, String> {
    let target = &task.run.target;
    let vacated: HashSet<&str> = planned
        .iter()
        .filter(|p| p.delete || p.rename_to.is_some())
        .map(|p| p.path.as_str())
        .collect();
    let mut claimed: HashMap<&str, &str> = HashMap::new();
    for plan in planned.iter().filter(|p| !p.delete) {
        let Some(to) = plan.rename_to.as_deref() else {
            continue;
        };
        if let Some(other) = claimed.insert(to, plan.path.as_str()) {
            return Err(format!(
                "conflicting changes: '{other}' and '{}' are both renamed to '{to}'",
                plan.path
            ));
        }
        let kept_in_place = planned
            .iter()
            .any(|p| p.path == to && !vacated.contains(p.path.as_str()));
        if kept_in_place || (!vacated.contains(to) && target.join(to).exists()) {
            return Err(format!(
                "conflicting changes: '{}' is renamed to '{to}', which already exists",
                plan.path
            ));
        }
    }

    let mut written = Vec::new();
    let mut renamed = Vec::new();
    let mut deleted = Vec::new();
    let mut removals: Vec<PathBuf> = Vec::new();
    let mut writes: BTreeMap<String, &str> = BTreeMap::new();
    for plan in planned {
        if plan.delete {
            deleted.push(plan.path.clone());
            removals.push(target.join(&plan.path));
            task.diff(&plan.path, &plan.original, "");
            continue;
        }
        let changed = plan.text != plan.original;
        match &plan.rename_to {
            Some(to) => {
                renamed.push(json!({ "from": plan.path, "to": to }));
                removals.push(target.join(&plan.path));
                writes.insert(to.clone(), &plan.text);
                if changed {
                    written.push(to.clone());
                    task.diff(to, &plan.original, &plan.text);
                }
            }
            None if changed => {
                written.push(plan.path.clone());
                writes.insert(plan.path.clone(), &plan.text);
                task.diff(&plan.path, &plan.original, &plan.text);
            }
            None => {}
        }
    }

    if !dry_run {
        let _guard = task
            .run
            .commit_lock
            .lock()
            .map_err(|_| "commit lock poisoned".to_string())?;
        for path in &removals {
            fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for (path, text) in &writes {
            let full = target.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            fs::write(&full, text).map_err(|e| format!("{}: {e}", full.display()))?;
        }
    }

    let summary = &task.run.services.summary;
    summary.add_modified(written.len() + renamed.len() + deleted.len());
    Ok(json!({ "written": written, "renamed": renamed, "deleted": deleted }))
}

impl TaskContext<'_> {
    pub(crate) fn diff(&self, path: &str, original: &str, text: &str) {
        let diff = generate_unified_diff(
            Path::new(path),
            original,
            text,
            &DiffConfig {
                color: false,
                ..DiffConfig::default()
            },
            DiffMetadata {
                step_id: Some(self.position.to_string()),
                step_name: Some(self.label.clone()),
                ..DiffMetadata::default()
            },
        );
        self.run.services.summary.add_diff(diff);
    }
}
