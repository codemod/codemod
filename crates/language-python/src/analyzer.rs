//! Python semantic analysis using Ruff's ty_ide.
//!
//! This module provides semantic analysis capabilities by delegating
//! to Ruff's ty_ide crate, which provides battle-tested goto-definition
//! and find-references functionality.

use crate::db::{create_db_with_files, PythonSemanticDb};
use crate::error::{PySemanticError, PySemanticResult};
use crate::ty::SnapshotSystem;
use language_core::{
    ByteRange, DefinitionKind, DefinitionOptions, DefinitionResult, FileReferences, FileSnapshot,
    ReferencesResult, SymbolKind, SymbolLocation,
};
use parking_lot::{Mutex, RwLock};
use ruff_db::source::source_text;
use ruff_db::system::SystemPathBuf;
use ruff_text_size::TextSize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ty_ide::{goto_definition, goto_references};

/// Convert ruff's TextRange to our ByteRange.
fn text_range_to_byte_range(range: ruff_text_size::TextRange) -> ByteRange {
    ByteRange::new(range.start().to_u32(), range.end().to_u32())
}

/// File-scope analyzer using ty_ide.
///
/// This analyzer creates a fresh Salsa database for each operation,
/// which ensures thread safety at the cost of some performance.
/// For most use cases, this trade-off is acceptable.
#[derive(Debug, Default)]
pub struct FileScopeAnalyzer {
    // No state - database is created per-operation with just the target file
}

impl FileScopeAnalyzer {
    /// Create a new file-scope analyzer.
    pub fn new() -> Self {
        Self {}
    }

    /// Clear the cache (no-op since we don't cache).
    pub fn clear_cache(&self) {
        // No-op: database is created per-operation
    }

    /// Process a file (no-op since we don't cache in file scope mode).
    pub fn process_file(&self, _path: &Path, _content: &str) -> PySemanticResult<()> {
        // No-op: database is created per-operation with just the target file
        Ok(())
    }

    /// Get definition for a symbol at the given range.
    pub fn get_definition(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
        _options: DefinitionOptions,
    ) -> PySemanticResult<Option<DefinitionResult>> {
        let workspace_root = path.parent().unwrap_or(path);

        // create file contents map with just this file
        let mut file_contents = HashMap::new();
        file_contents.insert(path.to_path_buf(), content.to_string());

        let db = create_db_with_files(workspace_root, &file_contents)
            .map_err(|e| PySemanticError::Other(e.to_string()))?;

        let file = db
            .get_file(path)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: path.to_path_buf(),
            })?;

        let offset = TextSize::from(range.start);
        let result = goto_definition(&db, file, offset);

        if let Some(ranged_targets) = result {
            if let Some(target) = ranged_targets.value.into_iter().next() {
                let target_path = target.file().path(&db);
                let target_content = source_text(&db, target.file()).to_string();
                let focus_range = text_range_to_byte_range(target.focus_range());

                let name = target_content
                    .get(focus_range.start as usize..focus_range.end as usize)
                    .unwrap_or("")
                    .to_string();

                let location = SymbolLocation::new(
                    PathBuf::from(target_path.as_str()),
                    focus_range,
                    SymbolKind::Unknown, // ty_ide doesn't provide kind in NavigationTarget
                    name,
                );

                let kind = if target_path.as_str() == path.to_string_lossy().as_ref() {
                    DefinitionKind::Local
                } else {
                    DefinitionKind::External
                };

                return Ok(Some(DefinitionResult::new(location, target_content, kind)));
            }
        }

        Ok(None)
    }

    /// Find all references to a symbol at the given range.
    pub fn find_references(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
    ) -> PySemanticResult<ReferencesResult> {
        let workspace_root = path.parent().unwrap_or(path);

        // create file contents map with just this file
        let mut file_contents = HashMap::new();
        file_contents.insert(path.to_path_buf(), content.to_string());

        let db = create_db_with_files(workspace_root, &file_contents)
            .map_err(|e| PySemanticError::Other(e.to_string()))?;

        let file = db
            .get_file(path)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: path.to_path_buf(),
            })?;

        let offset = TextSize::from(range.start);
        let refs = goto_references(&db, file, offset, true); // include declaration

        let mut result = ReferencesResult::new();

        if let Some(reference_list) = refs {
            // group references by file
            let mut file_refs: HashMap<PathBuf, Vec<SymbolLocation>> = HashMap::new();

            for ref_target in reference_list {
                let ref_path = PathBuf::from(ref_target.file().path(&db).as_str());
                let ref_range = text_range_to_byte_range(ref_target.range());

                // get content for name extraction
                let ref_content = source_text(&db, ref_target.file()).to_string();
                let name = ref_content
                    .get(ref_range.start as usize..ref_range.end as usize)
                    .unwrap_or("")
                    .to_string();

                let location =
                    SymbolLocation::new(ref_path.clone(), ref_range, SymbolKind::Unknown, name);

                file_refs.entry(ref_path).or_default().push(location);
            }

            // convert to FileReferences
            for (file_path, locations) in file_refs {
                let file_for_content = db.get_file(&file_path);
                let file_content = file_for_content
                    .map(|f| source_text(&db, f).to_string())
                    .unwrap_or_default();
                result
                    .files
                    .push(FileReferences::new(file_path, file_content, locations));
            }
        }

        Ok(result)
    }

    /// Get cache stats (always empty since we don't cache).
    pub fn cache(&self) -> CacheStats {
        CacheStats { len: 0 }
    }
}

