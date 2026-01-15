//! JavaScript semantic normalizer.

use std::collections::HashSet;
use tree_sitter::Parser;

use super::traits::{NormalizedNode, SemanticNormalizer};
use super::utils::{extract_sort_key, has_spread_element};

/// Semantic normalizer for JavaScript (including JSX, MJS, CJS).
pub struct JavaScriptNormalizer;

impl SemanticNormalizer for JavaScriptNormalizer {
    fn language_ids(&self) -> &[&'static str] {
        &["javascript", "js", "jsx", "mjs", "cjs"]
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[".js", ".jsx", ".mjs", ".cjs"]
    }

    fn get_parser(&self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .ok()?;
        Some(parser)
    }

    fn unordered_node_types(&self) -> HashSet<&'static str> {
        ["named_imports", "export_clause"].into_iter().collect()
    }

    fn normalize_children(
        &self,
        node_kind: &str,
        children: Vec<NormalizedNode>,
    ) -> (Vec<NormalizedNode>, bool) {
        match node_kind {
            "object" | "object_pattern" => (normalize_object_children(children), true),
            _ => (children, false),
        }
    }
}

/// Normalize object children, preserving spread element positions.
///
/// Spread element position matters because it affects property override behavior:
/// ```javascript
/// { a: 1, ...x }  // x.a would override a
/// { ...x, a: 1 }  // a: 1 would override x.a
/// ```
fn normalize_object_children(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    if has_spread_element(&children) {
        return children;
    }
    children.sort_by_key(extract_sort_key);
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_ids() {
        let normalizer = JavaScriptNormalizer;
        assert!(normalizer.handles_language("javascript"));
        assert!(normalizer.handles_language("js"));
        assert!(normalizer.handles_language("jsx"));
        assert!(normalizer.handles_language("JavaScript"));
        assert!(!normalizer.handles_language("typescript"));
    }

    #[test]
    fn test_file_extensions() {
        let normalizer = JavaScriptNormalizer;
        assert!(normalizer.handles_extension(".js"));
        assert!(normalizer.handles_extension(".jsx"));
        assert!(normalizer.handles_extension(".mjs"));
        assert!(normalizer.handles_extension(".JS"));
        assert!(!normalizer.handles_extension(".ts"));
    }

    #[test]
    fn test_get_parser() {
        assert!(JavaScriptNormalizer.get_parser().is_some());
    }

    #[test]
    fn test_unordered_types() {
        let types = JavaScriptNormalizer.unordered_node_types();
        assert!(types.contains("named_imports"));
        assert!(types.contains("export_clause"));
        assert!(!types.contains("array"));
    }

    #[test]
    fn test_normalize_object_without_spread() {
        let children = vec![
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("property_identifier".into(), "b".into())],
            ),
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("property_identifier".into(), "a".into())],
            ),
        ];
        let result = normalize_object_children(children);
        assert_eq!(result[0].children[0].text.as_deref(), Some("a"));
        assert_eq!(result[1].children[0].text.as_deref(), Some("b"));
    }

    #[test]
    fn test_normalize_object_with_spread_unchanged() {
        let children = vec![
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("property_identifier".into(), "b".into())],
            ),
            NormalizedNode::new("spread_element".into(), vec![]),
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("property_identifier".into(), "a".into())],
            ),
        ];
        let result = normalize_object_children(children);
        assert_eq!(result[0].children[0].text.as_deref(), Some("b"));
        assert_eq!(result[1].kind, "spread_element");
        assert_eq!(result[2].children[0].text.as_deref(), Some("a"));
    }
}
