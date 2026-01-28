//! Core traits and types for semantic normalization.

use std::collections::HashSet;
use tree_sitter::Parser;

/// A normalized representation of an AST node for semantic comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedNode {
    pub kind: String,
    pub text: Option<String>,
    pub children: Vec<NormalizedNode>,
}

impl NormalizedNode {
    /// Create a new normalized node with children.
    pub fn new(kind: String, children: Vec<NormalizedNode>) -> Self {
        Self {
            kind,
            text: None,
            children,
        }
    }

    /// Create a new leaf node with text content.
    pub fn leaf(kind: String, text: String) -> Self {
        Self {
            kind,
            text: Some(text),
            children: vec![],
        }
    }
}

/// Trait for providing a tree-sitter parser for a language.
///
/// This is the base trait for language support. It provides parser creation
/// and language identification. Languages with only parser support (no semantic
/// rules) can implement just this trait to enable AST and CST comparison.
pub trait ParserProvider: Send + Sync {
    /// Language identifiers this provider handles.
    ///
    /// Examples: `["javascript", "js", "jsx"]`, `["python", "py"]`
    fn language_ids(&self) -> &[&'static str];

    /// File extensions this provider handles (with leading dot).
    ///
    /// Examples: `[".js", ".mjs", ".cjs"]`, `[".py", ".pyi"]`
    fn file_extensions(&self) -> &[&'static str];

    /// Create a tree-sitter parser configured for this language.
    ///
    /// Returns `None` if the parser cannot be created.
    fn get_parser(&self) -> Option<Parser>;

    /// Check if this provider handles the given language identifier.
    fn handles_language(&self, language: &str) -> bool {
        self.language_ids()
            .iter()
            .any(|id| id.eq_ignore_ascii_case(language))
    }

    /// Check if this provider handles the given file extension.
    fn handles_extension(&self, extension: &str) -> bool {
        self.file_extensions()
            .iter()
            .any(|ext| ext.eq_ignore_ascii_case(extension))
    }
}

/// Trait for language-specific semantic normalization.
///
/// Extends [`ParserProvider`] with semantic rules for comparing code.
/// Implementations define how code in a specific language should be normalized
/// for semantic comparison. This includes:
/// - Which node types have unordered children (e.g., object properties)
/// - Custom normalization logic for specific constructs (e.g., Python keyword args)
pub trait SemanticNormalizer: ParserProvider {
    /// Node types whose children can be reordered without changing semantics.
    ///
    /// Examples: `object` in JavaScript, `dictionary` in Python
    fn unordered_node_types(&self) -> HashSet<&'static str>;

    /// Custom normalization for specific node types.
    ///
    /// Called during normalization to allow language-specific handling.
    /// Takes ownership of children to avoid cloning.
    ///
    /// # Arguments
    /// * `node_kind` - The kind of AST node being normalized
    /// * `children` - The node's children (takes ownership)
    ///
    /// # Returns
    /// A tuple of `(children, handled)`:
    /// - `children`: The (possibly reordered) children
    /// - `handled`: If `true`, the normalizer fully handled this node type and no
    ///   default sorting should be applied. If `false`, default sorting may be
    ///   applied based on `unordered_node_types`.
    ///
    /// # Example
    /// Python's `argument_list` needs special handling to sort keyword arguments
    /// while keeping positional arguments in order.
    fn normalize_children(
        &self,
        _node_kind: &str,
        children: Vec<NormalizedNode>,
    ) -> (Vec<NormalizedNode>, bool) {
        (children, false)
    }

    /// Node types where comment ordering should be normalized.
    ///
    /// For these node types, consecutive runs of comments are sorted by content
    /// while preserving their position relative to non-comment nodes. This allows
    /// comment content to be verified while making their exact order among
    /// adjacent comments irrelevant.
    ///
    /// Typically includes block/scope nodes where comments can appear between
    /// statements (e.g., `program`, `block`, `function_definition`).
    ///
    /// Default implementation returns an empty slice (no comment normalization).
    fn comment_scope_kinds(&self) -> &'static [&'static str] {
        &[]
    }
}
