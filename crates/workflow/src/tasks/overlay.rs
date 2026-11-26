//! A dry run's view of the target: what its steps would have changed so far.
//!
//! A dry run writes nothing, so without this each step would read the files
//! as they are on disk and miss what earlier steps changed. Instead, commits
//! in a dry run go here, and the steps that read files (`forEachFile`
//! callbacks, `astGrep`, `jssgTransform` in function and `dependencyGraph`
//! steps, `shard`'s file selection and filter, nested codemods) read
//! through it: they see the text earlier steps
//! left, files they renamed under their new paths, and not the files they
//! deleted. Shell steps do not run in a dry run, so they see none of it.
//! When the run ends, [`Overlay::changes`] is what it would have changed.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use butterflow_core::diff::{generate_unified_diff, DiffConfig, DiffMetadata, FileDiff};
use butterflow_core::execution::glob_overrides;
use codemod_sandbox::sandbox::engine::PendingFiles;
use ignore::Match;

use super::changes::{under, Change, PlannedChange};
use crate::target::TargetFiles;

pub(crate) struct Overlay {
    /// The run's target, as the run gives it.
    target: PathBuf,
    /// The target with links resolved, as files are kept under in
    /// [`PendingFiles`].
    root: PathBuf,
    /// By path relative to the target, with `/`: the file's text now, or
    /// `None` when it is gone (deleted, or renamed away).
    files: Mutex<BTreeMap<String, Option<String>>>,
    /// The snapshot the run reads instead of the disk, when it has one.
    snapshot: Option<Arc<dyn TargetFiles>>,
}

