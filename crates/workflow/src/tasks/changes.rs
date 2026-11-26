//! Applying file changes: every task that edits files plans its changes,
//! then commits them here in one go, after conflict checks. Diffs and
//! counters go to the run summary.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use butterflow_core::diff::{generate_unified_diff, DiffConfig, DiffMetadata, FileDiff};
use serde_json::{json, Value};

use super::{parallel_map, TaskContext};

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

/// `relative` (with `/` separators) under `target`, joined part by part so
/// the separators are the platform's: on Windows, a `\\?\` target does not
/// take `/`.
pub(crate) fn under(target: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(target.to_path_buf(), |path, part| path.join(part))
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

/// Check existing path components, including the last one if it exists.
/// A dangling symlink is not a missing component that we may create.
fn check_containment(target: &Path, path: &str) -> Result<(), String> {
    let mut prefix = target.to_path_buf();
    for component in Path::new(path).components() {
        prefix.push(component);
        match fs::symlink_metadata(&prefix) {
            Ok(_) => {
                let resolved = fs::canonicalize(&prefix)
                    .map_err(|e| format!("cannot resolve '{}': {e}", prefix.display()))?;
                if !resolved.starts_with(target) {
                    return Err(format!(
                        "refusing to change '{path}': the path must stay inside the target"
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("cannot inspect '{}': {e}", prefix.display())),
        }
    }
    Ok(())
}

/// Check the plan, then apply it: to disk, or in a dry run to the run's
/// overlay, which later steps read. Returns `{ written, renamed, deleted }`.
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
    if task.canceled() {
        return Err("canceled".to_string());
    }
    let overlay = run.overlay.as_deref();
    // A snapshot has no links to leave it by: `normalized` already refused
    // paths that climb out.
    let on_snapshot = overlay.is_some_and(|overlay| overlay.snapshot().is_some());
    if !planned.is_empty() && !on_snapshot {
        let root = fs::canonicalize(target)
            .map_err(|e| format!("cannot resolve target '{}': {e}", target.display()))?;
        for plan in &planned {
            check_containment(&root, &plan.path)?;
            if let Change::Move { to, .. } = &plan.change {
                check_containment(&root, to)?;
            }
        }
    }
    let exists = |path: &str| match overlay {
        Some(overlay) => overlay.exists(path),
        None => target.join(path).exists(),
    };
    check_conflicts(target, &planned, &exists)?;
    if let Some(overlay) = overlay {
        let destinations: Vec<&str> = planned
            .iter()
            .filter_map(|plan| match &plan.change {
                Change::Move { to, .. } => Some(to.as_str()),
                _ => None,
            })
            .collect();
        overlay.check_destinations(&destinations)?;
    }
    let unchanged = parallel_map(&planned, |plan| match overlay {
        Some(overlay) => overlay
            .read(&plan.path, &target.join(&plan.path))
            .is_ok_and(|now| now == plan.original),
        None => fs::read(target.join(&plan.path))
            .is_ok_and(|on_disk| on_disk == plan.original.as_bytes()),
    });
    if let Some((plan, _)) = planned.iter().zip(unchanged).find(|(_, same)| !same) {
        return Err(format!(
            "'{}' changed on disk while this step ran (another step or command edited it); nothing was written",
            plan.path
        ));
    }

    match overlay {
        Some(overlay) => overlay.apply(&planned),
        None if !dry_run => {
            if !planned.is_empty() {
                run.before_write()?;
            }
            apply(target, &planned)?
        }
        None => {}
    }

    let mut written = Vec::new();
    let mut renamed = Vec::new();
    let mut deleted = Vec::new();
    // (path, before, after) of each file whose text changed, for the diffs.
    let mut diffs: Vec<(&str, &str, &str)> = Vec::new();
    for plan in &planned {
        match &plan.change {
            Change::Delete => {
                deleted.push(plan.path.clone());
                diffs.push((&plan.path, &plan.original, ""));
            }
            Change::Move { to, text } => {
                renamed.push(json!({ "from": plan.path, "to": to }));
                if *text != plan.original {
                    written.push(to.clone());
                    diffs.push((to, &plan.original, text));
                }
            }
            Change::Edit(text) if *text != plan.original => {
                written.push(plan.path.clone());
                diffs.push((&plan.path, &plan.original, text));
            }
            Change::Edit(_) => {}
        }
    }
    let summary = &run.services.summary;
    // A real run reports each file as it changes; a dry run reports what it
    // would have changed when it ends.
    let report = overlay.is_none() && !dry_run;
    for diff in parallel_map(&diffs, |(path, before, after)| {
        task.diff(path, before, after)
    }) {
        if report {
            let (text, diff_omitted) = if diff.diff_text.len() <= crate::events::STEP_DIFF_LIMIT {
                (diff.diff_text.clone(), None)
            } else {
                (
                    String::new(),
                    Some(crate::events::Omitted {
                        bytes: diff.diff_text.len(),
                        limit: crate::events::STEP_DIFF_LIMIT,
                    }),
                )
            };
            run.events.emit(crate::events::Event::StepDiff {
                at: task.at(),
                path: diff.path.clone(),
                diff: text,
                additions: diff.additions,
                deletions: diff.deletions,
                diff_omitted,
            });
        }
        summary.add_diff(diff);
    }
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
/// `exists`: whether a path (relative) is a file now.
fn check_conflicts(
    target: &Path,
    planned: &[PlannedChange],
    exists: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
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
            && exists(to)
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

#[cfg(all(test, unix))]
mod containment_tests {
    use super::*;

    #[test]
    fn containment_checks_existing_ancestors_of_new_paths() {
        let target = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = target.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("outside")).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), root.join("dangling")).unwrap();
        assert!(check_containment(&root, "outside/new/file").is_err());
        assert!(check_containment(&root, "dangling").is_err());
        assert!(check_containment(&root, "new/file").is_ok());
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
    /// The diff of one file this task changed, for the run summary.
    fn diff(&self, path: &str, original: &str, text: &str) -> FileDiff {
        generate_unified_diff(
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
        )
    }
}

/// Files a step's isolate changed with `jssgTransform`, as a plan for
/// [`commit`], refusing paths outside the target.
pub(crate) fn plan(
    task: &TaskContext<'_>,
    changes: &[butterflow_core::jssg_runner::JssgFileChange],
) -> Result<Vec<PlannedChange>, String> {
    let target = &task.run.target;
    // Changed files are kept under their canonical paths, so under the
    // canonical target they need no lookup of their own. Rename targets may
    // not be canonical (`..`, symbolic links) and are looked up in full.
    let root = fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let canonical = |path: &Path| {
        let inside = path.strip_prefix(&root).ok()?;
        inside
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
            .then(|| inside.to_string_lossy().replace('\\', "/"))
            .filter(|inside| !inside.is_empty())
    };
    let mut planned = Vec::new();
    for change in changes {
        let path = canonical(&change.path)
            .or_else(|| relative(target, &change.path))
            .ok_or_else(|| {
                format!(
                    "the step changed '{}', outside the target",
                    change.path.display()
                )
            })?;
        let rename_to = change
            .rename_to
            .as_ref()
            .map(|to| {
                relative(target, to).ok_or_else(|| {
                    format!(
                        "the step renamed '{path}' to '{}', outside the target",
                        to.display()
                    )
                })
            })
            .transpose()?;
        planned.push(PlannedChange::moved(
            path,
            change.original.clone(),
            change.content.clone(),
            rename_to,
        ));
    }
    Ok(planned)
}
