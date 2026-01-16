//! Registry for semantic normalizers and parser providers.

use once_cell::sync::Lazy;
use tree_sitter::Parser;

use super::go::GoNormalizer;
use super::javascript::JavaScriptNormalizer;
use super::json::JsonNormalizer;
use super::python::PythonNormalizer;
use super::rust_lang::RustNormalizer;
use super::traits::{ParserProvider, SemanticNormalizer};
use super::typescript::{TsxNormalizer, TypeScriptNormalizer};

/// Lazily initialized default normalizer registry.
///
/// This avoids creating new normalizers on every comparison.
static DEFAULT_NORMALIZER_REGISTRY: Lazy<NormalizerRegistry> =
    Lazy::new(NormalizerRegistry::with_defaults);

/// Lazily initialized default parser registry.
///
/// This includes all languages with parser support.
static DEFAULT_PARSER_REGISTRY: Lazy<ParserRegistry> = Lazy::new(ParserRegistry::with_defaults);

/// Registry of parser providers.
///
/// Provides parser lookup by language identifier or file extension.
/// This is the base registry that enables AST and CST comparison.
/// Languages with semantic rules also register their parsers here.
pub struct ParserRegistry {
    providers: Vec<Box<dyn ParserProvider>>,
}

impl ParserRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Get a reference to the lazily-initialized default registry.
    ///
    /// This is more efficient than `Default::default()` when called multiple times
    /// because it reuses the same registry instance.
    pub fn default_ref() -> &'static Self {
        &DEFAULT_PARSER_REGISTRY
    }

    /// Create a registry with all built-in parser providers.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        // Register all languages that have parser support
        // These are the same as the normalizers since all normalizers provide parsers
        registry.register(Box::new(JavaScriptNormalizer));
        registry.register(Box::new(TypeScriptNormalizer));
        registry.register(Box::new(TsxNormalizer));
        registry.register(Box::new(PythonNormalizer));
        registry.register(Box::new(GoNormalizer));
        registry.register(Box::new(RustNormalizer));
        registry.register(Box::new(JsonNormalizer));
        registry
    }

    /// Register a parser provider.
    pub fn register(&mut self, provider: Box<dyn ParserProvider>) {
        self.providers.push(provider);
    }

    /// Get a parser provider by language identifier.
    ///
    /// Returns the first provider that handles the given language.
    pub fn get(&self, language: &str) -> Option<&dyn ParserProvider> {
        self.providers
            .iter()
            .find(|p| p.handles_language(language))
            .map(|p| p.as_ref())
    }

    /// Get a parser provider by file extension.
    ///
    /// The extension should include the leading dot (e.g., ".js", ".py").
    pub fn get_by_extension(&self, extension: &str) -> Option<&dyn ParserProvider> {
        self.providers
            .iter()
            .find(|p| p.handles_extension(extension))
            .map(|p| p.as_ref())
    }

    /// Get a parser for the given language.
    ///
    /// Returns `None` if no provider handles the language or parser creation fails.
    pub fn get_parser(&self, language: &str) -> Option<Parser> {
        self.get(language).and_then(|p| p.get_parser())
    }

    /// Check if any provider handles the given language.
    pub fn supports_language(&self, language: &str) -> bool {
        self.get(language).is_some()
    }

    /// Check if any provider handles the given file extension.
    pub fn supports_extension(&self, extension: &str) -> bool {
        self.get_by_extension(extension).is_some()
    }

    /// Get the number of registered providers.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Registry of semantic normalizers.
///
/// Provides lookup by language identifier or file extension.
/// Can be constructed with default normalizers or customized for testing.
pub struct NormalizerRegistry {
    normalizers: Vec<Box<dyn SemanticNormalizer>>,
}

impl NormalizerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            normalizers: Vec::new(),
        }
    }

    /// Get a reference to the lazily-initialized default registry.
    ///
    /// This is more efficient than `Default::default()` when called multiple times
    /// because it reuses the same registry instance.
    pub fn default_ref() -> &'static Self {
        &DEFAULT_NORMALIZER_REGISTRY
    }

    /// Create a registry with all built-in normalizers.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(JavaScriptNormalizer));
        registry.register(Box::new(TypeScriptNormalizer));
        registry.register(Box::new(TsxNormalizer));
        registry.register(Box::new(PythonNormalizer));
        registry.register(Box::new(GoNormalizer));
        registry.register(Box::new(RustNormalizer));
        registry.register(Box::new(JsonNormalizer));
        registry
    }

    /// Register a normalizer.
    pub fn register(&mut self, normalizer: Box<dyn SemanticNormalizer>) {
        self.normalizers.push(normalizer);
    }

    /// Get a normalizer by language identifier.
    ///
    /// Returns the first normalizer that handles the given language.
    pub fn get(&self, language: &str) -> Option<&dyn SemanticNormalizer> {
        self.normalizers
            .iter()
            .find(|n| n.handles_language(language))
            .map(|n| n.as_ref())
    }

    /// Get a normalizer by file extension.
    ///
    /// The extension should include the leading dot (e.g., ".js", ".py").
    pub fn get_by_extension(&self, extension: &str) -> Option<&dyn SemanticNormalizer> {
        self.normalizers
            .iter()
            .find(|n| n.handles_extension(extension))
            .map(|n| n.as_ref())
    }

    /// Check if any normalizer handles the given language.
    pub fn supports_language(&self, language: &str) -> bool {
        self.get(language).is_some()
    }

    /// Check if any normalizer handles the given file extension.
    pub fn supports_extension(&self, extension: &str) -> bool {
        self.get_by_extension(extension).is_some()
    }

    /// Get the number of registered normalizers.
    pub fn len(&self) -> usize {
        self.normalizers.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.normalizers.is_empty()
    }
}

