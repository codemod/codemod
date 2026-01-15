//! Semantic-aware diff generation for code comparison.
//!
//! Generates diffs that highlight actual semantic differences rather than
//! cosmetic differences like property ordering.

use super::registry::NormalizerRegistry;
use super::traits::{NormalizedNode, SemanticNormalizer};
use super::utils::extract_sort_key;
use tree_sitter::Node;

/// A semantic difference between two code snippets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticDiff {
    /// Human-readable description of differences.
    pub differences: Vec<DiffEntry>,
    /// Whether the comparison was semantic (true) or fell back to text (false).
    pub is_semantic: bool,
}

/// A single difference entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// Path to the differing node (e.g., "root > object > pair[0]").
    pub path: String,
    /// What was expected.
    pub expected: String,
    /// What was found.
    pub actual: String,
    /// The kind of difference.
    pub kind: DiffKind,
}

/// The kind of difference detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    /// Node kind mismatch.
    KindMismatch,
    /// Text content mismatch.
    TextMismatch,
    /// Different number of children.
    ChildCountMismatch,
    /// Missing child in actual.
    MissingChild,
    /// Extra child in actual.
    ExtraChild,
}

impl std::fmt::Display for SemanticDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.differences.is_empty() {
            return writeln!(f, "No differences found");
        }

        let mode = if self.is_semantic {
            "semantic"
        } else {
            "textual"
        };
        writeln!(
            f,
            "Found {} differences ({} comparison):",
            self.differences.len(),
            mode
        )?;
        writeln!(f)?;

        for (i, diff) in self.differences.iter().enumerate() {
            writeln!(f, "{}. {} at '{}'", i + 1, diff.kind, diff.path)?;
            writeln!(f, "   Expected: {}", diff.expected)?;
            writeln!(f, "   Actual:   {}", diff.actual)?;
            writeln!(f)?;
        }

        Ok(())
    }
}

impl std::fmt::Display for DiffKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffKind::KindMismatch => write!(f, "Node type mismatch"),
            DiffKind::TextMismatch => write!(f, "Value mismatch"),
            DiffKind::ChildCountMismatch => write!(f, "Child count mismatch"),
            DiffKind::MissingChild => write!(f, "Missing element"),
            DiffKind::ExtraChild => write!(f, "Extra element"),
        }
    }
}

/// Generate a semantic diff between two code strings.
///
/// Returns a diff highlighting actual semantic differences, ignoring
/// non-semantic variations like property ordering.
pub fn semantic_diff(expected: &str, actual: &str, language: &str) -> SemanticDiff {
    semantic_diff_with_registry(
        expected,
        actual,
        language,
        NormalizerRegistry::default_ref(),
    )
}

/// Generate a semantic diff using a custom registry.
pub fn semantic_diff_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> SemanticDiff {
    let Some(normalizer) = registry.get(language) else {
        return text_diff(expected, actual);
    };

    let Some(mut parser) = normalizer.get_parser() else {
        return text_diff(expected, actual);
    };

    let (Some(expected_tree), Some(actual_tree)) =
        (parser.parse(expected, None), parser.parse(actual, None))
    else {
        return text_diff(expected, actual);
    };

    let expected_normalized =
        normalize_node(expected_tree.root_node(), expected.as_bytes(), normalizer);
    let actual_normalized = normalize_node(actual_tree.root_node(), actual.as_bytes(), normalizer);

    let mut differences = Vec::new();
    compare_trees(
        &expected_normalized,
        &actual_normalized,
        "root",
        &mut differences,
    );

    SemanticDiff {
        differences,
        is_semantic: true,
    }
}

