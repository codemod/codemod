//! TypeScript semantic normalizer.

use std::collections::HashSet;
use tree_sitter::Parser;

use super::traits::{NormalizedNode, ParserProvider, SemanticNormalizer};
use super::utils::{extract_sort_key, flatten_and_sort, has_jsx_spread, has_spread_element};

const TS_UNORDERED_NODE_TYPES: &[&str] = &[
    "object_type",
    "named_imports",
    "export_clause",
    "interface_body",
];

/// Node types where comment ordering should be normalized.
/// These are typically block/scope nodes where comments can appear between statements.
const COMMENT_SCOPE_KINDS: &[&str] = &[
    "program",
    "statement_block",
    "class_body",
    "switch_body",
    "object",
    "array",
    "enum_body",
    "module_block",
];

/// Semantic normalizer for TypeScript (non-TSX files).
pub struct TypeScriptNormalizer;

impl ParserProvider for TypeScriptNormalizer {
    fn language_ids(&self) -> &[&'static str] {
        &["typescript", "ts"]
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[".ts", ".mts", ".cts"]
    }

    fn get_parser(&self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .ok()?;
        Some(parser)
    }
}

impl SemanticNormalizer for TypeScriptNormalizer {
    fn unordered_node_types(&self) -> HashSet<&'static str> {
        TS_UNORDERED_NODE_TYPES.iter().copied().collect()
    }

    fn normalize_children(
        &self,
        node_kind: &str,
        children: Vec<NormalizedNode>,
    ) -> (Vec<NormalizedNode>, bool) {
        normalize_ts_children(node_kind, children)
    }

    fn comment_scope_kinds(&self) -> &'static [&'static str] {
        COMMENT_SCOPE_KINDS
    }
}

/// Semantic normalizer for TSX files.
pub struct TsxNormalizer;

impl ParserProvider for TsxNormalizer {
    fn language_ids(&self) -> &[&'static str] {
        &["tsx"]
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[".tsx"]
    }

    fn get_parser(&self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
            .ok()?;
        Some(parser)
    }
}

impl SemanticNormalizer for TsxNormalizer {
    fn unordered_node_types(&self) -> HashSet<&'static str> {
        TS_UNORDERED_NODE_TYPES.iter().copied().collect()
    }

    fn normalize_children(
        &self,
        node_kind: &str,
        children: Vec<NormalizedNode>,
    ) -> (Vec<NormalizedNode>, bool) {
        normalize_ts_children(node_kind, children)
    }

    fn comment_scope_kinds(&self) -> &'static [&'static str] {
        COMMENT_SCOPE_KINDS
    }
}

fn normalize_ts_children(
    node_kind: &str,
    children: Vec<NormalizedNode>,
) -> (Vec<NormalizedNode>, bool) {
    match node_kind {
        "object" | "object_pattern" => (normalize_object_children(children), true),
        "union_type" => (
            flatten_and_sort(children, "union_type", extract_sort_key),
            true,
        ),
        "intersection_type" => (
            flatten_and_sort(children, "intersection_type", extract_sort_key),
            true,
        ),
        "jsx_self_closing_element" | "jsx_opening_element" => {
            (normalize_jsx_element(children), true)
        }
        "implements_clause" | "extends_type_clause" => (sort_by_key(children), true),
        _ => (children, false),
    }
}

fn normalize_object_children(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    if has_spread_element(&children) {
        return children;
    }
    children.sort_by_key(extract_sort_key);
    children
}

/// Normalize JSX element children (attributes).
///
/// Only sorts jsx_attribute nodes if no spread attributes are present.
/// Spread attribute position matters because it affects prop override behavior.
fn normalize_jsx_element(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    if has_jsx_spread(&children) {
        return children;
    }
    children.sort_by_key(|n| {
        if n.kind == "jsx_attribute" {
            n.children
                .first()
                .and_then(|c| c.text.clone())
                .unwrap_or_default()
        } else {
            String::new()
        }
    });
    children
}

fn sort_by_key(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    children.sort_by_key(extract_sort_key);
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typescript_language_ids() {
        let normalizer = TypeScriptNormalizer;
        assert!(normalizer.handles_language("typescript"));
        assert!(normalizer.handles_language("ts"));
        assert!(normalizer.handles_language("TypeScript"));
        assert!(!normalizer.handles_language("tsx"));
    }

    #[test]
    fn test_typescript_file_extensions() {
        let normalizer = TypeScriptNormalizer;
        assert!(normalizer.handles_extension(".ts"));
        assert!(normalizer.handles_extension(".mts"));
        assert!(normalizer.handles_extension(".cts"));
        assert!(!normalizer.handles_extension(".tsx"));
    }

    #[test]
    fn test_tsx_language_ids() {
        let normalizer = TsxNormalizer;
        assert!(normalizer.handles_language("tsx"));
        assert!(!normalizer.handles_language("typescript"));
    }

    #[test]
    fn test_tsx_file_extensions() {
        let normalizer = TsxNormalizer;
        assert!(normalizer.handles_extension(".tsx"));
        assert!(!normalizer.handles_extension(".ts"));
    }

    #[test]
    fn test_get_parsers() {
        assert!(TypeScriptNormalizer.get_parser().is_some());
        assert!(TsxNormalizer.get_parser().is_some());
    }

    #[test]
    fn test_unordered_types_includes_object_type() {
        let types = TypeScriptNormalizer.unordered_node_types();
        assert!(types.contains("object_type"));
        assert!(types.contains("interface_body"));
        assert!(types.contains("named_imports"));
    }
}
