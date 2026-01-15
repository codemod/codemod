//! JSON semantic normalizer.

use std::collections::HashSet;
use tree_sitter::Parser;

use super::traits::SemanticNormalizer;

/// Semantic normalizer for JSON.
pub struct JsonNormalizer;

impl SemanticNormalizer for JsonNormalizer {
    fn language_ids(&self) -> &[&'static str] {
        &["json"]
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[".json", ".jsonc"]
    }

    fn get_parser(&self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_json::LANGUAGE.into())
            .ok()?;
        Some(parser)
    }

    fn unordered_node_types(&self) -> HashSet<&'static str> {
        ["object"].into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_ids() {
        let normalizer = JsonNormalizer;
        assert!(normalizer.handles_language("json"));
        assert!(normalizer.handles_language("JSON"));
        assert!(!normalizer.handles_language("yaml"));
    }

    #[test]
    fn test_file_extensions() {
        let normalizer = JsonNormalizer;
        assert!(normalizer.handles_extension(".json"));
        assert!(normalizer.handles_extension(".jsonc"));
        assert!(!normalizer.handles_extension(".yaml"));
    }

    #[test]
    fn test_get_parser() {
        let normalizer = JsonNormalizer;
        assert!(normalizer.get_parser().is_some());
    }

    #[test]
    fn test_unordered_types() {
        let normalizer = JsonNormalizer;
        let types = normalizer.unordered_node_types();
        assert!(types.contains("object"));
        assert!(!types.contains("array"));
    }
}
