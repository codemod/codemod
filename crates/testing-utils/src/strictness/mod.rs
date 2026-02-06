//! Strictness comparison for code testing.
//!
//! This module provides AST-based comparison of code strings, allowing tests to pass
//! even when there are semantically irrelevant differences like:
//! - Object property ordering
//! - Dictionary key ordering
//! - Keyword argument ordering (Python)
//! - Comment indentation differences (for non-indentation-sensitive languages like JS, TS, Go, Rust, JSON)
//!
//! Note: Python preserves indentation checking since indentation is semantically significant.
//!
//! Note: Whitespace normalization is handled separately by the test runner
//! (e.g., via the `ignore_whitespace` option for strict mode). The AST/CST/loose
//! comparison modes inherently handle whitespace differences through tree-based
//! comparison rather than string comparison.
//!
//! # Example
//!
//! ```
//! use testing_utils::strictness::loose_compare;
//!
//! // Object property order doesn't matter in JavaScript
//! let expected = r#"const obj = { a: 1, b: 2 };"#;
//! let actual = r#"const obj = { b: 2, a: 1 };"#;
//! assert!(loose_compare(expected, actual, "javascript"));
//!
//! // Python keyword argument order doesn't matter
//! let expected = "func(a=1, b=2)";
//! let actual = "func(b=2, a=1)";
//! assert!(loose_compare(expected, actual, "python"));
//! ```
//!
//! # Auto-detection
//!
//! Language can be automatically detected from file extensions:
//!
//! ```
//! use testing_utils::strictness::{detect_language, loose_compare_with_path};
//! use std::path::Path;
//!
//! // Detect language from file extension
//! assert_eq!(detect_language(Path::new("file.py")), Some("python"));
//!
//! // Compare with automatic language detection
//! let path = Path::new("test.py");
//! let expected = "func(a=1, b=2)";
//! let actual = "func(b=2, a=1)";
//! assert!(loose_compare_with_path(expected, actual, path, None));
//! ```

mod compare;
mod detect;
mod diff;
mod go;
mod javascript;
mod json;
mod python;
mod registry;
mod rust_lang;
mod traits;
mod typescript;
mod utils;

pub use compare::{
    ast_compare, ast_compare_with_parser_registry, ast_compare_with_registry, cst_compare,
    cst_compare_with_parser_registry, cst_compare_with_registry, loose_compare,
    loose_compare_with_registries, loose_compare_with_registry, try_ast_compare,
    try_ast_compare_with_parser_registry, try_ast_compare_with_registry, try_cst_compare,
    try_cst_compare_with_parser_registry, try_cst_compare_with_registry, try_loose_compare,
    try_loose_compare_with_registry, CompareError,
};
pub use detect::{
    detect_language, detect_language_from_path, loose_compare_with_path,
    loose_compare_with_path_and_registry,
};
pub use diff::{semantic_diff, semantic_diff_with_registry, DiffEntry, DiffKind, SemanticDiff};
pub use registry::{NormalizerRegistry, ParserRegistry};
pub use traits::{NormalizedNode, ParserProvider, SemanticNormalizer};

pub use go::GoNormalizer;
pub use javascript::JavaScriptNormalizer;
pub use json::JsonNormalizer;
pub use python::PythonNormalizer;
pub use rust_lang::RustNormalizer;
pub use typescript::{TsxNormalizer, TypeScriptNormalizer};
