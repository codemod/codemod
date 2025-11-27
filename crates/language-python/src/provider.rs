//! Main Ruff semantic provider implementation for Python.

use crate::analyzer::{FileScopeAnalyzer, WorkspaceScopeAnalyzer};
use language_core::{
    ByteRange, DefinitionResult, ProviderMode, ReferencesResult, SemanticProvider, SemanticResult,
};
use std::path::{Path, PathBuf};

/// Semantic analysis provider for Python using Ruff.
///
/// This provider supports two modes:
/// - **FileScope**: Single-file analysis with no cross-file resolution.
/// - **WorkspaceScope**: Workspace-wide analysis with cross-file support.
pub enum RuffSemanticProvider {
    /// FileScope mode analyzer (single-file analysis)
    FileScope(FileScopeAnalyzer),
    /// WorkspaceScope mode analyzer (workspace-wide analysis)
    WorkspaceScope(WorkspaceScopeAnalyzer),
}

impl RuffSemanticProvider {
    /// Create a file-scope provider for single-file analysis.
    ///
    /// This mode is best for:
    /// - Quick dry runs
    /// - High-level analysis
    /// - Single-file transformations
    /// - When cross-file references are not needed
    pub fn file_scope() -> Self {
        Self::FileScope(FileScopeAnalyzer::new())
    }

    /// Create a workspace-scope provider for workspace-wide analysis.
    ///
    /// This mode is best for:
    /// - Full codemod runs requiring cross-file references
    /// - Precise symbol resolution
    /// - When you need to find all usages of a symbol across the workspace
    pub fn workspace_scope(workspace_root: PathBuf) -> Self {
        Self::WorkspaceScope(WorkspaceScopeAnalyzer::new(workspace_root))
    }

    /// Clear all cached data.
    pub fn clear_cache(&self) {
        match self {
            Self::FileScope(analyzer) => analyzer.clear_cache(),
            Self::WorkspaceScope(analyzer) => analyzer.clear(),
        }
    }

    /// Get the number of cached files.
    pub fn cached_file_count(&self) -> usize {
        match self {
            Self::FileScope(analyzer) => analyzer.cache().len(),
            Self::WorkspaceScope(analyzer) => analyzer.cache().len(),
        }
    }
}

impl std::fmt::Debug for RuffSemanticProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileScope(_) => write!(f, "RuffSemanticProvider::FileScope"),
            Self::WorkspaceScope(_) => write!(f, "RuffSemanticProvider::WorkspaceScope"),
        }
    }
}

impl SemanticProvider for RuffSemanticProvider {
    fn get_definition(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<Option<DefinitionResult>> {
        // We need the file content for analysis
        let content = std::fs::read_to_string(file_path).map_err(|e| {
            language_core::SemanticError::FileRead {
                path: file_path.to_path_buf(),
                message: e.to_string(),
            }
        })?;

        match self {
            Self::FileScope(analyzer) => analyzer
                .get_definition(file_path, &content, range)
                .map_err(Into::into),
            Self::WorkspaceScope(analyzer) => analyzer
                .get_definition(file_path, &content, range)
                .map_err(Into::into),
        }
    }

    fn find_references(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        let content = std::fs::read_to_string(file_path).map_err(|e| {
            language_core::SemanticError::FileRead {
                path: file_path.to_path_buf(),
                message: e.to_string(),
            }
        })?;

        match self {
            Self::FileScope(analyzer) => analyzer
                .find_references(file_path, &content, range)
                .map_err(Into::into),
            Self::WorkspaceScope(analyzer) => analyzer
                .find_references(file_path, &content, range)
                .map_err(Into::into),
        }
    }

    fn get_type(&self, _file_path: &Path, _range: ByteRange) -> SemanticResult<Option<String>> {
        // Type inference would require additional integration with ruff's type checker
        // For now, return None (type info not available)
        Ok(None)
    }

    fn notify_file_processed(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        match self {
            Self::FileScope(analyzer) => analyzer
                .process_file(file_path, content)
                .map_err(Into::into),
            Self::WorkspaceScope(analyzer) => analyzer
                .process_file(file_path, content)
                .map_err(Into::into),
        }
    }

    fn supports_language(&self, lang: &str) -> bool {
        matches!(lang.to_lowercase().as_str(), "python" | "py")
    }

    fn mode(&self) -> ProviderMode {
        match self {
            Self::FileScope(_) => ProviderMode::FileScope,
            Self::WorkspaceScope(_) => ProviderMode::WorkspaceScope,
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
    ) -> SemanticResult<Option<DefinitionResult>> {
        match self {
            Self::FileScope(analyzer) => analyzer
                .get_definition(file_path, content, range)
                .map_err(Into::into),
            Self::WorkspaceScope(analyzer) => analyzer
                .get_definition(file_path, content, range)
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
        match self {
            Self::FileScope(analyzer) => analyzer
                .find_references(file_path, content, range)
                .map_err(Into::into),
            Self::WorkspaceScope(analyzer) => analyzer
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
        assert_eq!(provider.cached_file_count(), 1);
    }

    #[test]
    fn test_provider_clear_cache() {
        let provider = RuffSemanticProvider::file_scope();

        let content = "x = 1";
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");
        fs::write(&file_path, content).unwrap();

        provider.notify_file_processed(&file_path, content).unwrap();
        assert_eq!(provider.cached_file_count(), 1);

        provider.clear_cache();
        assert_eq!(provider.cached_file_count(), 0);
    }
}
