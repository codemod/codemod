//! Semantic comparison for code testing.
//!
//! This module provides semantic comparison of code strings, allowing tests to pass
//! even when there are semantically irrelevant differences like:
//! - Object property ordering
//! - Dictionary key ordering
//! - Keyword argument ordering (Python)
//! - Whitespace differences
//!
//! # Example
//!
//! ```
//! use testing_utils::semantic::semantic_compare;
//!
//! // Object property order doesn't matter in JavaScript
//! let expected = r#"const obj = { a: 1, b: 2 };"#;
//! let actual = r#"const obj = { b: 2, a: 1 };"#;
//! assert!(semantic_compare(expected, actual, "javascript"));
//!
//! // Python keyword argument order doesn't matter
//! let expected = "func(a=1, b=2)";
//! let actual = "func(b=2, a=1)";
//! assert!(semantic_compare(expected, actual, "python"));
//! ```
//!
//! # Auto-detection
//!
//! Language can be automatically detected from file extensions:
//!
//! ```
//! use testing_utils::semantic::{detect_language, semantic_compare_with_path};
//! use std::path::Path;
//!
//! // Detect language from file extension
//! assert_eq!(detect_language(Path::new("file.py")), Some("python"));
//!
//! // Compare with automatic language detection
//! let path = Path::new("test.py");
//! let expected = "func(a=1, b=2)";
//! let actual = "func(b=2, a=1)";
//! assert!(semantic_compare_with_path(expected, actual, path, None));
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
    semantic_compare, semantic_compare_with_registry, try_semantic_compare,
    try_semantic_compare_with_registry, SemanticCompareError,
};
pub use detect::{
    detect_language, detect_language_from_path, semantic_compare_with_path,
    semantic_compare_with_path_and_registry,
};
pub use diff::{semantic_diff, semantic_diff_with_registry, DiffEntry, DiffKind, SemanticDiff};
pub use registry::NormalizerRegistry;
pub use traits::{NormalizedNode, SemanticNormalizer};

pub use go::GoNormalizer;
pub use javascript::JavaScriptNormalizer;
pub use json::JsonNormalizer;
pub use python::PythonNormalizer;
pub use rust_lang::RustNormalizer;
pub use typescript::{TsxNormalizer, TypeScriptNormalizer};
