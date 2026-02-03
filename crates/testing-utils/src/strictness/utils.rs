//! Shared utilities for semantic normalization.

use super::traits::{NormalizedNode, SemanticNormalizer};
use tree_sitter::Node;

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
    children.iter().any(|c| {
        matches!(
            c.kind.as_str(),
            "spread_element" | "rest_pattern" | "dictionary_splat"
        )
    })
}

/// Check if children contain JSX spread attributes.
pub fn has_jsx_spread(children: &[NormalizedNode]) -> bool {
    children.iter().any(|c| {
        c.kind == "jsx_expression"
            && c.children
                .iter()
                .any(|inner| inner.kind == "spread_element")
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

/// Check if text is meaningful content (not just punctuation).
///
/// Returns true if:
/// - The text has more than one character (e.g., identifiers, strings), OR
/// - The single character is alphanumeric (e.g., variable 'x', digit '1')
///
/// This filters out single-character punctuation like ';', ',', '(' that
/// shouldn't be used as sort keys when searching for meaningful text content.
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
///
/// Takes ownership of `children` to avoid cloning during flattening.
pub fn flatten_and_sort<F>(
    children: Vec<NormalizedNode>,
    kind: &str,
    key_fn: F,
) -> Vec<NormalizedNode>
where
    F: Fn(&NormalizedNode) -> String,
{
    let mut flattened = Vec::new();
    flatten_recursive_owned(children, kind, &mut flattened);
    flattened.sort_by_key(key_fn);
    flattened
}

fn flatten_recursive_owned(
    children: Vec<NormalizedNode>,
    kind: &str,
    result: &mut Vec<NormalizedNode>,
) {
    for child in children {
        if child.kind == kind {
            flatten_recursive_owned(child.children, kind, result);
        } else {
            result.push(child);
        }
    }
}

/// Normalize a tree-sitter node into a NormalizedNode with loose comparison.
///
/// This function recursively normalizes a tree-sitter AST node, applying
/// semantic normalization rules (like sorting unordered children) using
/// the provided normalizer. This is the "loose" comparison mode.
pub fn normalize_node<'a>(
    node: Node<'a>,
    source: &'a [u8],
    normalizer: &dyn SemanticNormalizer,
) -> NormalizedNode {
    let kind = node.kind().to_string();

    if node.named_child_count() == 0 {
        let text = node.utf8_text(source).unwrap_or("").to_string();
        let normalized_text = if normalizer.is_indentation_sensitive() {
            text
        } else {
            normalize_indentation(&text)
        };
        return NormalizedNode::leaf(kind, normalized_text);
    }

    let children: Vec<NormalizedNode> = node
        .named_children(&mut node.walk())
        .map(|child| normalize_node(child, source, normalizer))
        .collect();

    // Let the normalizer handle it first (no clone needed - takes ownership)
    let (mut children, handled) = normalizer.normalize_children(&kind, children);

    // Apply comment normalization if this is a comment scope and normalizer didn't handle it
    if !handled && normalizer.comment_scope_kinds().contains(&kind.as_str()) {
        children = normalize_comments_in_children(children);
    } else if !handled && normalizer.unordered_node_types().contains(node.kind()) {
        // Apply default sorting only if normalizer didn't handle it and node type is unordered
        children.sort_by_key(extract_sort_key);
    }

    NormalizedNode::new(kind, children)
}

/// Normalize a tree-sitter node for AST comparison without sorting.
///
/// This function creates a normalized representation but preserves child ordering.
/// Used for "ast" strictness level where we ignore whitespace/formatting but
/// preserve semantic ordering.
pub fn normalize_node_for_ast<'a>(node: Node<'a>, source: &'a [u8]) -> NormalizedNode {
    let kind = node.kind().to_string();

    if node.named_child_count() == 0 {
        return NormalizedNode::leaf(kind, node.utf8_text(source).unwrap_or("").to_string());
    }

    let children: Vec<NormalizedNode> = node
        .named_children(&mut node.walk())
        .map(|child| normalize_node_for_ast(child, source))
        .collect();

    NormalizedNode::new(kind, children)
}

/// Build a CST node including all tokens (named and unnamed).
///
/// This function builds a tree that includes all tokens including whitespace
/// and punctuation. Used for "cst" strictness level where exact token
/// structure matters.
pub fn build_cst_node<'a>(node: Node<'a>, source: &'a [u8]) -> NormalizedNode {
    let kind = node.kind().to_string();

    if node.child_count() == 0 {
        return NormalizedNode::leaf(kind, node.utf8_text(source).unwrap_or("").to_string());
    }

    // Include ALL children (named and unnamed) for CST comparison
    let mut cursor = node.walk();
    let children: Vec<NormalizedNode> = node
        .children(&mut cursor)
        .map(|child| build_cst_node(child, source))
        .collect();

    NormalizedNode::new(kind, children)
}

