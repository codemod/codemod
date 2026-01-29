//! Python semantic normalizer.

use std::collections::HashSet;
use tree_sitter::Parser;

use super::traits::{NormalizedNode, ParserProvider, SemanticNormalizer};
use super::utils::{extract_sort_key, flatten_and_sort, has_spread_element};

const COMMENT_SCOPE_KINDS: &[&str] = &[
    "module",
    "block",
    "function_definition",
    "class_definition",
    "if_statement",
    "for_statement",
    "while_statement",
    "try_statement",
    "with_statement",
    "match_statement",
];

/// Semantic normalizer for Python.
///
/// Handles special cases like keyword argument reordering in function calls.
pub struct PythonNormalizer;

impl ParserProvider for PythonNormalizer {
    fn language_ids(&self) -> &[&'static str] {
        &["python", "py"]
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[".py", ".pyw", ".pyi"]
    }

    fn get_parser(&self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .ok()?;
        Some(parser)
    }
}

impl SemanticNormalizer for PythonNormalizer {
    fn unordered_node_types(&self) -> HashSet<&'static str> {
        ["set"].into_iter().collect()
    }

    fn normalize_children(
        &self,
        node_kind: &str,
        children: Vec<NormalizedNode>,
    ) -> (Vec<NormalizedNode>, bool) {
        match node_kind {
            "dictionary" => (normalize_dictionary(children), true),
            "argument_list" => (normalize_argument_list(children), true),
            "import_from_statement" => (normalize_imports(children), true),
            "import_statement" => (sort_by_first_child(children), true),
            "global_statement" | "nonlocal_statement" => (sort_by_text(children), true),
            "except_clause" => (normalize_except_clause(children), true),
            "type" => (normalize_type_annotation(children), true),
            _ => (children, false),
        }
    }

    fn comment_scope_kinds(&self) -> &'static [&'static str] {
        COMMENT_SCOPE_KINDS
    }
}

/// Normalize dictionary children, preserving dictionary_splat positions.
///
/// Splat position matters because it affects key override behavior:
/// ```python
/// {'a': 1, **x}  # x['a'] would override 'a': 1
/// {**x, 'a': 1}  # 'a': 1 would override x['a']
/// ```
fn normalize_dictionary(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    if has_spread_element(&children) {
        return children;
    }
    children.sort_by_key(extract_sort_key);
    children
}

fn normalize_argument_list(children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    let (positional, mut keyword): (Vec<_>, Vec<_>) = children
        .into_iter()
        .partition(|c| c.kind != "keyword_argument");

    keyword.sort_by_key(|n| n.children.first().and_then(|c| c.text.clone()));
    positional.into_iter().chain(keyword).collect()
}

fn sort_by_first_child(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    children.sort_by_key(|n| {
        n.children
            .first()
            .and_then(|c| c.text.clone())
            .unwrap_or_default()
    });
    children
}

fn sort_by_text(mut children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    children.sort_by_key(|n| n.text.clone().unwrap_or_default());
    children
}

/// Normalize Python import statements by sorting imported names.
///
/// Preserves: module path (first `dotted_name`), then sorts imported names.
/// Handles: `from x import a, b, c` -> sorts a, b, c
/// Preserves: `from x import *` (wildcard imports remain as-is)
fn normalize_imports(children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    let mut result = Vec::new();
    let mut names_to_sort = Vec::new();
    let mut module_path_found = false;

    for child in children {
        match child.kind.as_str() {
            "dotted_name" if !module_path_found => {
                result.push(child);
                module_path_found = true;
            }
            "dotted_name" | "aliased_import" => names_to_sort.push(child),
            "relative_import" => {
                result.push(child);
                module_path_found = true;
            }
            _ => result.push(child),
        }
    }

    names_to_sort.sort_by_key(|n| {
        n.children
            .first()
            .and_then(|c| c.text.clone())
            .unwrap_or_default()
    });
    result.extend(names_to_sort);
    result
}

fn normalize_except_clause(children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    children
        .into_iter()
        .map(|mut child| {
            if child.kind == "tuple" {
                child
                    .children
                    .sort_by_key(|n| n.text.clone().unwrap_or_else(|| n.kind.clone()));
            }
            child
        })
        .collect()
}

