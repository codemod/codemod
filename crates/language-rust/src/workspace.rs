//! A rust-analyzer database for one workspace: loaded once from the Cargo
//! project (or, without one, from the `.rs` files as standalone files), with
//! file texts kept in step with what the codemod sees.

use parking_lot::{Mutex, RwLock};
use ra_ap_ide::{AnalysisHost, FileId};
use ra_ap_ide_db::{base_db::SourceDatabase, RootDatabase};
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice};
use ra_ap_paths::{AbsPathBuf, Utf8PathBuf};
use ra_ap_project_model::{
    CargoConfig, ManifestPath, ProjectManifest, ProjectWorkspace, RustLibSource,
};
use ra_ap_vfs::{Vfs, VfsPath};
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Directories never searched for workspace files.
pub(crate) const SKIPPED_DIRS: &[&str] = &["target", "node_modules", ".git"];

/// How the workspace was loaded, for logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadedAs {
    /// `cargo metadata` with dependencies (from the local cache, offline).
    Cargo,
    /// `cargo metadata --no-deps`: the workspace's own crates only.
    CargoNoDeps,
    /// No Cargo project: each file on its own, with the standard library.
    Detached,
}

pub(crate) struct RaWorkspace {
    /// Changes go through the host; queries use clones of its database.
    host: Mutex<AnalysisHost>,
    paths: RwLock<FxHashMap<PathBuf, FileId>>,
    by_id: RwLock<FxHashMap<FileId, PathBuf>>,
    /// The files of the workspace's own crates (not dependencies, not std).
    local: RwLock<Vec<PathBuf>>,
}

fn cargo_config(no_deps: bool) -> CargoConfig {
    CargoConfig {
        sysroot: Some(RustLibSource::Discover),
        all_targets: true,
        set_test: true,
        no_deps,
        // Never touch the network: dependencies come from the local cache.
        metadata_extra_args: if no_deps {
            Vec::new()
        } else {
            vec!["--offline".to_string()]
        },
        ..CargoConfig::default()
    }
}

fn load_config() -> LoadCargoConfig {
    LoadCargoConfig {
        // No `cargo check`: build scripts are not run.
        load_out_dirs_from_check: false,
        // Proc macros (derives) are not expanded: no server process.
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: false,
        num_worker_threads: 1,
        proc_macro_processes: 1,
    }
}

fn abs(path: &Path) -> Option<AbsPathBuf> {
    let utf8 = Utf8PathBuf::from_path_buf(path.to_path_buf()).ok()?;
    AbsPathBuf::try_from(utf8).ok()
}