impl Overlay {
    pub fn new(target: &Path) -> Self {
        Self {
            target: target.to_path_buf(),
            root: std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf()),
            files: Mutex::default(),
            snapshot: None,
        }
    }

    /// The overlay of a run whose target is `snapshot`: nothing is read from
    /// disk, and `target` is only the name its paths are kept under.
    pub fn on_snapshot(target: &Path, snapshot: Arc<dyn TargetFiles>) -> Self {
        Self {
            target: target.to_path_buf(),
            root: target.to_path_buf(),
            files: Mutex::default(),
            snapshot: Some(snapshot),
        }
    }

    pub fn snapshot(&self) -> Option<&Arc<dyn TargetFiles>> {
        self.snapshot.as_ref()
    }

    /// `path` as the run started with it: from the snapshot, else the disk.
    fn read_base(&self, path: &str, file: &Path) -> io::Result<String> {
        match &self.snapshot {
            Some(snapshot) => snapshot
                .read(&[path])
                .pop()
                .flatten()
                .map(|text| text.to_string())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_string())),
            None => std::fs::read_to_string(file),
        }
    }

    fn base_is_file(&self, path: &str) -> bool {
        match &self.snapshot {
            Some(snapshot) => snapshot.is_file(path),
            None => under(&self.target, path).is_file(),
        }
    }

    fn base_exists(&self, path: &str) -> bool {
        match &self.snapshot {
            Some(snapshot) => snapshot.is_file(path) || snapshot.is_dir(path),
            None => under(&self.target, path).exists(),
        }
    }

    fn base_is_dir(&self, path: &str) -> bool {
        match &self.snapshot {
            Some(snapshot) => snapshot.is_dir(path),
            None => under(&self.target, path).is_dir(),
        }
    }

    /// The snapshot's files that `include` and `exclude` select, as the
    /// walk selects them on disk; `None` for a run on disk.
    pub fn select_snapshot(
        &self,
        include: &[String],
        exclude: &[String],
    ) -> Option<Result<Vec<String>, String>> {
        let snapshot = self.snapshot.as_ref()?;
        Some(glob_filter(
            &self.target,
            snapshot.paths().to_vec(),
            include,
            exclude,
        ))
    }

    fn files(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Option<String>>> {
        self.files.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What earlier steps made of `path`: `Some(None)` when they removed it,
    /// `None` when it is as on disk.
    pub fn get(&self, path: &str) -> Option<Option<String>> {
        self.files().get(path).cloned()
    }

    /// Whether an earlier step removed `path` (deleted it or renamed it away).
    pub fn is_gone(&self, path: &str) -> bool {
        matches!(self.files().get(path), Some(None))
    }

    /// The text of `path` as earlier steps left it: from here, else from
    /// `file` on disk.
    pub fn read(&self, path: &str, file: &Path) -> io::Result<String> {
        match self.get(path) {
            Some(Some(text)) => Ok(text),
            Some(None) => Err(gone(path)),
            None => self.read_base(path, file),
        }
    }

    /// The text of a file (absolute) as earlier steps left it.
    pub fn read_file(&self, file: &Path) -> io::Result<String> {
        match self.relative(file) {
            Some(path) => self.read(&path, file),
            None if self.snapshot.is_some() => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("'{}' is outside the snapshot", file.display()),
            )),
            None => std::fs::read_to_string(file),
        }
    }

    /// Whether `path` exists as earlier steps left it.
    pub fn exists(&self, path: &str) -> bool {
        match self.get(path) {
            Some(text) => text.is_some(),
            None => self.base_exists(path),
        }
    }

    /// Whether files may be moved to `destinations` as earlier steps left
    /// the target, as a real run would find it: no parent of one is a file
    /// (here, on disk, or another destination), and none is a directory.
    pub fn check_destinations(&self, destinations: &[&str]) -> Result<(), String> {
        let files = self.files();
        let is_file = |path: &str| match files.get(path) {
            Some(text) => text.is_some(),
            None => self.base_is_file(path),
        };
        for to in destinations {
            let parts: Vec<&str> = to.split('/').collect();
            for end in 1..parts.len() {
                let ancestor = parts[..end].join("/");
                if is_file(&ancestor) || destinations.contains(&ancestor.as_str()) {
                    return Err(format!(
                        "cannot move a file to '{to}': '{ancestor}' is a file"
                    ));
                }
            }
            let inside = format!("{to}/");
            let has_files = files
                .range(inside.clone()..)
                .take_while(|(path, _)| path.starts_with(&inside))
                .any(|(_, text)| text.is_some());
            if has_files || self.base_is_dir(to) {
                return Err(format!("cannot move a file to '{to}': it is a directory"));
            }
        }
        Ok(())
    }

    /// Record a step's checked plan as done.
    pub fn apply(&self, planned: &[PlannedChange]) {
        let mut files = self.files();
        // Files that move away first, so a file moved onto the path another
        // one left keeps its new text.
        for plan in planned {
            if !matches!(plan.change, Change::Edit(_)) {
                files.insert(plan.path.clone(), None);
            }
        }
        for plan in planned {
            match &plan.change {
                Change::Edit(text) => {
                    files.insert(plan.path.clone(), Some(text.clone()));
                }
                Change::Move { to, text } => {
                    files.insert(to.clone(), Some(text.clone()));
                }
                Change::Delete => {}
            }
        }
    }

    /// Files earlier steps created (renamed files' new paths) that `include`
    /// and `exclude` select, as the walk would have: relative paths, with
    /// `/`. Without `include`, every file not excluded.
    pub fn created(&self, include: &[String], exclude: &[String]) -> Result<Vec<String>, String> {
        self.created_in(None, include, exclude)
    }

    /// [`Overlay::created`] for a walk of `dir` (relative to the target, with
    /// `/`) instead of the whole target: the globs are relative to `dir`, and
    /// so are the paths returned.
    pub fn created_in(
        &self,
        dir: Option<&str>,
        include: &[String],
        exclude: &[String],
    ) -> Result<Vec<String>, String> {
        let root = match dir {
            Some(dir) => under(&self.target, dir),
            None => self.target.clone(),
        };
        let prefix = dir.map(|dir| format!("{dir}/"));
        let created: Vec<String> = self
            .files()
            .iter()
            .filter(|(path, text)| text.is_some() && !self.base_exists(path))
            .filter_map(|(path, _)| match &prefix {
                Some(prefix) => path.strip_prefix(prefix.as_str()).map(String::from),
                None => Some(path.clone()),
            })
            .collect();
        glob_filter(&root, created, include, exclude)
    }

    /// `path` (absolute, as [`PendingFiles`] keeps it) relative to the
    /// target, with `/`.
    fn relative(&self, path: &Path) -> Option<String> {
        let inside = match path
            .strip_prefix(&self.root)
            .or_else(|_| path.strip_prefix(&self.target))
        {
            Ok(inside) => inside.to_path_buf(),
            // A file on no disk (an earlier step renamed it here) is kept
            // under the path given: resolve its directory.
            Err(_) => {
                let parent = std::fs::canonicalize(path.parent()?).ok()?;
                parent
                    .strip_prefix(&self.root)
                    .ok()?
                    .join(path.file_name()?)
            }
        };
        let relative = inside.to_string_lossy().replace('\\', "/");
        (!relative.is_empty()).then_some(relative)
    }

    /// The step's unwritten files, for `jssgTransform`: a file earlier steps
    /// changed starts from their text.
    pub fn pending_files(self: &std::sync::Arc<Self>) -> PendingFiles {
        let overlay = std::sync::Arc::clone(self);
        PendingFiles::with_base(move |path| {
            let relative = overlay.relative(path)?;
            match overlay.get(&relative) {
                Some(Some(text)) => Some(Ok(text)),
                Some(None) => Some(Err(gone(&relative))),
                // A snapshot's files are never on disk.
                None if overlay.snapshot.is_some() => Some(overlay.read_base(&relative, path)),
                None => None,
            }
        })
    }

    /// What the run would change: `(path, before, after)` for each file whose
    /// text differs from the disk's, by path. A file that is not there is
    /// `None`, so a rename is the old path removed and the new one added.
    pub fn changes(&self) -> Vec<(String, Option<String>, Option<String>)> {
        self.files()
            .iter()
            .filter_map(|(path, after)| {
                let before = self.read_base(path, &under(&self.target, path)).ok();
                (before != *after).then(|| (path.clone(), before, after.clone()))
            })
            .collect()
    }
}

