//! Main Ruff semantic provider implementation for Python.

use crate::analyzer::{FileScopeAnalyzer, WorkspaceScopeAnalyzer};
use language_core::{
    filesystem, ByteRange, DefinitionOptions, DefinitionResult, ProviderMode, ReferencesResult,
    SemanticProvider, SemanticResult,
};
use std::path::{Path, PathBuf};
use vfs::VfsPath;

/// Inner analyzer type for the provider.
enum AnalyzerKind {
    /// FileScope mode analyzer (single-file analysis)
    FileScope(FileScopeAnalyzer),
    /// WorkspaceScope mode analyzer (workspace-wide analysis)
    WorkspaceScope(WorkspaceScopeAnalyzer),
}

/// Semantic analysis provider for Python using Ruff's ty_ide.
///
/// This provider supports two modes:
/// - **FileScope**: Single-file analysis with no cross-file resolution.
/// - **WorkspaceScope**: Workspace-wide analysis with cross-file support.
///
/// Under the hood, both modes use Ruff's ty_ide crate which provides
/// battle-tested semantic analysis with Salsa-based incremental computation.
///
/// The provider uses a virtual filesystem abstraction, allowing it to work with
/// either real filesystems or in-memory filesystems (useful for testing or
/// environments like pg_ast_grep).
pub struct RuffSemanticProvider {
    /// The analyzer implementation
    analyzer: AnalyzerKind,
    /// Virtual filesystem root for file operations
    fs_root: VfsPath,
    /// Physical root path for converting absolute paths to relative paths.
    /// Only used when fs_root is PhysicalFS.
    physical_root: Option<PathBuf>,
}

