//! Go semantic normalizer.

use std::collections::HashSet;
use tree_sitter::Parser;

use super::traits::{NormalizedNode, ParserProvider, SemanticNormalizer};
use super::utils::{extract_sort_key, find_first_text};

const COMMENT_SCOPE_KINDS: &[&str] = &[
    "source_file",
    "block",
    "function_declaration",
    "method_declaration",
    "type_declaration",
    "const_declaration",
    "var_declaration",
    "if_statement",
    "for_statement",
    "switch_statement",
    "select_statement",
];

/// Semantic normalizer for Go.
pub struct GoNormalizer;

impl ParserProvider for GoNormalizer {
    fn language_ids(&self) -> &[&'static str] {
        &["go", "golang"]
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[".go"]
    }

    fn get_parser(&self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_go::LANGUAGE.into()).ok()?;
        Some(parser)
    }
}

impl SemanticNormalizer for GoNormalizer {
    fn unordered_node_types(&self) -> HashSet<&'static str> {
        // Note: literal_value is handled specially in normalize_children
        // because it can be either keyed (reorderable) or unkeyed (positional)
        ["interface_type", "type_elem"].into_iter().collect()
    }

    fn normalize_children(
        &self,
        node_kind: &str,
        children: Vec<NormalizedNode>,
    ) -> (Vec<NormalizedNode>, bool) {
        match node_kind {
            "import_spec_list" => (sort_by_path(children), true),
            "literal_value" => (normalize_literal_value(children), true),
            _ => (children, false),
        }
    }

    fn comment_scope_kinds(&self) -> &'static [&'static str] {
        COMMENT_SCOPE_KINDS
    }
}

/// Normalize Go literal_value (struct/map literals).
///
/// Only sorts if the literal contains keyed elements. Unkeyed (positional)
/// elements must maintain their order.
/// ```go
/// Point{x: 1, y: 2}  // keyed - can reorder
/// Point{1, 2, 3}     // unkeyed - order matters
/// ```
fn normalize_literal_value(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    // Check if any child is a keyed_element
    let has_keyed = children.iter().any(|c| c.kind == "keyed_element");
    if has_keyed {
        children.sort_by_key(extract_sort_key);
    }
    children
}

fn sort_by_path(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    children.sort_by_key(|n| find_first_text(n).unwrap_or_default());
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_ids() {
        let normalizer = GoNormalizer;
        assert!(normalizer.handles_language("go"));
        assert!(normalizer.handles_language("Go"));
        assert!(normalizer.handles_language("golang"));
        assert!(normalizer.handles_language("Golang"));
        assert!(!normalizer.handles_language("rust"));
    }

    #[test]
    fn test_file_extensions() {
        let normalizer = GoNormalizer;
        assert!(normalizer.handles_extension(".go"));
        assert!(!normalizer.handles_extension(".rs"));
    }

    #[test]
    fn test_get_parser() {
        assert!(GoNormalizer.get_parser().is_some());
    }

    #[test]
    fn test_unordered_types() {
        let types = GoNormalizer.unordered_node_types();
        // literal_value is handled specially in normalize_children
        assert!(!types.contains("literal_value"));
        assert!(types.contains("interface_type"));
        assert!(types.contains("type_elem"));
    }

    #[test]
    fn test_normalize_children_import_spec_list() {
        let children = vec![
            NormalizedNode::new(
                "import_spec".into(),
                vec![NormalizedNode::leaf(
                    "interpreted_string_literal_content".into(),
                    "os".into(),
                )],
            ),
            NormalizedNode::new(
                "import_spec".into(),
                vec![NormalizedNode::leaf(
                    "interpreted_string_literal_content".into(),
                    "fmt".into(),
                )],
            ),
        ];

        let (sorted, handled) = GoNormalizer.normalize_children("import_spec_list", children);
        assert!(handled);
        assert_eq!(sorted.len(), 2);
        assert_eq!(find_first_text(&sorted[0]), Some("fmt".to_string()));
        assert_eq!(find_first_text(&sorted[1]), Some("os".to_string()));
    }
}
