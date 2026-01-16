//! Rust semantic normalizer.

use std::collections::HashSet;
use tree_sitter::Parser;

use super::traits::{NormalizedNode, ParserProvider, SemanticNormalizer};
use super::utils::{extract_sort_key, flatten_and_sort};

/// Semantic normalizer for Rust.
pub struct RustNormalizer;

impl ParserProvider for RustNormalizer {
    fn language_ids(&self) -> &[&'static str] {
        &["rust", "rs"]
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[".rs"]
    }

    fn get_parser(&self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .ok()?;
        Some(parser)
    }
}

impl SemanticNormalizer for RustNormalizer {
    fn unordered_node_types(&self) -> HashSet<&'static str> {
        [
            "struct_expression",
            "field_initializer_list",
            "trait_bounds",
            "where_clause",
            "use_list",
        ]
        .into_iter()
        .collect()
    }

    fn normalize_children(
        &self,
        node_kind: &str,
        children: Vec<NormalizedNode>,
    ) -> (Vec<NormalizedNode>, bool) {
        match node_kind {
            "attribute" => (normalize_derive_attribute(children), true),
            "or_pattern" => (
                flatten_and_sort(children, "or_pattern", extract_sort_key),
                true,
            ),
            _ => (children, false),
        }
    }
}

fn normalize_derive_attribute(children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    let is_derive = children
        .first()
        .map(|c| c.kind == "identifier" && c.text.as_deref() == Some("derive"))
        .unwrap_or(false);

    if !is_derive {
        return children;
    }

    children
        .into_iter()
        .map(|mut child| {
            if child.kind == "token_tree" {
                child
                    .children
                    .sort_by_key(|n| n.text.clone().unwrap_or_else(|| n.kind.clone()));
            }
            child
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_ids() {
        let normalizer = RustNormalizer;
        assert!(normalizer.handles_language("rust"));
        assert!(normalizer.handles_language("rs"));
        assert!(normalizer.handles_language("Rust"));
        assert!(!normalizer.handles_language("go"));
    }

    #[test]
    fn test_file_extensions() {
        let normalizer = RustNormalizer;
        assert!(normalizer.handles_extension(".rs"));
        assert!(!normalizer.handles_extension(".go"));
    }

    #[test]
    fn test_get_parser() {
        assert!(RustNormalizer.get_parser().is_some());
    }

    #[test]
    fn test_unordered_types() {
        let types = RustNormalizer.unordered_node_types();
        assert!(types.contains("struct_expression"));
        assert!(types.contains("field_initializer_list"));
        assert!(types.contains("trait_bounds"));
        assert!(types.contains("where_clause"));
        assert!(types.contains("use_list"));
    }
}