/// Comment node kinds across different languages.
const COMMENT_KINDS: &[&str] = &["comment", "line_comment", "block_comment", "hash_comment"];

/// Normalize indentation by stripping leading whitespace from each line.
///
/// Used for non-indentation-sensitive languages (JavaScript, TypeScript, Go, etc.)
/// where indentation is purely cosmetic and shouldn't affect semantic comparison.
pub fn normalize_indentation(text: &str) -> String {
    text.lines()
        .map(|line| line.trim_start())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check if a node kind represents a comment.
pub fn is_comment_kind(kind: &str) -> bool {
    COMMENT_KINDS.contains(&kind)
}

/// Sort comments by their text content for consistent comparison.
fn sort_comments_by_text(comments: &mut [NormalizedNode]) {
    comments.sort_by(|a, b| {
        let a_text = a.text.as_deref().unwrap_or("");
        let b_text = b.text.as_deref().unwrap_or("");
        a_text.cmp(b_text)
    });
}

/// Normalize children by sorting consecutive runs of comments.
///
/// This allows comment content to be verified while making the order of
/// adjacent comments irrelevant. Comments that are separated by non-comment
/// nodes maintain their relative position to those nodes.
///
/// For example:
/// ```text
/// // B
/// // A
/// function foo() {}
/// // D
/// // C
/// ```
/// Becomes:
/// ```text
/// // A  (first run sorted)
/// // B
/// function foo() {}
/// // C  (second run sorted)
/// // D
/// ```
///
/// This preserves the semantic meaning of comments relative to code while
/// allowing flexibility in the order of adjacent comments.
pub fn normalize_comments_in_children(children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    let mut result = Vec::with_capacity(children.len());
    let mut comment_run: Vec<NormalizedNode> = Vec::new();

    for child in children {
        if is_comment_kind(&child.kind) {
            comment_run.push(child);
        } else {
            // Flush any accumulated comments (sorted) before adding non-comment
            if !comment_run.is_empty() {
                sort_comments_by_text(&mut comment_run);
                result.append(&mut comment_run);
            }
            result.push(child);
        }
    }

    // Flush any trailing comments
    if !comment_run.is_empty() {
        sort_comments_by_text(&mut comment_run);
        result.append(&mut comment_run);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_indentation_single_line() {
        assert_eq!(normalize_indentation("    const x = 1;"), "const x = 1;");
    }

    #[test]
    fn test_normalize_indentation_multi_line() {
        let input = "    const x = 1;\n    const y = 2;";
        let expected = "const x = 1;\nconst y = 2;";
        assert_eq!(normalize_indentation(input), expected);
    }

    #[test]
    fn test_normalize_indentation_mixed_indentation() {
        let input = "  line1\n    line2\n\tline3";
        let expected = "line1\nline2\nline3";
        assert_eq!(normalize_indentation(input), expected);
    }

    #[test]
    fn test_normalize_indentation_empty_lines() {
        let input = "    code\n\n    more";
        let expected = "code\n\nmore";
        assert_eq!(normalize_indentation(input), expected);
    }

    #[test]
    fn test_normalize_indentation_no_indentation() {
        let input = "no indent";
        assert_eq!(normalize_indentation(input), "no indent");
    }
}
