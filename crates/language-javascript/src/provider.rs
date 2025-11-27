//! Main OXC semantic provider implementation.

use crate::accurate::AccurateAnalyzer;
use crate::lightweight::LightweightAnalyzer;
use language_core::{
    ByteRange, DefinitionResult, ProviderMode, ReferencesResult, SemanticProvider, SemanticResult,
};
use std::path::{Path, PathBuf};

/// Semantic analysis provider for JavaScript and TypeScript using OXC.
///
/// This provider supports two modes:
/// - **FileScope**: Single-file analysis with no cross-file resolution. Fast startup,
///   only finds references within the same file.
/// - **WorkspaceScope**: Workspace-wide lazy indexing. Full cross-file support but
///   higher resource usage.
pub enum OxcSemanticProvider {
    /// FileScope mode analyzer (single-file analysis)
    FileScope(LightweightAnalyzer),
    /// WorkspaceScope mode analyzer (workspace-wide analysis)
    WorkspaceScope(AccurateAnalyzer),
}

impl OxcSemanticProvider {
    /// Create a file-scope provider for single-file analysis.
    ///
    /// This mode is best for:
    /// - Quick dry runs
    /// - High-level analysis
    /// - Single-file transformations
    /// - When cross-file references are not needed
    pub fn file_scope() -> Self {
        Self::FileScope(LightweightAnalyzer::new())
    }

    /// Create a workspace-scope provider for workspace-wide analysis.
    ///
    /// This mode is best for:
    /// - Full codemod runs requiring cross-file references
    /// - Precise symbol resolution
    /// - When you need to find all usages of a symbol across the workspace
    pub fn workspace_scope(workspace_root: PathBuf) -> Self {
        Self::WorkspaceScope(AccurateAnalyzer::new(workspace_root))
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

impl std::fmt::Debug for OxcSemanticProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileScope(_) => write!(f, "OxcSemanticProvider::FileScope"),
            Self::WorkspaceScope(_) => write!(f, "OxcSemanticProvider::WorkspaceScope"),
        }
    }
}

impl SemanticProvider for OxcSemanticProvider {
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
            Self::FileScope(analyzer) => analyzer.get_definition(file_path, &content, range),
            Self::WorkspaceScope(analyzer) => analyzer.get_definition(file_path, &content, range),
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
            Self::FileScope(analyzer) => analyzer.find_references(file_path, &content, range),
            Self::WorkspaceScope(analyzer) => analyzer.find_references(file_path, &content, range),
        }
    }

    fn get_type(&self, _file_path: &Path, _range: ByteRange) -> SemanticResult<Option<String>> {
        // Type inference is more complex and would require TypeScript's type checker
        // For now, return None (type info not available)
        // Future: could integrate with oxc's type inference or use TypeScript server
        Ok(None)
    }

    fn notify_file_processed(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        match self {
            Self::FileScope(analyzer) => analyzer.process_file(file_path, content),
            Self::WorkspaceScope(analyzer) => analyzer.process_file(file_path, content),
        }
    }

    fn supports_language(&self, lang: &str) -> bool {
        matches!(
            lang.to_lowercase().as_str(),
            "javascript" | "typescript" | "js" | "ts" | "jsx" | "tsx" | "mjs" | "cjs"
        )
    }

    fn mode(&self) -> ProviderMode {
        match self {
            Self::FileScope(_) => ProviderMode::FileScope,
            Self::WorkspaceScope(_) => ProviderMode::WorkspaceScope,
        }
    }
}

/// Helper to get definition with content provided (avoids re-reading file).
impl OxcSemanticProvider {
    /// Get definition with content already available.
    pub fn get_definition_with_content(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
    ) -> SemanticResult<Option<DefinitionResult>> {
        match self {
            Self::FileScope(analyzer) => analyzer.get_definition(file_path, content, range),
            Self::WorkspaceScope(analyzer) => analyzer.get_definition(file_path, content, range),
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
            Self::FileScope(analyzer) => analyzer.find_references(file_path, content, range),
            Self::WorkspaceScope(analyzer) => analyzer.find_references(file_path, content, range),
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
        let provider = OxcSemanticProvider::file_scope();
        assert!(provider.supports_language("javascript"));
        assert!(provider.supports_language("JavaScript"));
        assert!(provider.supports_language("typescript"));
        assert!(provider.supports_language("TypeScript"));
        assert!(provider.supports_language("js"));
        assert!(provider.supports_language("ts"));
        assert!(provider.supports_language("jsx"));
        assert!(provider.supports_language("tsx"));
        assert!(!provider.supports_language("css"));
        assert!(!provider.supports_language("python"));
    }

    #[test]
    fn test_file_scope_provider_mode() {
        let provider = OxcSemanticProvider::file_scope();
        assert_eq!(provider.mode(), ProviderMode::FileScope);
    }

    #[test]
    fn test_workspace_scope_provider_mode() {
        let dir = TempDir::new().unwrap();
        let provider = OxcSemanticProvider::workspace_scope(dir.path().to_path_buf());
        assert_eq!(provider.mode(), ProviderMode::WorkspaceScope);
    }

    #[test]
    fn test_provider_notify_file_processed() {
        let provider = OxcSemanticProvider::file_scope();

        let content = "const x = 1;";
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.ts");
        fs::write(&file_path, content).unwrap();

        let result = provider.notify_file_processed(&file_path, content);
        assert!(result.is_ok());
        assert_eq!(provider.cached_file_count(), 1);
    }

    #[test]
    fn test_provider_clear_cache() {
        let provider = OxcSemanticProvider::file_scope();

        let content = "const x = 1;";
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.ts");
        fs::write(&file_path, content).unwrap();

        provider.notify_file_processed(&file_path, content).unwrap();
        assert_eq!(provider.cached_file_count(), 1);

        provider.clear_cache();
        assert_eq!(provider.cached_file_count(), 0);
    }

    #[test]
    fn test_provider_get_definition() {
        let provider = OxcSemanticProvider::file_scope();

        let content = r#"const x = 1;
const y = x + 2;"#;
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.ts");
        fs::write(&file_path, content).unwrap();

        // First notify about the file
        provider.notify_file_processed(&file_path, content).unwrap();

        // Get definition at the position of 'x' in 'const x'
        let result =
            provider.get_definition_with_content(&file_path, content, ByteRange::new(6, 7));

        assert!(result.is_ok());
        let definition = result.unwrap();
        assert!(definition.is_some());
        let def = definition.unwrap();
        assert!(!def.content.is_empty());
    }

    #[test]
    fn test_provider_find_references() {
        let provider = OxcSemanticProvider::file_scope();

        let content = r#"const x = 1;
const y = x + 2;
console.log(x);"#;
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.ts");
        fs::write(&file_path, content).unwrap();

        // First notify about the file
        provider.notify_file_processed(&file_path, content).unwrap();

        // Find references to 'x'
        let result =
            provider.find_references_with_content(&file_path, content, ByteRange::new(6, 7));

        assert!(result.is_ok());
        let refs = result.unwrap();
        // Should find at least the definition
        assert!(!refs.is_empty());
        // Each file should have content
        for file in &refs.files {
            assert!(!file.content.is_empty());
        }
    }

    #[test]
    fn test_provider_get_type_returns_none() {
        let provider = OxcSemanticProvider::file_scope();
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.ts");
        fs::write(&file_path, "const x = 1;").unwrap();

        // Type info is not yet implemented
        let result = provider.get_type(&file_path, ByteRange::new(6, 7));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