impl RuffSemanticProvider {
    /// Create a file-scope provider for single-file analysis.
    ///
    /// This mode is best for:
    /// - Quick dry runs
    /// - High-level analysis
    /// - Single-file transformations
    /// - When cross-file references are not needed
    ///
    /// Uses the real filesystem (PhysicalFS) with the current directory as root.
    pub fn file_scope() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            analyzer: AnalyzerKind::FileScope(FileScopeAnalyzer::new()),
            fs_root: filesystem::physical_path(&cwd),
            physical_root: Some(cwd),
        }
    }

    /// Create a file-scope provider with a custom virtual filesystem.
    ///
    /// This is useful for:
    /// - Testing with in-memory filesystems
    /// - Running in environments without real filesystem access (e.g., pg_ast_grep)
    ///
    /// # Arguments
    ///
    /// * `fs_root` - The virtual filesystem root to use for file operations
    pub fn file_scope_with_fs(fs_root: VfsPath) -> Self {
        Self {
            analyzer: AnalyzerKind::FileScope(FileScopeAnalyzer::new()),
            fs_root,
            physical_root: None, // MemoryFS or other VFS - paths are virtual
        }
    }

    /// Create a workspace-scope provider for workspace-wide analysis.
    ///
    /// This mode is best for:
    /// - Full codemod runs requiring cross-file references
    /// - Precise symbol resolution
    /// - When you need to find all usages of a symbol across the workspace
    ///
    /// Uses the real filesystem (PhysicalFS) with the workspace root.
    pub fn workspace_scope(workspace_root: PathBuf) -> Self {
        // Canonicalize workspace root to handle symlinks (e.g., /var -> /private/var on macOS)
        let canonical_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.clone());
        let fs_root = filesystem::physical_path(&canonical_root);
        Self {
            analyzer: AnalyzerKind::WorkspaceScope(WorkspaceScopeAnalyzer::new(
                canonical_root.clone(),
            )),
            fs_root,
            physical_root: Some(canonical_root),
        }
    }

    /// Create a workspace-scope provider with a custom virtual filesystem.
    ///
    /// # Arguments
    ///
    /// * `workspace_root` - The workspace root path for module resolution
    /// * `fs_root` - The virtual filesystem root to use for file operations
    pub fn workspace_scope_with_fs(workspace_root: PathBuf, fs_root: VfsPath) -> Self {
        Self {
            analyzer: AnalyzerKind::WorkspaceScope(WorkspaceScopeAnalyzer::new(workspace_root)),
            fs_root,
            physical_root: None, // Custom VFS - paths handled by VFS implementation
        }
    }

    /// Clear all cached data.
    pub fn clear_cache(&self) {
        match &self.analyzer {
            AnalyzerKind::FileScope(analyzer) => analyzer.clear_cache(),
            AnalyzerKind::WorkspaceScope(analyzer) => analyzer.clear(),
        }
    }

    /// Get the number of cached files.
    /// With Salsa, this returns 1 if database is initialized, 0 otherwise.
    pub fn cached_file_count(&self) -> usize {
        match &self.analyzer {
            AnalyzerKind::FileScope(analyzer) => analyzer.cache().len(),
            AnalyzerKind::WorkspaceScope(analyzer) => analyzer.cache().len(),
        }
    }

    /// Read file content using the virtual filesystem.
    fn read_file(&self, file_path: &Path) -> SemanticResult<String> {
        // Canonicalize the file path to handle symlinks (e.g., /var -> /private/var on macOS)
        let canonical_file = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());

        // For PhysicalFS, try to convert to a path relative to the physical root.
        // If the file is outside the root, read it directly using its absolute path.
        let (vfs_root, relative_path) = if let Some(ref root) = self.physical_root {
            match canonical_file.strip_prefix(root) {
                Ok(rel) => (&self.fs_root, rel.to_path_buf()),
                Err(_) => {
                    // File is outside the VFS root, create a VFS at the file's parent directory
                    let parent = canonical_file.parent().unwrap_or(Path::new("/"));
                    let file_name = canonical_file
                        .file_name()
                        .ok_or_else(|| language_core::SemanticError::FileRead {
                            path: file_path.to_path_buf(),
                            message: "Invalid file path".to_string(),
                        })?;
                    let temp_root = filesystem::physical_path(parent);
                    let path_str = file_name.to_string_lossy();
                    let vfs_path = temp_root
                        .join(&*path_str)
                        .map_err(|e| language_core::SemanticError::FileRead {
                            path: file_path.to_path_buf(),
                            message: e.to_string(),
                        })?;
                    return filesystem::read_to_string(&vfs_path).map_err(|e| {
                        language_core::SemanticError::FileRead {
                            path: file_path.to_path_buf(),
                            message: e.to_string(),
                        }
                    });
                }
            }
        } else {
            (&self.fs_root, file_path.to_path_buf())
        };

        let path_str = relative_path.to_string_lossy();
        let vfs_path = vfs_root
            .join(&*path_str)
            .map_err(|e| language_core::SemanticError::FileRead {
                path: file_path.to_path_buf(),
                message: e.to_string(),
            })?;

        filesystem::read_to_string(&vfs_path).map_err(|e| language_core::SemanticError::FileRead {
            path: file_path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Get the virtual filesystem root used by this provider.
    pub fn fs_root(&self) -> &VfsPath {
        &self.fs_root
    }
}

impl std::fmt::Debug for RuffSemanticProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.analyzer {
            AnalyzerKind::FileScope(_) => write!(f, "RuffSemanticProvider::FileScope"),
            AnalyzerKind::WorkspaceScope(_) => write!(f, "RuffSemanticProvider::WorkspaceScope"),
        }
    }
}

impl SemanticProvider for RuffSemanticProvider {
    fn get_definition(
        &self,
        file_path: &Path,
        range: ByteRange,
        options: DefinitionOptions,
    ) -> SemanticResult<Option<DefinitionResult>> {
        // We need the file content for analysis
        let content = self.read_file(file_path)?;

        match &self.analyzer {
            AnalyzerKind::FileScope(analyzer) => analyzer
                .get_definition(file_path, &content, range, options)
                .map_err(Into::into),
            AnalyzerKind::WorkspaceScope(analyzer) => analyzer
                .get_definition(file_path, &content, range, options)
                .map_err(Into::into),
        }
    }