/// Fallback to text-based diff when semantic comparison isn't possible.
fn text_diff(expected: &str, actual: &str) -> SemanticDiff {
    let mut differences = Vec::new();

    if expected != actual {
        differences.push(DiffEntry {
            path: "content".to_string(),
            expected: truncate(expected, 100),
            actual: truncate(actual, 100),
            kind: DiffKind::TextMismatch,
        });
    }

    SemanticDiff {
        differences,
        is_semantic: false,
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

fn normalize_node<'a>(
    node: Node<'a>,
    source: &'a [u8],
    normalizer: &dyn SemanticNormalizer,
) -> NormalizedNode {
    let kind = node.kind().to_string();

    if node.named_child_count() == 0 {
        return NormalizedNode::leaf(kind, node.utf8_text(source).unwrap_or("").to_string());
    }

    let children: Vec<NormalizedNode> = node
        .named_children(&mut node.walk())
        .map(|child| normalize_node(child, source, normalizer))
        .collect();

    let (mut children, handled) = normalizer.normalize_children(&kind, children);

    if !handled && normalizer.unordered_node_types().contains(node.kind()) {
        children.sort_by_key(extract_sort_key);
    }

    NormalizedNode::new(kind, children)
}

fn compare_trees(
    expected: &NormalizedNode,
    actual: &NormalizedNode,
    path: &str,
    differences: &mut Vec<DiffEntry>,
) {
    // Check kind
    if expected.kind != actual.kind {
        differences.push(DiffEntry {
            path: path.to_string(),
            expected: expected.kind.clone(),
            actual: actual.kind.clone(),
            kind: DiffKind::KindMismatch,
        });
        return; // Can't compare children if kinds differ
    }

    // Check text content for leaf nodes
    if expected.children.is_empty() && actual.children.is_empty() {
        if expected.text != actual.text {
            differences.push(DiffEntry {
                path: path.to_string(),
                expected: expected.text.clone().unwrap_or_default(),
                actual: actual.text.clone().unwrap_or_default(),
                kind: DiffKind::TextMismatch,
            });
        }
        return;
    }

    // Check child count
    if expected.children.len() != actual.children.len() {
        differences.push(DiffEntry {
            path: path.to_string(),
            expected: format!("{} children", expected.children.len()),
            actual: format!("{} children", actual.children.len()),
            kind: DiffKind::ChildCountMismatch,
        });

        // Try to find specific missing/extra children
        find_missing_children(expected, actual, path, differences);
        return;
    }

    // Compare children recursively
    for (i, (exp_child, act_child)) in expected.children.iter().zip(&actual.children).enumerate() {
        let child_path = format!("{} > {}[{}]", path, exp_child.kind, i);
        compare_trees(exp_child, act_child, &child_path, differences);
    }
}

fn find_missing_children(
    expected: &NormalizedNode,
    actual: &NormalizedNode,
    path: &str,
    differences: &mut Vec<DiffEntry>,
) {
    // Find children in expected but not in actual
    for exp_child in &expected.children {
        let exp_key = extract_sort_key(exp_child);
        let found = actual
            .children
            .iter()
            .any(|a| extract_sort_key(a) == exp_key);

        if !found {
            differences.push(DiffEntry {
                path: format!("{} > {}", path, exp_child.kind),
                expected: format!("'{}' present", exp_key),
                actual: "missing".to_string(),
                kind: DiffKind::MissingChild,
            });
        }
    }

    // Find children in actual but not in expected
    for act_child in &actual.children {
        let act_key = extract_sort_key(act_child);
        let found = expected
            .children
            .iter()
            .any(|e| extract_sort_key(e) == act_key);

        if !found {
            differences.push(DiffEntry {
                path: format!("{} > {}", path, act_child.kind),
                expected: "not present".to_string(),
                actual: format!("'{}' found", act_key),
                kind: DiffKind::ExtraChild,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_diff_equal() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;
        let diff = semantic_diff(expected, actual, "javascript");
        assert!(diff.is_semantic);
        assert!(diff.differences.is_empty());
    }

    #[test]
    fn test_semantic_diff_value_mismatch() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { a: 1, b: 3 };"#;
        let diff = semantic_diff(expected, actual, "javascript");
        assert!(diff.is_semantic);
        assert!(!diff.differences.is_empty());
        assert!(diff
            .differences
            .iter()
            .any(|d| d.kind == DiffKind::TextMismatch));
    }

    #[test]
    fn test_semantic_diff_missing_property() {
        let expected = r#"const obj = { a: 1, b: 2, c: 3 };"#;
        let actual = r#"const obj = { a: 1, b: 2 };"#;
        let diff = semantic_diff(expected, actual, "javascript");
        assert!(diff.is_semantic);
        assert!(!diff.differences.is_empty());
    }

    #[test]
    fn test_semantic_diff_fallback_to_text() {
        let expected = "some code";
        let actual = "different code";
        let diff = semantic_diff(expected, actual, "unknown_language");
        assert!(!diff.is_semantic);
        assert!(!diff.differences.is_empty());
    }

    #[test]
    fn test_diff_display() {
        let diff = SemanticDiff {
            differences: vec![DiffEntry {
                path: "root > object".to_string(),
                expected: "1".to_string(),
                actual: "2".to_string(),
                kind: DiffKind::TextMismatch,
            }],
            is_semantic: true,
        };
        let output = diff.to_string();
        assert!(output.contains("Value mismatch"));
        assert!(output.contains("Expected: 1"));
        assert!(output.contains("Actual:   2"));
    }

    #[test]
    fn test_empty_diff_display() {
        let diff = SemanticDiff {
            differences: vec![],
            is_semantic: true,
        };
        let output = diff.to_string();
        assert!(output.contains("No differences found"));
    }
}