impl Default for NormalizerRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_defaults_has_all_normalizers() {
        let registry = NormalizerRegistry::with_defaults();
        assert_eq!(registry.len(), 7); // JS, TS, TSX, Python, Go, Rust, JSON
    }

    #[test]
    fn test_get_by_language() {
        let registry = NormalizerRegistry::default();

        assert!(registry.get("javascript").is_some());
        assert!(registry.get("js").is_some());
        assert!(registry.get("typescript").is_some());
        assert!(registry.get("tsx").is_some());
        assert!(registry.get("python").is_some());
        assert!(registry.get("go").is_some());
        assert!(registry.get("golang").is_some());
        assert!(registry.get("rust").is_some());
        assert!(registry.get("json").is_some());
        assert!(registry.get("unknown").is_none());
        assert!(registry.get("css").is_none());
    }

    #[test]
    fn test_get_by_extension() {
        let registry = NormalizerRegistry::default();

        assert!(registry.get_by_extension(".js").is_some());
        assert!(registry.get_by_extension(".jsx").is_some());
        assert!(registry.get_by_extension(".ts").is_some());
        assert!(registry.get_by_extension(".tsx").is_some());
        assert!(registry.get_by_extension(".py").is_some());
        assert!(registry.get_by_extension(".go").is_some());
        assert!(registry.get_by_extension(".rs").is_some());
        assert!(registry.get_by_extension(".json").is_some());

        assert!(registry.get_by_extension(".css").is_none());
        assert!(registry.get_by_extension(".html").is_none());
    }

    #[test]
    fn test_case_insensitive_lookup() {
        let registry = NormalizerRegistry::default();

        assert!(registry.get("JavaScript").is_some());
        assert!(registry.get("PYTHON").is_some());
        assert!(registry.get_by_extension(".JS").is_some());
        assert!(registry.get_by_extension(".PY").is_some());
    }

    #[test]
    fn test_custom_registry() {
        let mut registry = NormalizerRegistry::new();
        assert!(registry.is_empty());

        registry.register(Box::new(PythonNormalizer));
        assert_eq!(registry.len(), 1);
        assert!(registry.get("python").is_some());
        assert!(registry.get("javascript").is_none());
    }

    #[test]
    fn test_supports_language() {
        let registry = NormalizerRegistry::default();
        assert!(registry.supports_language("javascript"));
        assert!(!registry.supports_language("css"));
    }

    #[test]
    fn test_supports_extension() {
        let registry = NormalizerRegistry::default();
        assert!(registry.supports_extension(".js"));
        assert!(!registry.supports_extension(".css"));
    }

    // ParserRegistry tests
    #[test]
    fn test_parser_registry_with_defaults() {
        let registry = ParserRegistry::with_defaults();
        assert_eq!(registry.len(), 7); // JS, TS, TSX, Python, Go, Rust, JSON
    }

    #[test]
    fn test_parser_registry_get_by_language() {
        let registry = ParserRegistry::default();

        assert!(registry.get("javascript").is_some());
        assert!(registry.get("js").is_some());
        assert!(registry.get("typescript").is_some());
        assert!(registry.get("tsx").is_some());
        assert!(registry.get("python").is_some());
        assert!(registry.get("go").is_some());
        assert!(registry.get("rust").is_some());
        assert!(registry.get("json").is_some());
        assert!(registry.get("unknown").is_none());
    }

    #[test]
    fn test_parser_registry_get_parser() {
        let registry = ParserRegistry::default();

        assert!(registry.get_parser("javascript").is_some());
        assert!(registry.get_parser("python").is_some());
        assert!(registry.get_parser("unknown").is_none());
    }

    #[test]
    fn test_parser_registry_supports_language() {
        let registry = ParserRegistry::default();
        assert!(registry.supports_language("javascript"));
        assert!(!registry.supports_language("css"));
    }
}