    fn find_references(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        let content = self.read_file(file_path)?;

        match &self.analyzer {
            AnalyzerKind::FileScope(analyzer) => analyzer
                .find_references(file_path, &content, range)
                .map_err(Into::into),
            AnalyzerKind::WorkspaceScope(analyzer) => analyzer
                .find_references(file_path, &content, range)
                .map_err(Into::into),
        }
    }

    fn get_type(&self, _file_path: &Path, _range: ByteRange) -> SemanticResult<Option<String>> {
        // Type inference is available through ty_ide but not exposed yet
        // Could be added in the future using ty_ide::hover
        Ok(None)
    }

    fn notify_file_processed(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        match &self.analyzer {
            AnalyzerKind::FileScope(analyzer) => analyzer
                .process_file(file_path, content)
                .map_err(Into::into),
            AnalyzerKind::WorkspaceScope(analyzer) => analyzer
                .process_file(file_path, content)
                .map_err(Into::into),
        }
    }

    fn supports_language(&self, lang: &str) -> bool {
        matches!(lang.to_lowercase().as_str(), "python" | "py")
    }

    fn mode(&self) -> ProviderMode {
        match &self.analyzer {
            AnalyzerKind::FileScope(_) => ProviderMode::FileScope,
            AnalyzerKind::WorkspaceScope(_) => ProviderMode::WorkspaceScope,
        }
    }
}

/// Helper to get definition with content provided (avoids re-reading file).
impl RuffSemanticProvider {
    /// Get definition with content already available.
    pub fn get_definition_with_content(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: DefinitionOptions,
    ) -> SemanticResult<Option<DefinitionResult>> {
        match &self.analyzer {
            AnalyzerKind::FileScope(analyzer) => analyzer
                .get_definition(file_path, content, range, options)
                .map_err(Into::into),
            AnalyzerKind::WorkspaceScope(analyzer) => analyzer
                .get_definition(file_path, content, range, options)
                .map_err(Into::into),
        }
    }

