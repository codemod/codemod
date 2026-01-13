use std::collections::HashSet;
use tree_sitter::{Node, Parser};

fn get_unordered_node_types(language: &str) -> HashSet<&'static str> {
    match language.to_lowercase().as_str() {
        "javascript" | "js" | "jsx" => {
            ["object", "object_pattern"].into_iter().collect()
        }
        "typescript" | "ts" | "tsx" => {
            ["object", "object_pattern", "object_type"].into_iter().collect()
        }
        "python" | "py" => {
            ["dictionary", "set"].into_iter().collect()
        }
        "go" => {
            ["literal_value"].into_iter().collect()
        }
        "rust" => {
            ["struct_expression", "field_initializer_list"].into_iter().collect()
        }
        "json" => {
            ["object"].into_iter().collect()
        }
        _ => HashSet::new(),
    }
}

fn get_parser(language: &str) -> Option<Parser> {
    let mut parser = Parser::new();
    let lang = match language.to_lowercase().as_str() {
        "javascript" | "js" | "jsx" => tree_sitter_javascript::LANGUAGE,
        "typescript" | "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX,
        "python" | "py" => tree_sitter_python::LANGUAGE,
        "go" => tree_sitter_go::LANGUAGE,
        "rust" => tree_sitter_rust::LANGUAGE,
        _ => return None,
    };
    parser.set_language(&lang.into()).ok()?;
    Some(parser)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedNode {
    kind: String,
    text: Option<String>,
    children: Vec<NormalizedNode>,
}

fn normalize_node<'a>(
    node: Node<'a>,
    source: &'a [u8],
    unordered_types: &HashSet<&str>,
) -> NormalizedNode {
    let kind = node.kind().to_string();

    if node.named_child_count() == 0 {
        return NormalizedNode {
            kind,
            text: Some(node.utf8_text(source).unwrap_or("").to_string()),
            children: vec![],
        };
    }

    let mut children: Vec<NormalizedNode> = node
        .named_children(&mut node.walk())
        .map(|child| normalize_node(child, source, unordered_types))
        .collect();

    if unordered_types.contains(node.kind()) {
        children.sort_by(|a, b| {
            let key_a = extract_sort_key(a);
            let key_b = extract_sort_key(b);
            key_a.cmp(&key_b)
        });
    }

    NormalizedNode {
        kind,
        text: None,
        children,
    }
}

fn extract_sort_key(node: &NormalizedNode) -> String {
    if node.kind == "pair" || node.kind == "property" || node.kind == "shorthand_property" {
        if let Some(first_child) = node.children.first() {
            if let Some(text) = &first_child.text {
                return text.clone();
            }
        }
    }

    if let Some(text) = &node.text {
        return text.clone();
    }

    node.kind.clone()
}

fn trees_equal(tree1: &NormalizedNode, tree2: &NormalizedNode) -> bool {
    if tree1.kind != tree2.kind {
        return false;
    }

    if tree1.children.is_empty() && tree2.children.is_empty() {
        return tree1.text == tree2.text;
    }

    if tree1.children.len() != tree2.children.len() {
        return false;
    }

    for (c1, c2) in tree1.children.iter().zip(tree2.children.iter()) {
        if !trees_equal(c1, c2) {
            return false;
        }
    }

    true
}

pub fn semantic_compare(expected: &str, actual: &str, language: &str) -> bool {
    let Some(mut parser) = get_parser(language) else {
        return expected == actual;
    };

    let Some(expected_tree) = parser.parse(expected, None) else {
        return expected == actual;
    };

    let Some(actual_tree) = parser.parse(actual, None) else {
        return expected == actual;
    };

    let unordered_types = get_unordered_node_types(language);

    let expected_normalized = normalize_node(
        expected_tree.root_node(),
        expected.as_bytes(),
        &unordered_types,
    );

    let actual_normalized = normalize_node(
        actual_tree.root_node(),
        actual.as_bytes(),
        &unordered_types,
    );

    trees_equal(&expected_normalized, &actual_normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_property_order() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_nested_object_property_order() {
        let expected = r#"const obj = { a: { x: 1, y: 2 }, b: 3 };"#;
        let actual = r#"const obj = { b: 3, a: { y: 2, x: 1 } };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_different_values_fail() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { a: 1, b: 3 };"#;
        assert!(!semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_array_order_matters() {
        let expected = r#"const arr = [1, 2, 3];"#;
        let actual = r#"const arr = [3, 2, 1];"#;
        assert!(!semantic_compare(expected, actual, "javascript"));
    }
}
