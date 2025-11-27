//! No-op semantic provider for languages without symbol indexing support.

use crate::{
    ByteRange, DefinitionResult, ProviderMode, ReferencesResult, SemanticProvider, SemanticResult,
};
use std::path::Path;

/// A no-op semantic provider that returns empty results for all queries.
///
/// This is used for languages that don't require or support symbol indexing,
/// such as CSS, HTML, JSON, YAML, and Markdown.
#[derive(Debug, Clone, Default)]
pub struct NoopSemanticProvider {
    /// Languages that this provider claims to support (returns noop for them).
    supported_languages: Vec<String>,
}

impl NoopSemanticProvider {
    /// Create a new no-op provider.
    pub fn new() -> Self {
        Self {
            supported_languages: vec![
                "css".to_string(),
                "html".to_string(),
                "json".to_string(),
                "yaml".to_string(),
                "markdown".to_string(),
            ],
        }
    }

    /// Create a no-op provider with custom supported languages.
    pub fn with_languages(languages: Vec<String>) -> Self {
        Self {
            supported_languages: languages,
        }
    }
}

impl SemanticProvider for NoopSemanticProvider {
    fn get_definition(
        &self,
        _file_path: &Path,
        _range: ByteRange,
    ) -> SemanticResult<Option<DefinitionResult>> {
        // No-op: always returns None
        Ok(None)
    }

    fn find_references(
        &self,
        _file_path: &Path,
        _range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        // No-op: always returns empty result
        Ok(ReferencesResult::new())
    }

    fn get_type(&self, _file_path: &Path, _range: ByteRange) -> SemanticResult<Option<String>> {
        // No-op: type information not available
        Ok(None)
    }

    fn notify_file_processed(&self, _file_path: &Path, _content: &str) -> SemanticResult<()> {
        // No-op: nothing to index
        Ok(())
    }

    fn supports_language(&self, lang: &str) -> bool {
        self.supported_languages
            .iter()
            .any(|l| l.eq_ignore_ascii_case(lang))
    }

    fn mode(&self) -> ProviderMode {
        ProviderMode::Lightweight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_get_definition() {
        let provider = NoopSemanticProvider::new();
        let result = provider.get_definition(Path::new("test.css"), ByteRange::new(0, 10));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_noop_find_references() {
        let provider = NoopSemanticProvider::new();
        let result = provider.find_references(Path::new("test.css"), ByteRange::new(0, 10));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_noop_get_type() {
        let provider = NoopSemanticProvider::new();
        let result = provider.get_type(Path::new("test.css"), ByteRange::new(0, 10));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_noop_supports_language() {
        let provider = NoopSemanticProvider::new();
        assert!(provider.supports_language("css"));
        assert!(provider.supports_language("CSS"));
        assert!(provider.supports_language("html"));
        assert!(provider.supports_language("json"));
        assert!(provider.supports_language("yaml"));
        assert!(provider.supports_language("markdown"));
        assert!(!provider.supports_language("javascript"));
        assert!(!provider.supports_language("typescript"));
    }

    #[test]
    fn test_noop_custom_languages() {
        let provider = NoopSemanticProvider::with_languages(vec![
            "custom".to_string(),
            "lang".to_string(),
        ]);
        assert!(provider.supports_language("custom"));
        assert!(provider.supports_language("lang"));
        assert!(!provider.supports_language("css"));
    }

    #[test]
    fn test_noop_notify_file_processed() {
        let provider = NoopSemanticProvider::new();
        let result = provider.notify_file_processed(Path::new("test.css"), ".body { color: red; }");
        assert!(result.is_ok());
    }

    #[test]
    fn test_noop_mode() {
        let provider = NoopSemanticProvider::new();
        assert_eq!(provider.mode(), ProviderMode::Lightweight);
    }
}