    /// Find references with content already available.
    pub fn find_references_with_content(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        match &self.analyzer {
            AnalyzerKind::FileScope(analyzer) => analyzer
                .find_references(file_path, content, range)
                .map_err(Into::into),
            AnalyzerKind::WorkspaceScope(analyzer) => analyzer
                .find_references(file_path, content, range)
                .map_err(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_file_scope_provider_supports_language() {
        let provider = RuffSemanticProvider::file_scope();
        assert!(provider.supports_language("python"));
        assert!(provider.supports_language("Python"));
        assert!(provider.supports_language("py"));
        assert!(!provider.supports_language("javascript"));
        assert!(!provider.supports_language("rust"));
    }

    #[test]
    fn test_file_scope_provider_mode() {
        let provider = RuffSemanticProvider::file_scope();
        assert_eq!(provider.mode(), ProviderMode::FileScope);
    }

    #[test]
    fn test_workspace_scope_provider_mode() {
        let dir = TempDir::new().unwrap();
        let provider = RuffSemanticProvider::workspace_scope(dir.path().to_path_buf());
        assert_eq!(provider.mode(), ProviderMode::WorkspaceScope);
    }

    #[test]
    fn test_provider_notify_file_processed() {
        let provider = RuffSemanticProvider::file_scope();

        let content = "x = 1";
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");
        fs::write(&file_path, content).unwrap();

        let result = provider.notify_file_processed(&file_path, content);
        assert!(result.is_ok());
    }

    #[test]
    fn test_provider_clear_cache() {
        let provider = RuffSemanticProvider::file_scope();

        let content = "x = 1";
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");
        fs::write(&file_path, content).unwrap();

        let _ = provider.notify_file_processed(&file_path, content);
        provider.clear_cache();
        assert_eq!(provider.cached_file_count(), 0);
    }

    // VFS (Virtual FileSystem) tests

    #[test]
    fn test_file_scope_with_memory_fs() {
        use std::io::Write;
        use std::path::PathBuf;

        let fs_root = filesystem::memory_fs();

        // Create a file in the memory filesystem
        let file = fs_root.join("test.py").unwrap();
        let content = "x = 1\ny = x + 2";
        file.create_file().unwrap().write_all(content.as_bytes()).unwrap();

        let provider = RuffSemanticProvider::file_scope_with_fs(fs_root);
        assert_eq!(provider.mode(), ProviderMode::FileScope);

        // Use a virtual path for the file
        let file_path = PathBuf::from("test.py");

        // Process the file
        let result = provider.notify_file_processed(&file_path, content);
        assert!(result.is_ok());

        // Get definition using the _with_content method (bypasses read_file)
        let def_result = provider.get_definition_with_content(
            &file_path,
            content,
            ByteRange::new(0, 1), // position of 'x'
            language_core::DefinitionOptions::default(),
        );
        assert!(def_result.is_ok());
        // Note: ty_ide may or may not find the definition depending on its internal state
        // The main thing is that it doesn't crash with MemoryFS

        // Find references using the _with_content method
        let refs_result = provider.find_references_with_content(&file_path, content, ByteRange::new(0, 1));
        assert!(refs_result.is_ok());
    }

    #[test]
    fn test_workspace_scope_with_memory_fs() {
        use std::io::Write;
        use std::path::PathBuf;

        let fs_root = filesystem::memory_fs();

        // Create files in the memory filesystem
        let utils_content = "def add(a, b):\n    return a + b";
        fs_root
            .join("utils.py")
            .unwrap()
            .create_file()
            .unwrap()
            .write_all(utils_content.as_bytes())
            .unwrap();

        let main_content = "from utils import add\nresult = add(1, 2)";
        fs_root
            .join("main.py")
            .unwrap()
            .create_file()
            .unwrap()
            .write_all(main_content.as_bytes())
            .unwrap();

        let workspace_root = PathBuf::from("/virtual/workspace");
        let provider = RuffSemanticProvider::workspace_scope_with_fs(workspace_root, fs_root);
        assert_eq!(provider.mode(), ProviderMode::WorkspaceScope);

        // Process the utils file
        let utils_path = PathBuf::from("utils.py");
        let result = provider.notify_file_processed(&utils_path, utils_content);
        assert!(result.is_ok());

        // Process the main file
        let main_path = PathBuf::from("main.py");
        let result = provider.notify_file_processed(&main_path, main_content);
        assert!(result.is_ok());

        // Get definition of 'add' in utils.py
        let def_result = provider.get_definition_with_content(
            &utils_path,
            utils_content,
            ByteRange::new(4, 7), // position of 'add' in "def add"
            language_core::DefinitionOptions::default(),
        );
        assert!(def_result.is_ok());
    }

    #[test]
    fn test_memory_fs_read_file() {
        use std::io::Write;
        use std::path::PathBuf;

        let fs_root = filesystem::memory_fs();

        // Create a file in the memory filesystem
        let content = "hello = 'world'";
        fs_root
            .join("test.py")
            .unwrap()
            .create_file()
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();

        let provider = RuffSemanticProvider::file_scope_with_fs(fs_root);

        // The read_file method should work with virtual paths
        let file_path = PathBuf::from("test.py");
        let read_result = provider.read_file(&file_path);
        assert!(read_result.is_ok());
        assert_eq!(read_result.unwrap(), content);
    }

    #[test]
    fn test_memory_fs_nested_paths() {
        use std::io::Write;
        use std::path::PathBuf;

        let fs_root = filesystem::memory_fs();

        // Create nested directory structure
        fs_root.join("src").unwrap().create_dir().unwrap();
        fs_root.join("src/models").unwrap().create_dir().unwrap();

        let content = "class User:\n    pass";
        fs_root
            .join("src/models/user.py")
            .unwrap()
            .create_file()
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();

        let provider = RuffSemanticProvider::file_scope_with_fs(fs_root);

        // Read the nested file
        let file_path = PathBuf::from("src/models/user.py");
        let read_result = provider.read_file(&file_path);
        assert!(read_result.is_ok());
        assert_eq!(read_result.unwrap(), content);
    }
}