/// Workspace-scope analyzer using ty_ide.
///
/// One ty database per step, built on the first question and shared by every
/// later one (clones share its caches), so a references query reads and
/// indexes each file once per step instead of once per query. On a real file
/// system it holds every Python file under the workspace root, read through
/// the step's snapshot, so a file the run did not tell the provider about
/// (the file a step is running on, say) is still searched. On a virtual file
/// system it holds the files the run told the provider about.
pub struct WorkspaceScopeAnalyzer {
    workspace_root: PathBuf,
    /// The texts the run told the analyzer about, or a query passed in.
    file_contents: RwLock<HashMap<PathBuf, String>>,
    /// The step's file texts, on a real file system.
    snapshot: Option<Arc<FileSnapshot>>,
    /// Built on first use; dropped when a text it may have read changes.
    db: Mutex<Option<Box<PythonSemanticDb>>>,
}

impl WorkspaceScopeAnalyzer {
    /// An analyzer over the files the run tells it about.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            file_contents: RwLock::new(HashMap::new()),
            snapshot: None,
            db: Mutex::new(None),
        }
    }

    /// An analyzer over the workspace on disk, read through `snapshot`.
    pub fn over_snapshot(workspace_root: PathBuf, snapshot: Arc<FileSnapshot>) -> Self {
        Self {
            snapshot: Some(snapshot),
            ..Self::new(workspace_root)
        }
    }

    /// Get the workspace root.
    #[allow(dead_code)]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Clear the cache.
    pub fn clear(&self) {
        self.file_contents.write().clear();
        *self.db.lock() = None;
    }

    /// Process a file (store content for cross-file analysis).
    pub fn process_file(&self, path: &Path, content: &str) -> PySemanticResult<()> {
        // Canonicalize the path to handle symlinks (e.g., /var -> /private/var on macOS)
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.see(canonical, content);
        Ok(())
    }

    /// Record `content` as `path`'s text; a database that may have read
    /// another text for it is dropped.
    fn see(&self, path: PathBuf, content: &str) {
        let changed = {
            let mut contents = self.file_contents.write();
            if contents.get(&path).is_some_and(|known| known == content) {
                false
            } else {
                contents.insert(path.clone(), content.to_string());
                true
            }
        };
        if let Some(snapshot) = &self.snapshot {
            snapshot.pin(&path, content);
        }
        if changed {
            *self.db.lock() = None;
        }
    }

    /// The step's database, with `path` at `content` (the text the question
    /// is about).
    fn db_for(&self, path: &Path, content: &str) -> PySemanticResult<PythonSemanticDb> {
        let current = match &self.snapshot {
            Some(snapshot) => snapshot
                .get_or_read(path, || std::fs::read_to_string(path))
                .ok()
                .is_some_and(|text| &*text == content),
            None => self
                .file_contents
                .read()
                .get(path)
                .is_some_and(|known| known == content),
        };
        if !current {
            self.see(path.to_path_buf(), content);
        }
        let mut slot = self.db.lock();
        if let Some(db) = slot.as_deref() {
            return Ok(db.clone());
        }
        let db = match &self.snapshot {
            Some(snapshot) => {
                let root =
                    SystemPathBuf::from_path_buf(self.workspace_root.clone()).map_err(|path| {
                        PySemanticError::Other(format!("not a UTF-8 path: {}", path.display()))
                    })?;
                let system = SnapshotSystem::new(&root, Arc::clone(snapshot));
                PythonSemanticDb::over(&root, Arc::new(system))
            }
            None => create_db_with_files(&self.workspace_root, &self.file_contents.read()),
        }
        .map_err(|e| PySemanticError::Other(e.to_string()))?;
        *slot = Some(Box::new(db.clone()));
        Ok(db)
    }

    /// Get definition for a symbol at the given range.
    pub fn get_definition(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
        _options: DefinitionOptions,
    ) -> PySemanticResult<Option<DefinitionResult>> {
        // Canonicalize the path to handle symlinks (e.g., /var -> /private/var on macOS)
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // add current file to contents
        let db = self.db_for(&canonical, content)?;

        let file = db
            .get_file(&canonical)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: canonical.clone(),
            })?;

        let offset = TextSize::from(range.start);
        let result = goto_definition(&db, file, offset);

        if let Some(ranged_targets) = result {
            if let Some(target) = ranged_targets.value.into_iter().next() {
                let target_path = target.file().path(&db);
                let target_content = source_text(&db, target.file()).to_string();
                let focus_range = text_range_to_byte_range(target.focus_range());

                let name = target_content
                    .get(focus_range.start as usize..focus_range.end as usize)
                    .unwrap_or("")
                    .to_string();

                let location = SymbolLocation::new(
                    PathBuf::from(target_path.as_str()),
                    focus_range,
                    SymbolKind::Unknown,
                    name,
                );

                let kind = if target_path.as_str() == path.to_string_lossy().as_ref() {
                    DefinitionKind::Local
                } else {
                    DefinitionKind::External
                };

                return Ok(Some(DefinitionResult::new(location, target_content, kind)));
            }
        }

        Ok(None)
    }

    /// Find all references to a symbol at the given range.
    pub fn find_references(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
    ) -> PySemanticResult<ReferencesResult> {
        // Canonicalize the path to handle symlinks (e.g., /var -> /private/var on macOS)
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        let db = self.db_for(&canonical, content)?;

        let file = db
            .get_file(&canonical)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: canonical.clone(),
            })?;

        let offset = TextSize::from(range.start);
        let refs = goto_references(&db, file, offset, true);

        let mut result = ReferencesResult::new();

        if let Some(reference_list) = refs {
            let mut file_refs: HashMap<PathBuf, Vec<SymbolLocation>> = HashMap::new();

            for ref_target in reference_list {
                let ref_path = PathBuf::from(ref_target.file().path(&db).as_str());
                let ref_range = text_range_to_byte_range(ref_target.range());

                let ref_content = source_text(&db, ref_target.file()).to_string();
                let name = ref_content
                    .get(ref_range.start as usize..ref_range.end as usize)
                    .unwrap_or("")
                    .to_string();

                let location =
                    SymbolLocation::new(ref_path.clone(), ref_range, SymbolKind::Unknown, name);

                file_refs.entry(ref_path).or_default().push(location);
            }

            for (file_path, locations) in file_refs {
                let file_for_content = db.get_file(&file_path);
                let file_content = file_for_content
                    .map(|f| source_text(&db, f).to_string())
                    .unwrap_or_default();
                result
                    .files
                    .push(FileReferences::new(file_path, file_content, locations));
            }
        }

        Ok(result)
    }

    /// Get cache stats.
    pub fn cache(&self) -> CacheStats {
        CacheStats {
            len: self.file_contents.read().len(),
        }
    }
}

impl std::fmt::Debug for WorkspaceScopeAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceScopeAnalyzer")
            .field("workspace_root", &self.workspace_root)
            .field("file_count", &self.file_contents.read().len())
            .finish()
    }
}

/// Cache statistics.
pub struct CacheStats {
    len: usize,
}

impl CacheStats {
    pub fn len(&self) -> usize {
        self.len
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[allow(dead_code)]
    pub fn contains(&self, _path: &Path) -> bool {
        self.len > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_file_scope_analyzer_process_file() {
        let analyzer = FileScopeAnalyzer::new();
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");

        let content = r#"
x = 1
y = x + 2
"#;
        fs::write(&file_path, content).unwrap();

        let result = analyzer.process_file(&file_path, content);
        assert!(result.is_ok());
    }

    #[test]
    fn test_workspace_scope_analyzer_new() {
        let dir = TempDir::new().unwrap();
        let analyzer = WorkspaceScopeAnalyzer::new(dir.path().to_path_buf());
        assert_eq!(analyzer.workspace_root(), dir.path());
    }
}