impl Overlay {
    /// [`Overlay::changes`] as unified diffs, without color.
    pub fn diffs(&self) -> Vec<FileDiff> {
        self.changes()
            .into_iter()
            .map(|(path, before, after)| {
                generate_unified_diff(
                    Path::new(&path),
                    before.as_deref().unwrap_or_default(),
                    after.as_deref().unwrap_or_default(),
                    &DiffConfig {
                        color: false,
                        ..DiffConfig::default()
                    },
                    DiffMetadata::default(),
                )
            })
            .collect()
    }
}

/// The `paths` (relative to `root`, with `/`) that `include` and `exclude`
/// select, as the walk's overrides would: an excluded directory leaves out
/// what is in it, and without `include` every path not excluded is kept.
fn glob_filter(
    root: &Path,
    paths: Vec<String>,
    include: &[String],
    exclude: &[String],
) -> Result<Vec<String>, String> {
    let Some(overrides) = glob_overrides(root, include, exclude)? else {
        return Ok(paths);
    };
    Ok(paths
        .into_iter()
        .filter(|path| {
            let full = under(root, path);
            let excluded_dir = full
                .ancestors()
                .skip(1)
                .take_while(|dir| dir.starts_with(root) && *dir != root)
                .any(|dir| matches!(overrides.matched(dir, true), Match::Ignore(_)));
            if excluded_dir {
                return false;
            }
            match overrides.matched(&full, false) {
                Match::Whitelist(_) => true,
                Match::Ignore(_) => false,
                Match::None => include.is_empty(),
            }
        })
        .collect())
}

fn gone(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("'{path}' was removed by an earlier step of this dry run"),
    )
}
