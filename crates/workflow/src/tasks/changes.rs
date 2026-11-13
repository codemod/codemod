//! Applying file changes: every task that edits files plans its changes,
//! then commits them here in one go, after conflict checks. Diffs and
//! counters go to the run summary.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use butterflow_core::diff::{generate_unified_diff, DiffConfig, DiffMetadata};
use serde_json::{json, Value};

use super::TaskContext;

/// What happens to one file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Change {
    /// The file stays where it is, with this text.
    Edit(String),
    /// The file moves to `to` (relative to the target), with this text.
    Move {
        to: String,
        text: String,
    },
    Delete,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedChange {
    /// Relative to the target, with `/` separators.
    pub path: String,
    /// The file's text when the task read it.
    pub original: String,
    pub change: Change,
}

impl PlannedChange {
    pub fn edit(path: String, original: String, text: String) -> Self {
        Self {
            path,
            original,
            change: Change::Edit(text),
        }
    }

    /// The file with `text`, at `to` when that is another path.
    pub fn moved(path: String, original: String, text: String, to: Option<String>) -> Self {
        let change = match to.filter(|to| *to != path) {
            Some(to) => Change::Move { to, text },
            None => Change::Edit(text),
        };
        Self {
            path,
            original,
            change,
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

/// A relative path in one spelling: `/` separators, no `.` parts. `None`
/// when it is empty, absolute, or climbs out with `..`.
pub(crate) fn normalize(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let mut parts = Vec::new();
    for component in Path::new(&path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

pub(crate) fn is_safe_relative(path: &str) -> bool {
    normalize(path).is_some()
}

/// A task's `basePath`: a directory under the target, or the target itself
/// (`None`) for `.`, `./` and nothing.
pub(crate) fn base_path(base: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(base) = base.map(str::trim) else {
        return Ok(None);
    };
    if base.is_empty() || base.trim_end_matches('/') == "." {
        return Ok(None);
    }
    normalize(base)
        .map(|base| Some(PathBuf::from(base)))
        .ok_or_else(|| format!("basePath '{base}' must stay inside the target"))
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

/// Check the plan, then apply it unless `dry_run`. Returns
/// `{ written, renamed, deleted }`.
///
/// Nothing is applied when a path leaves the target, two changes collide, a
/// file changed on disk since the task read it (another step edited it), or
/// the run was canceled. A write that fails part-way is rolled back.
pub(crate) fn commit(
    task: &TaskContext<'_>,
    planned: &[PlannedChange],
    dry_run: bool,
) -> Result<Value, String> {
    let run = task.run;
    let target = &run.target;
    let planned = normalized(planned)?;

    // One commit at a time: the checks below must still hold when we write.
    let _guard = run
        .commit_lock
        .lock()
        .map_err(|_| "commit lock poisoned".to_string())?;
    if run.canceled() {
        return Err("canceled".to_string());
    }
    check_conflicts(target, &planned)?;
    for plan in &planned {
        let on_disk = fs::read_to_string(target.join(&plan.path)).ok();
        if on_disk.as_deref() != Some(plan.original.as_str()) {
            return Err(format!(
                "'{}' changed on disk while this step ran (another step or command edited it); nothing was written",
                plan.path
            ));
        }
    }

    if !dry_run {
        apply(target, &planned)?;
    }

    let mut written = Vec::new();
    let mut renamed = Vec::new();
    let mut deleted = Vec::new();
    for plan in &planned {
        match &plan.change {
            Change::Delete => {
                deleted.push(plan.path.clone());
                task.diff(&plan.path, &plan.original, "");
            }
            Change::Move { to, text } => {
                renamed.push(json!({ "from": plan.path, "to": to }));
                if *text != plan.original {
                    written.push(to.clone());
                    task.diff(to, &plan.original, text);
                }
            }
            Change::Edit(text) if *text != plan.original => {
                written.push(plan.path.clone());
                task.diff(&plan.path, &plan.original, text);
            }
            Change::Edit(_) => {}
        }
    }
    let summary = &run.services.summary;
    summary.add_modified(written.len() + renamed.len() + deleted.len());
    Ok(json!({ "written": written, "renamed": renamed, "deleted": deleted }))
}

/// The plan with every path in one spelling, refusing paths outside the
/// target and files planned twice.
fn normalized(planned: &[PlannedChange]) -> Result<Vec<PlannedChange>, String> {
    let outside =
        |path: &str| format!("refusing to change '{path}': the path must stay inside the target");
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(planned.len());
    for plan in planned {
        let path = normalize(&plan.path).ok_or_else(|| outside(&plan.path))?;
        if !seen.insert(path.clone()) {
            return Err(format!(
                "conflicting changes: '{path}' is changed more than once in this step"
            ));
        }
        let change = match &plan.change {
            Change::Move { to, text } => {
                let to = normalize(to).ok_or_else(|| outside(to))?;
                if to == path {
                    Change::Edit(text.clone())
                } else {
                    Change::Move {
                        to,
                        text: text.clone(),
                    }
                }
            }
            other => other.clone(),
        };
        result.push(PlannedChange {
            path,
            original: plan.original.clone(),
            change,
        });
    }
    Ok(result)
}

/// Two files moved to one path, or a file moved onto one that stays.
fn check_conflicts(target: &Path, planned: &[PlannedChange]) -> Result<(), String> {
    let vacated: HashSet<&str> = planned
        .iter()
        .filter(|p| !matches!(p.change, Change::Edit(_)))
        .map(|p| p.path.as_str())
        .collect();
    let planned_paths: HashSet<&str> = planned.iter().map(|p| p.path.as_str()).collect();
    let mut claimed: HashMap<&str, &str> = HashMap::new();
    for plan in planned {
        let Change::Move { to, .. } = &plan.change else {
            continue;
        };
        if let Some(other) = claimed.insert(to, plan.path.as_str()) {
            return Err(format!(
                "conflicting changes: '{other}' and '{}' are both renamed to '{to}'",
                plan.path
            ));
        }
        let stays_planned = planned_paths.contains(to.as_str()) && !vacated.contains(to.as_str());
        let stays_on_disk = !vacated.contains(to.as_str())
            && target.join(to).exists()
            && !same_file(&target.join(&plan.path), &target.join(to));
        if stays_planned || stays_on_disk {
            return Err(format!(
                "conflicting changes: '{}' is renamed to '{to}', which already exists",
                plan.path
            ));
        }
    }
    Ok(())
}

/// Whether two paths name one file: a rename that only changes letter case on
/// a case-insensitive file system.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

/// How to take one applied operation back.
enum Undo {
    /// Put this text back (the file was overwritten or removed).
    Restore(PathBuf, String),
    /// Remove a file the commit created.
    Remove(PathBuf),
    /// Rename back (a rename that only changed letter case).
    Rename { from: PathBuf, to: PathBuf },
}

/// Write the plan to disk: new contents first, then the files that moved
/// away or were deleted. If anything fails, what was done is undone.
fn apply(target: &Path, planned: &[PlannedChange]) -> Result<(), String> {
    let mut undo = Vec::new();
    let result = apply_steps(target, planned, &mut undo);
    if result.is_err() {
        for step in undo.into_iter().rev() {
            // Best effort: the original error is the one to report.
            let _ = match step {
                Undo::Restore(path, text) => fs::write(path, text),
                Undo::Remove(path) => fs::remove_file(path),
                Undo::Rename { from, to } => fs::rename(from, to),
            };
        }
    }
    result
}

fn apply_steps(
    target: &Path,
    planned: &[PlannedChange],
    undo: &mut Vec<Undo>,
) -> Result<(), String> {
    let originals: HashMap<&str, &str> = planned
        .iter()
        .map(|p| (p.path.as_str(), p.original.as_str()))
        .collect();
    let mut rewritten: HashSet<&str> = HashSet::new();
    for plan in planned {
        let (to, text) = match &plan.change {
            Change::Edit(text) if *text == plan.original => continue,
            Change::Edit(text) => (plan.path.as_str(), text),
            Change::Move { to, text } => (to.as_str(), text),
            Change::Delete => continue,
        };
        let from = target.join(&plan.path);
        let full = target.join(to);
        let case_only = to != plan.path && same_file(&from, &full);
        if case_only {
            fs::rename(&from, &full).map_err(|e| format!("{}: {e}", from.display()))?;
            undo.push(Undo::Rename {
                from: full.clone(),
                to: from,
            });
            // The old spelling is gone with the rename; nothing to remove.
            rewritten.insert(plan.path.as_str());
        }
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        // What the path held before: this file's own text after a case-only
        // rename, else the text of the planned file that is moving away.
        let before = if case_only {
            Some(plan.original.as_str())
        } else {
            originals.get(to).copied().filter(|_| full.exists())
        };
        fs::write(&full, text).map_err(|e| format!("{}: {e}", full.display()))?;
        undo.push(match before {
            Some(original) => Undo::Restore(full, original.to_string()),
            None => Undo::Remove(full),
        });
        rewritten.insert(to);
    }
    for plan in planned {
        if matches!(plan.change, Change::Edit(_)) || rewritten.contains(plan.path.as_str()) {
            continue;
        }
        let full = target.join(&plan.path);
        fs::remove_file(&full).map_err(|e| format!("{}: {e}", full.display()))?;
        undo.push(Undo::Restore(full, plan.original.clone()));
    }
    Ok(())
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
