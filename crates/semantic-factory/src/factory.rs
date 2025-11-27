//! Factory for creating semantic providers.

use std::sync::Arc;

use language_core::SemanticProvider;
use language_javascript::OxcSemanticProvider;
use language_python::RuffSemanticProvider;

use crate::config::{SemanticConfig, SemanticScope};

/// Factory for creating semantic providers based on language and configuration.
pub struct SemanticFactory;

impl SemanticFactory {
    /// Create a semantic provider for the given language and configuration.
    ///
    /// Returns `None` for languages that don't support semantic analysis
    /// (e.g., CSS, HTML, JSON, YAML).
    ///
    /// # Arguments
    ///
    /// * `language` - The language identifier (e.g., "javascript", "typescript")
    /// * `config` - Configuration for the semantic provider
    ///
    /// # Example
    ///
    /// ```
    /// use semantic_factory::{SemanticFactory, SemanticConfig};
    /// use std::path::PathBuf;
    ///
    /// // Create a file-scope provider
    /// let provider = SemanticFactory::create("typescript", SemanticConfig::file_scope());
    ///
    /// // Create a workspace-scope provider
    /// let provider = SemanticFactory::create(
    ///     "javascript",
    ///     SemanticConfig::workspace_scope(PathBuf::from("/path/to/project"))
    /// );
    /// ```
    pub fn create(language: &str, config: SemanticConfig) -> Option<Arc<dyn SemanticProvider>> {
        match language.to_lowercase().as_str() {
            // JavaScript/TypeScript family
            "javascript" | "typescript" | "js" | "ts" | "jsx" | "tsx" | "mjs" | "cjs" => {
                Some(Arc::new(match config.scope {
                    SemanticScope::FileScope => OxcSemanticProvider::file_scope(),
                    SemanticScope::WorkspaceScope { root } => {
                        OxcSemanticProvider::workspace_scope(root)
                    }
                }))
            }
            // Python
            "python" | "py" => Some(Arc::new(match config.scope {
                SemanticScope::FileScope => RuffSemanticProvider::file_scope(),
                SemanticScope::WorkspaceScope { root } => {
                    RuffSemanticProvider::workspace_scope(root)
                }
            })),
            // Languages without semantic support
            "css" | "html" | "json" | "yaml" | "markdown" | "md" => None,
            // Unknown languages - no semantic support
            _ => {
                log::debug!("No semantic provider available for language: {}", language);
                None
            }
        }
    }

    /// Check if a language has semantic analysis support.
    pub fn supports_language(language: &str) -> bool {
        matches!(
            language.to_lowercase().as_str(),
            "javascript"
                | "typescript"
                | "js"
                | "ts"
                | "jsx"
                | "tsx"
                | "mjs"
                | "cjs"
                | "python"
                | "py"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use language_core::ProviderMode;

    #[test]
    fn test_create_file_scope_javascript() {
        let provider = SemanticFactory::create("javascript", SemanticConfig::file_scope());
        assert!(provider.is_some());
        let provider = provider.unwrap();
        assert_eq!(provider.mode(), ProviderMode::FileScope);
    }

    #[test]
    fn test_create_file_scope_typescript() {
        let provider = SemanticFactory::create("typescript", SemanticConfig::file_scope());
        assert!(provider.is_some());
        let provider = provider.unwrap();
        assert_eq!(provider.mode(), ProviderMode::FileScope);
    }

    #[test]
    fn test_create_workspace_scope() {
        let config = SemanticConfig::workspace_scope(std::path::PathBuf::from("/tmp"));
        let provider = SemanticFactory::create("typescript", config);
        assert!(provider.is_some());
        let provider = provider.unwrap();
        assert_eq!(provider.mode(), ProviderMode::WorkspaceScope);
    }

    #[test]
    fn test_create_unsupported_language() {
        let provider = SemanticFactory::create("css", SemanticConfig::file_scope());
        assert!(provider.is_none());
    }

    #[test]
    fn test_create_unknown_language() {
        let provider = SemanticFactory::create("unknown", SemanticConfig::file_scope());
        assert!(provider.is_none());
    }

    #[test]
    fn test_supports_language() {
        assert!(SemanticFactory::supports_language("javascript"));
        assert!(SemanticFactory::supports_language("typescript"));
        assert!(SemanticFactory::supports_language("jsx"));
        assert!(SemanticFactory::supports_language("tsx"));
        assert!(SemanticFactory::supports_language("python"));
        assert!(SemanticFactory::supports_language("py"));
        assert!(!SemanticFactory::supports_language("css"));
        assert!(!SemanticFactory::supports_language("html"));
        assert!(!SemanticFactory::supports_language("unknown"));
    }

    #[test]
    fn test_case_insensitive() {
        assert!(SemanticFactory::create("JavaScript", SemanticConfig::file_scope()).is_some());
        assert!(SemanticFactory::create("TYPESCRIPT", SemanticConfig::file_scope()).is_some());
        assert!(SemanticFactory::create("Python", SemanticConfig::file_scope()).is_some());
        assert!(SemanticFactory::create("PYTHON", SemanticConfig::file_scope()).is_some());
    }

    #[test]
    fn test_create_file_scope_python() {
        let provider = SemanticFactory::create("python", SemanticConfig::file_scope());
        assert!(provider.is_some());
        let provider = provider.unwrap();
        assert_eq!(provider.mode(), ProviderMode::FileScope);
    }

    #[test]
    fn test_create_workspace_scope_python() {
        let config = SemanticConfig::workspace_scope(std::path::PathBuf::from("/tmp"));
        let provider = SemanticFactory::create("python", config);
        assert!(provider.is_some());
        let provider = provider.unwrap();
        assert_eq!(provider.mode(), ProviderMode::WorkspaceScope);
    }
}