/// Normalize Python 3.10+ type union annotations.
///
/// Type unions like `int | str | None` are represented as nested binary_operators.
/// This function flattens and sorts them for semantic comparison.
fn normalize_type_annotation(children: Vec<NormalizedNode>) -> Vec<NormalizedNode> {
    children
        .into_iter()
        .map(|child| {
            if child.kind == "binary_operator" && child.children.len() >= 2 {
                flatten_union_type(child)
            } else {
                child
            }
        })
        .collect()
}

fn flatten_union_type(node: NormalizedNode) -> NormalizedNode {
    let flattened = flatten_and_sort(node.children, "binary_operator", |n| {
        n.text.clone().unwrap_or_else(|| n.kind.clone())
    });
    NormalizedNode::new("binary_operator".into(), flattened)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_ids() {
        let normalizer = PythonNormalizer;
        assert!(normalizer.handles_language("python"));
        assert!(normalizer.handles_language("py"));
        assert!(normalizer.handles_language("Python"));
        assert!(!normalizer.handles_language("javascript"));
    }

    #[test]
    fn test_file_extensions() {
        let normalizer = PythonNormalizer;
        assert!(normalizer.handles_extension(".py"));
        assert!(normalizer.handles_extension(".pyw"));
        assert!(normalizer.handles_extension(".pyi"));
        assert!(!normalizer.handles_extension(".js"));
    }

    #[test]
    fn test_get_parser() {
        assert!(PythonNormalizer.get_parser().is_some());
    }

    #[test]
    fn test_unordered_types() {
        let types = PythonNormalizer.unordered_node_types();
        assert!(types.contains("set"));
        assert!(!types.contains("dictionary"));
        assert!(!types.contains("list"));
    }

    #[test]
    fn test_normalize_argument_list_keyword_only() {
        let children = vec![
            NormalizedNode::new(
                "keyword_argument".into(),
                vec![NormalizedNode::leaf("identifier".into(), "b".into())],
            ),
            NormalizedNode::new(
                "keyword_argument".into(),
                vec![NormalizedNode::leaf("identifier".into(), "a".into())],
            ),
        ];
        let result = normalize_argument_list(children);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].children[0].text.as_deref(), Some("a"));
        assert_eq!(result[1].children[0].text.as_deref(), Some("b"));
    }

    #[test]
    fn test_normalize_argument_list_mixed() {
        let children = vec![
            NormalizedNode::leaf("integer".into(), "1".into()),
            NormalizedNode::new(
                "keyword_argument".into(),
                vec![NormalizedNode::leaf("identifier".into(), "z".into())],
            ),
            NormalizedNode::new(
                "keyword_argument".into(),
                vec![NormalizedNode::leaf("identifier".into(), "a".into())],
            ),
        ];
        let result = normalize_argument_list(children);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].kind, "integer");
        assert_eq!(result[1].children[0].text.as_deref(), Some("a"));
        assert_eq!(result[2].children[0].text.as_deref(), Some("z"));
    }

    #[test]
    fn test_normalize_dictionary_without_splat() {
        let children = vec![
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("string".into(), "'b'".into())],
            ),
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("string".into(), "'a'".into())],
            ),
        ];
        let result = normalize_dictionary(children);
        assert_eq!(result[0].children[0].text.as_deref(), Some("'a'"));
        assert_eq!(result[1].children[0].text.as_deref(), Some("'b'"));
    }

    #[test]
    fn test_normalize_dictionary_with_splat_unchanged() {
        let children = vec![
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("string".into(), "'b'".into())],
            ),
            NormalizedNode::new("dictionary_splat".into(), vec![]),
            NormalizedNode::new(
                "pair".into(),
                vec![NormalizedNode::leaf("string".into(), "'a'".into())],
            ),
        ];
        let result = normalize_dictionary(children);
        assert_eq!(result[0].children[0].text.as_deref(), Some("'b'"));
        assert_eq!(result[1].kind, "dictionary_splat");
        assert_eq!(result[2].children[0].text.as_deref(), Some("'a'"));
    }
}
