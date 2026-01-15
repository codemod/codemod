//! Shared utilities for semantic normalization.

use super::traits::NormalizedNode;

/// Punctuation and delimiter kinds that should be skipped when searching for meaningful text.
const SKIP_KINDS: &[&str] = &[
    "string_start",
    "string_end",
    "\"",
    "'",
    "`",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ",",
    ":",
];

/// Node kinds that have a "key" as their first child (e.g., property: value).
/// Used for consistent sort key extraction across all normalizers.
pub const KEY_BEARING_KINDS: &[&str] = &[
    "pair",
    "property",
    "shorthand_property",
    "field_initializer",
    "keyed_element",
    "property_signature",
    "import_specifier",
    "export_specifier",
    "method_elem",
    "where_predicate",
    "method_signature",
    "method_definition",
    "type_elem",
];

/// Check if children contain a spread-like element that affects ordering semantics.
pub fn has_spread_element(children: &[NormalizedNode]) -> bool {
    children
        .iter()
        .any(|c| matches!(c.kind.as_str(), "spread_element" | "rest_pattern" | "dictionary_splat"))
}

/// Check if children contain JSX spread attributes.
pub fn has_jsx_spread(children: &[NormalizedNode]) -> bool {
    children.iter().any(|c| {
        c.kind == "jsx_expression" && c.children.iter().any(|inner| inner.kind == "spread_element")
    })
}

/// Recursively find the first meaningful text in a node tree.
///
/// Skips delimiter and punctuation nodes to find actual content.
pub fn find_first_text(node: &NormalizedNode) -> Option<String> {
    if SKIP_KINDS.contains(&node.kind.as_str()) {
        return None;
    }

    if let Some(text) = &node.text {
        if is_meaningful_text(text) {
            return Some(text.clone());
        }
    }

    node.children.iter().find_map(find_first_text)
}

fn is_meaningful_text(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| text.len() > 1 || c.is_alphanumeric())
}

/// Extract a sort key from a node, searching recursively if needed.
///
/// For KEY_BEARING_KINDS (like `pair`, `property`, etc.), extracts the key from
/// the first child. For other nodes, uses the node's text or searches recursively.
pub fn extract_sort_key(node: &NormalizedNode) -> String {
    // For key-bearing nodes, extract the key from the first child
    if KEY_BEARING_KINDS.contains(&node.kind.as_str()) {
        if let Some(key) = node
            .children
            .first()
            .and_then(|c| c.text.clone().or_else(|| find_first_text(c)))
        {
            return key;
        }
    }

    // For other nodes, use text or search recursively
    node.text
        .clone()
        .or_else(|| find_first_text(node))
        .unwrap_or_else(|| node.kind.clone())
}

/// Flatten nested nodes of a given kind and sort the results.
///
/// Used for associative operations like union types (`A | B | C`) where
/// the AST represents them as nested binary nodes.
pub fn flatten_and_sort<F>(children: Vec<NormalizedNode>, kind: &str, key_fn: F) -> Vec<NormalizedNode>
where
    F: Fn(&NormalizedNode) -> String,
{
    let mut flattened = Vec::new();
    flatten_recursive(&children, kind, &mut flattened);
    flattened.sort_by_key(key_fn);
    flattened
}

fn flatten_recursive(children: &[NormalizedNode], kind: &str, result: &mut Vec<NormalizedNode>) {
    for child in children {
        if child.kind == kind {
            flatten_recursive(&child.children, kind, result);
        } else {
            result.push(child.clone());
        }
    }
}