/// The `.rs` files under `root`, skipping `target/` and hidden directories.
pub(crate) fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                if !SKIPPED_DIRS.contains(&name.as_ref()) && !name.starts_with('.') {
                    stack.push(path);
                }
            } else if kind.is_file() && name.ends_with(".rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

impl RaWorkspace {
    /// Load the Cargo project at (or above) `root`; without one, `files`
    /// as standalone files.
    pub(crate) fn load(root: &Path, files: &[PathBuf]) -> Result<Self, String> {
        let progress = |_: String| {};
        let manifest = abs(root)
            .ok_or_else(|| format!("{} is not an absolute UTF-8 path", root.display()))
            .and_then(|root| ProjectManifest::discover_single(&root).map_err(|e| e.to_string()));
        if let Ok(manifest) = manifest {
            for (no_deps, loaded_as) in [(false, LoadedAs::Cargo), (true, LoadedAs::CargoNoDeps)] {
                let config = cargo_config(no_deps);
                match ProjectWorkspace::load(manifest.clone(), &config, &progress) {
                    Ok(workspace) => {
                        match ra_ap_load_cargo::load_workspace(
                            workspace,
                            &config.extra_env,
                            &load_config(),
                        ) {
                            Ok((db, vfs, _)) => return Ok(Self::new(db, vfs, root, loaded_as)),
                            Err(e) => log::debug!("rust-analyzer: loading {root:?} failed: {e}"),
                        }
                    }
                    Err(e) => {
                        log::debug!("rust-analyzer: cargo metadata for {root:?} failed: {e:#}")
                    }
                }
            }
        }
        Self::load_detached(root, files)
    }

    pub(crate) fn load_detached(root: &Path, files: &[PathBuf]) -> Result<Self, String> {
        let config = cargo_config(true);
        let manifests: Vec<ManifestPath> = files
            .iter()
            .filter_map(|file| abs(file))
            .filter_map(|file| ManifestPath::try_from(file).ok())
            .collect();
        let mut workspaces = ProjectWorkspace::load_detached_files(manifests, &config)
            .into_iter()
            .filter_map(Result::ok);
        let Some(first) = workspaces.next() else {
            return Err(format!("no Rust files to analyze under {}", root.display()));
        };
        // One detached workspace holds all the files.
        let (db, vfs, _) =
            ra_ap_load_cargo::load_workspace(first, &config.extra_env, &load_config())
                .map_err(|e| e.to_string())?;
        Ok(Self::new(db, vfs, root, LoadedAs::Detached))
    }

    fn new(db: RootDatabase, vfs: Vfs, root: &Path, loaded_as: LoadedAs) -> Self {
        let mut paths = FxHashMap::default();
        let mut by_id = FxHashMap::default();
        let mut local = Vec::new();
        for (file_id, vfs_path) in vfs.iter() {
            let Some(path) = vfs_path.as_path() else {
                continue;
            };
            let path = PathBuf::from(path.as_str());
            if path.extension().is_some_and(|e| e == "rs") {
                let source_root = db.file_source_root(file_id).source_root_id(&db);
                let is_library = db.source_root(source_root).source_root(&db).is_library;
                if !is_library && path.starts_with(root) {
                    local.push(path.clone());
                }
            }
            paths.insert(path.clone(), file_id);
            by_id.insert(file_id, path);
        }
        local.sort();
        log::debug!("rust-analyzer: {} loaded as {loaded_as:?}", root.display());
        RaWorkspace {
            host: Mutex::new(AnalysisHost::with_database(db)),
            paths: RwLock::new(paths),
            by_id: RwLock::new(by_id),
            local: RwLock::new(local),
        }
    }

    /// A database to query, for this thread.
    pub(crate) fn db(&self) -> RootDatabase {
        self.host.lock().raw_database().clone()
    }

    pub(crate) fn analysis(&self) -> ra_ap_ide::Analysis {
        self.host.lock().analysis()
    }

    /// Run `f` against a snapshot of the database. A file change made
    /// meanwhile (another step's text) cancels the run; it is run again on
    /// the new snapshot.
    pub(crate) fn with_db<T>(&self, f: impl Fn(&RootDatabase) -> T) -> T {
        loop {
            let db = self.db();
            let run = std::panic::AssertUnwindSafe(|| ra_ap_hir::attach_db(&db, || f(&db)));
            match ra_salsa::Cancelled::catch(run) {
                Ok(value) => return value,
                Err(_) => continue,
            }
        }
    }

    /// Ask the IDE layer, again if a change cancels the question.
    pub(crate) fn ask<T>(
        &self,
        f: impl Fn(&ra_ap_ide::Analysis) -> ra_ap_ide::Cancellable<T>,
    ) -> T {
        loop {
            if let Ok(value) = f(&self.analysis()) {
                return value;
            }
        }
    }

    pub(crate) fn file_id(&self, path: &Path) -> Option<FileId> {
        self.paths.read().get(path).copied()
    }

    pub(crate) fn path(&self, file_id: FileId) -> Option<PathBuf> {
        self.by_id.read().get(&file_id).cloned()
    }

    /// The workspace's own `.rs` files that rust-analyzer knows.
    pub(crate) fn local_files(&self) -> Vec<PathBuf> {
        self.local.read().clone()
    }

    pub(crate) fn is_local(&self, path: &Path) -> bool {
        self.local.read().binary_search(&path.to_path_buf()).is_ok()
    }

    /// The text rust-analyzer has for `path`.
    pub(crate) fn text(&self, path: &Path) -> Option<Arc<str>> {
        let file_id = self.file_id(path)?;
        let db = self.db();
        let text = db.file_text(file_id).text(&db);
        Some(Arc::from(&**text))
    }

    /// Make rust-analyzer see `text` for `path` (the file as the step
    /// started, which in a dry run differs from the disk).
    pub(crate) fn set_text(&self, path: &Path, text: &str) {
        let Some(file_id) = self.file_id(path) else {
            return;
        };
        if self.text(path).is_some_and(|old| &*old == text) {
            return;
        }
        let mut change = ra_ap_hir::ChangeWithProcMacros::default();
        change.change_file(file_id, Some(text.to_string()));
        self.host.lock().apply_change(change);
    }

    /// The `VfsPath` of a path, for messages.
    #[allow(dead_code)]
    pub(crate) fn vfs_path(path: &Path) -> Option<VfsPath> {
        abs(path).map(VfsPath::from)
    }
}
