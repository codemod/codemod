//! Lazy-initialized semantic provider.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use language_core::{
    ByteRange, DefinitionOptions, DefinitionResult, ProviderMode, ReferencesResult,
    SemanticProvider, SemanticResult,
};

use crate::config::SemanticConfig;
use crate::factory::SemanticFactory;

/// A lazy-initialized semantic provider that creates the underlying
/// provider on first use.
///
/// This provider defers the creation of the actual semantic provider
/// until the first semantic operation is requested. This is useful for:
///
/// - Reducing startup time when semantic analysis may not be needed
/// - Automatically detecting the language from the first file processed
/// - Providing a simple default behavior (FileScope) without configuration
///
/// # Example
///
/// ```
/// use semantic_factory::{LazySemanticProvider, SemanticConfig};
/// use language_core::SemanticProvider;
/// use std::path::Path;
///
/// // Create a lazy provider with default (FileScope) configuration
/// let provider = LazySemanticProvider::new(SemanticConfig::default());
///
/// // The actual provider is created on first use
/// // provider.get_definition(Path::new("test.ts"), ByteRange::new(0, 5));
/// ```
pub struct LazySemanticProvider {
    inner: OnceLock<Option<Arc<dyn SemanticProvider>>>,
    config: SemanticConfig,
}

impl LazySemanticProvider {
    /// Create a new lazy semantic provider with the given configuration.
    pub fn new(config: SemanticConfig) -> Self {
        Self {
            inner: OnceLock::new(),
            config,
        }
    }

    /// Create a lazy provider with default FileScope configuration.
    pub fn file_scope() -> Self {
        Self::new(SemanticConfig::file_scope())
    }

    /// Create a lazy provider with WorkspaceScope configuration.
    pub fn workspace_scope(root: std::path::PathBuf) -> Self {
        Self::new(SemanticConfig::workspace_scope(root))
    }

    /// Get or create the underlying provider for the given language.
    fn get_or_init(&self, language: &str) -> Option<&Arc<dyn SemanticProvider>> {
        self.inner
            .get_or_init(|| SemanticFactory::create(language, self.config.clone()))
            .as_ref()
    }

    /// Detect language from file extension.
    fn detect_language(file_path: &Path) -> &'static str {
        match file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_lowercase())
            .as_deref()
        {
            // TODO: probably a duplicate code? should we consolidate all the language detection code into one place?
            Some("js" | "mjs" | "cjs") => "javascript",
            Some("ts" | "mts" | "cts") => "typescript",
            Some("jsx") => "jsx",
            Some("tsx") => "tsx",
            Some("py" | "pyi") => "python",
            Some("css") => "css",
            Some("html" | "htm") => "html",
            Some("json") => "json",
            Some("yaml" | "yml") => "yaml",
            Some("md" | "markdown") => "markdown",
            _ => "unknown",
        }
    }
}

impl std::fmt::Debug for LazySemanticProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazySemanticProvider")
            .field("initialized", &self.inner.get().is_some())
            .field("config", &self.config)
            .finish()
    }
}

impl SemanticProvider for LazySemanticProvider {
    fn get_definition(
        &self,
        file_path: &Path,
        range: ByteRange,
        options: DefinitionOptions,
    ) -> SemanticResult<Option<DefinitionResult>> {
        let lang = Self::detect_language(file_path);
        match self.get_or_init(lang) {
            Some(provider) => provider.get_definition(file_path, range, options),
            None => Ok(None), // Unsupported language
        }
    }

    fn find_references(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        let lang = Self::detect_language(file_path);
        match self.get_or_init(lang) {
            Some(provider) => provider.find_references(file_path, range),
            None => Ok(ReferencesResult::new()), // Unsupported language
        }
    }

    fn get_type(&self, file_path: &Path, range: ByteRange) -> SemanticResult<Option<String>> {
        let lang = Self::detect_language(file_path);
        match self.get_or_init(lang) {
            Some(provider) => provider.get_type(file_path, range),
            None => Ok(None), // Unsupported language
        }
    }

    fn notify_file_processed(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        let lang = Self::detect_language(file_path);
        match self.get_or_init(lang) {
            Some(provider) => provider.notify_file_processed(file_path, content),
            None => Ok(()), // Unsupported language - no-op
        }
    }

    fn supports_language(&self, lang: &str) -> bool {
        SemanticFactory::supports_language(lang)
    }

    fn mode(&self) -> ProviderMode {
        // Return the mode based on config, even if not yet initialized
        match &self.config.scope {
            crate::config::SemanticScope::FileScope => ProviderMode::FileScope,
            crate::config::SemanticScope::WorkspaceScope { .. } => ProviderMode::WorkspaceScope,
        }
    }
}

// Implement Send + Sync for LazySemanticProvider
// OnceLock is already Send + Sync, and Arc<dyn SemanticProvider> is Send + Sync
unsafe impl Send for LazySemanticProvider {}
unsafe impl Sync for LazySemanticProvider {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_lazy_provider_not_initialized_until_use() {
        let provider = LazySemanticProvider::file_scope();
        assert!(provider.inner.get().is_none());
    }

    #[test]
    fn test_lazy_provider_initializes_on_first_use() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.ts");
        fs::write(&file_path, "const x = 1;").unwrap();

        let provider = LazySemanticProvider::file_scope();

        // Not initialized yet
        assert!(provider.inner.get().is_none());

        // Trigger initialization
        let _ = provider.get_definition(
            &file_path,
            ByteRange::new(6, 7),
            DefinitionOptions::default(),
        );

        // Now initialized
        assert!(provider.inner.get().is_some());
    }

    #[test]
    fn test_lazy_provider_returns_none_for_unsupported() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.css");
        fs::write(&file_path, ".body { color: red; }").unwrap();

        let provider = LazySemanticProvider::file_scope();
        let result = provider.get_definition(
            &file_path,
            ByteRange::new(0, 5),
            DefinitionOptions::default(),
        );

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.js")),
            "javascript"
        );
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.ts")),
            "typescript"
        );
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.jsx")),
            "jsx"
        );
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.tsx")),
            "tsx"
        );
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.py")),
            "python"
        );
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.pyi")),
            "python"
        );
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.css")),
            "css"
        );
        assert_eq!(
            LazySemanticProvider::detect_language(Path::new("test.unknown")),
            "unknown"
        );
    }

    #[test]
    fn test_lazy_provider_mode() {
        let file_provider = LazySemanticProvider::file_scope();
        assert_eq!(file_provider.mode(), ProviderMode::FileScope);

        let workspace_provider =
            LazySemanticProvider::workspace_scope(std::path::PathBuf::from("/tmp"));
        assert_eq!(workspace_provider.mode(), ProviderMode::WorkspaceScope);
    }

    #[test]
    fn test_lazy_provider_supports_language() {
        let provider = LazySemanticProvider::file_scope();
        assert!(provider.supports_language("javascript"));
        assert!(provider.supports_language("typescript"));
        assert!(provider.supports_language("python"));
        assert!(!provider.supports_language("css"));
    }

    #[test]
    fn test_lazy_provider_initializes_for_python() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");
        fs::write(&file_path, "x = 1").unwrap();

        let provider = LazySemanticProvider::file_scope();

        // Not initialized yet
        assert!(provider.inner.get().is_none());

        // Trigger initialization with Python file
        let _ = provider.get_definition(
            &file_path,
            ByteRange::new(0, 1),
            DefinitionOptions::default(),
        );

        // Now initialized
        assert!(provider.inner.get().is_some());
    }
}
