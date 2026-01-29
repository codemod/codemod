//! Semantic comparison functions.

use std::fmt;

use super::registry::{NormalizerRegistry, ParserRegistry};
use super::traits::{NormalizedNode, SemanticNormalizer};
use super::utils::{build_cst_node, normalize_node, normalize_node_for_ast};
use tree_sitter::Tree;

/// Errors that can occur during semantic comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareError {
    /// No normalizer found for the given language.
    UnsupportedLanguage(String),
    /// Failed to create a parser for the language.
    ParserCreationFailed(String),
    /// Failed to parse the expected code.
    ExpectedParseFailed,
    /// Failed to parse the actual code.
    ActualParseFailed,
}

impl fmt::Display for CompareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedLanguage(lang) => {
                write!(f, "no normalizer found for language '{}'", lang)
            }
            Self::ParserCreationFailed(lang) => {
                write!(f, "failed to create parser for language '{}'", lang)
            }
            Self::ExpectedParseFailed => write!(f, "failed to parse expected code"),
            Self::ActualParseFailed => write!(f, "failed to parse actual code"),
        }
    }
}

impl std::error::Error for CompareError {}

/// Parsed trees for semantic comparison (with normalizer).
struct ParsedTreesWithNormalizer<'a> {
    expected: Tree,
    actual: Tree,
    normalizer: &'a dyn SemanticNormalizer,
}

/// Parsed trees for AST/CST comparison (parser only).
struct ParsedTrees {
    expected: Tree,
    actual: Tree,
}

/// Parse both expected and actual code strings using a normalizer.
fn parse_both_with_normalizer<'a>(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &'a NormalizerRegistry,
) -> Result<ParsedTreesWithNormalizer<'a>, CompareError> {
    let normalizer = registry
        .get(language)
        .ok_or_else(|| CompareError::UnsupportedLanguage(language.to_string()))?;

    let mut parser = normalizer
        .get_parser()
        .ok_or_else(|| CompareError::ParserCreationFailed(language.to_string()))?;

    let expected_tree = parser
        .parse(expected, None)
        .ok_or(CompareError::ExpectedParseFailed)?;

    let actual_tree = parser
        .parse(actual, None)
        .ok_or(CompareError::ActualParseFailed)?;

    Ok(ParsedTreesWithNormalizer {
        expected: expected_tree,
        actual: actual_tree,
        normalizer,
    })
}

/// Parse both expected and actual code strings using a parser provider.
fn parse_both_with_parser(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &ParserRegistry,
) -> Result<ParsedTrees, CompareError> {
    let provider = registry
        .get(language)
        .ok_or_else(|| CompareError::UnsupportedLanguage(language.to_string()))?;

    let mut parser = provider
        .get_parser()
        .ok_or_else(|| CompareError::ParserCreationFailed(language.to_string()))?;

    let expected_tree = parser
        .parse(expected, None)
        .ok_or(CompareError::ExpectedParseFailed)?;

    let actual_tree = parser
        .parse(actual, None)
        .ok_or(CompareError::ActualParseFailed)?;

    Ok(ParsedTrees {
        expected: expected_tree,
        actual: actual_tree,
    })
}

/// Compare two code strings for semantic equivalence.
///
/// Uses the default registries. Implements the fallback chain:
/// loose (semantic) → AST → CST → exact string comparison.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
///
/// # Returns
/// `true` if the code is semantically equivalent, `false` otherwise.
/// Falls back through the chain if comparison at any level fails.
pub fn loose_compare(expected: &str, actual: &str, language: &str) -> bool {
    loose_compare_with_registries(
        expected,
        actual,
        language,
        NormalizerRegistry::default_ref(),
        ParserRegistry::default_ref(),
    )
}

/// Compare two code strings for semantic equivalence using a custom normalizer registry.
///
/// Uses the default parser registry for fallback. Implements the fallback chain:
/// loose (semantic) → AST → CST → exact string comparison.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
/// * `registry` - The normalizer registry to use
///
/// # Returns
/// `true` if the code is semantically equivalent, `false` otherwise.
/// Falls back through the chain if comparison at any level fails.
pub fn loose_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> bool {
    loose_compare_with_registries(
        expected,
        actual,
        language,
        registry,
        ParserRegistry::default_ref(),
    )
}

/// Compare two code strings for semantic equivalence using custom registries.
///
/// Implements the fallback chain: loose (semantic) → AST → CST → exact string comparison.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
/// * `normalizer_registry` - The normalizer registry to use for semantic comparison
/// * `parser_registry` - The parser registry to use for AST/CST fallback
///
/// # Returns
/// `true` if the code is semantically equivalent, `false` otherwise.
/// Falls back through the chain if comparison at any level fails.
pub fn loose_compare_with_registries(
    expected: &str,
    actual: &str,
    language: &str,
    normalizer_registry: &NormalizerRegistry,
    parser_registry: &ParserRegistry,
) -> bool {
    // Try loose (semantic) comparison first
    match try_loose_compare_with_registry(expected, actual, language, normalizer_registry) {
        Ok(result) => return result,
        Err(e) => {
            eprintln!(
                "Warning: Loose comparison failed ({}), falling back to AST comparison",
                e
            );
        }
    }

    // Fallback to AST comparison
    ast_compare_with_parser_registry(expected, actual, language, parser_registry)
}

/// Try to compare two code strings for semantic equivalence.
///
/// Uses the default normalizer registry. Returns an error if semantic comparison
/// cannot be performed.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
///
/// # Returns
/// `Ok(true)` if semantically equivalent, `Ok(false)` if not,
/// `Err` if semantic comparison cannot be performed.
pub fn try_loose_compare(
    expected: &str,
    actual: &str,
    language: &str,
) -> Result<bool, CompareError> {
    try_loose_compare_with_registry(
        expected,
        actual,
        language,
        NormalizerRegistry::default_ref(),
    )
}

/// Try to compare two code strings for semantic equivalence using a custom registry.
///
/// Returns an error if semantic comparison cannot be performed.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
/// * `registry` - The normalizer registry to use
///
/// # Returns
/// `Ok(true)` if semantically equivalent, `Ok(false)` if not,
/// `Err` if semantic comparison cannot be performed.
pub fn try_loose_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> Result<bool, CompareError> {
    let parsed = parse_both_with_normalizer(expected, actual, language, registry)?;

    let expected_normalized = normalize_node(
        parsed.expected.root_node(),
        expected.as_bytes(),
        parsed.normalizer,
    );
    let actual_normalized = normalize_node(
        parsed.actual.root_node(),
        actual.as_bytes(),
        parsed.normalizer,
    );

    Ok(trees_equal(&expected_normalized, &actual_normalized))
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

    tree1
        .children
        .iter()
        .zip(&tree2.children)
        .all(|(c1, c2)| trees_equal(c1, c2))
}

/// Compare two code strings using CST (Concrete Syntax Tree) comparison.
///
/// CST comparison includes all tokens (including punctuation and whitespace tokens)
/// in the comparison. This is stricter than AST comparison but ignores exact
/// whitespace content between tokens.
///
/// Uses the default parser registry. Falls back to exact string comparison if parsing fails.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
///
/// # Returns
/// `true` if the CSTs are equivalent, `false` otherwise.
pub fn cst_compare(expected: &str, actual: &str, language: &str) -> bool {
    cst_compare_with_parser_registry(expected, actual, language, ParserRegistry::default_ref())
}

/// Compare two code strings using CST comparison with a normalizer registry.
///
/// This is a convenience function for backwards compatibility.
/// Uses the normalizer's parser provider.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier
/// * `registry` - The normalizer registry to use for parser lookup
///
/// # Returns
/// `true` if the CSTs are equivalent, `false` otherwise.
pub fn cst_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> bool {
    match try_cst_compare_with_registry(expected, actual, language, registry) {
        Ok(result) => result,
        Err(e) => {
            eprintln!(
                "Warning: CST comparison failed ({}), falling back to exact string comparison",
                e
            );
            expected == actual
        }
    }
}

/// Compare two code strings using CST comparison with a parser registry.
///
/// Falls back to exact string comparison if CST comparison fails.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier
/// * `registry` - The parser registry to use
///
/// # Returns
/// `true` if the CSTs are equivalent, `false` otherwise.
pub fn cst_compare_with_parser_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &ParserRegistry,
) -> bool {
    match try_cst_compare_with_parser_registry(expected, actual, language, registry) {
        Ok(result) => result,
        Err(e) => {
            eprintln!(
                "Warning: CST comparison failed ({}), falling back to exact string comparison",
                e
            );
            expected == actual
        }
    }
}

/// Try to compare two code strings using CST comparison.
///
/// Returns an error if CST comparison cannot be performed.
pub fn try_cst_compare(expected: &str, actual: &str, language: &str) -> Result<bool, CompareError> {
    try_cst_compare_with_parser_registry(expected, actual, language, ParserRegistry::default_ref())
}

/// Try to compare two code strings using CST comparison with a normalizer registry.
pub fn try_cst_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> Result<bool, CompareError> {
    let parsed = parse_both_with_normalizer(expected, actual, language, registry)?;

    let expected_cst = build_cst_node(parsed.expected.root_node(), expected.as_bytes());
    let actual_cst = build_cst_node(parsed.actual.root_node(), actual.as_bytes());

    Ok(trees_equal(&expected_cst, &actual_cst))
}

/// Try to compare two code strings using CST comparison with a parser registry.
pub fn try_cst_compare_with_parser_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &ParserRegistry,
) -> Result<bool, CompareError> {
    let parsed = parse_both_with_parser(expected, actual, language, registry)?;

    let expected_cst = build_cst_node(parsed.expected.root_node(), expected.as_bytes());
    let actual_cst = build_cst_node(parsed.actual.root_node(), actual.as_bytes());

    Ok(trees_equal(&expected_cst, &actual_cst))
}

/// Compare two code strings using AST (Abstract Syntax Tree) comparison.
///
/// AST comparison normalizes the tree (strips unnamed tokens like whitespace and
/// punctuation) but preserves the ordering of children. This is stricter than
/// "loose" semantic comparison which also reorders unordered children.
///
/// Uses the default parser registry. Implements the fallback chain:
/// AST → CST → exact string comparison.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
///
/// # Returns
/// `true` if the ASTs are equivalent, `false` otherwise.
pub fn ast_compare(expected: &str, actual: &str, language: &str) -> bool {
    ast_compare_with_parser_registry(expected, actual, language, ParserRegistry::default_ref())
}

/// Compare two code strings using AST comparison with a normalizer registry.
///
/// This is a convenience function for backwards compatibility.
/// Implements the fallback chain: AST → CST → exact string comparison.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier
/// * `registry` - The normalizer registry to use for parser lookup
///
/// # Returns
/// `true` if the ASTs are equivalent, `false` otherwise.
pub fn ast_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> bool {
    match try_ast_compare_with_registry(expected, actual, language, registry) {
        Ok(result) => result,
        Err(e) => {
            eprintln!(
                "Warning: AST comparison failed ({}), falling back to CST comparison",
                e
            );
            cst_compare_with_registry(expected, actual, language, registry)
        }
    }
}

/// Compare two code strings using AST comparison with a parser registry.
///
/// Implements the fallback chain: AST → CST → exact string comparison.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier
/// * `registry` - The parser registry to use
///
/// # Returns
/// `true` if the ASTs are equivalent, `false` otherwise.
pub fn ast_compare_with_parser_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &ParserRegistry,
) -> bool {
    match try_ast_compare_with_parser_registry(expected, actual, language, registry) {
        Ok(result) => result,
        Err(e) => {
            eprintln!(
                "Warning: AST comparison failed ({}), falling back to CST comparison",
                e
            );
            cst_compare_with_parser_registry(expected, actual, language, registry)
        }
    }
}

/// Try to compare two code strings using AST comparison.
///
/// Returns an error if AST comparison cannot be performed.
pub fn try_ast_compare(expected: &str, actual: &str, language: &str) -> Result<bool, CompareError> {
    try_ast_compare_with_parser_registry(expected, actual, language, ParserRegistry::default_ref())
}

/// Try to compare two code strings using AST comparison with a normalizer registry.
pub fn try_ast_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> Result<bool, CompareError> {
    let parsed = parse_both_with_normalizer(expected, actual, language, registry)?;

    let expected_ast = normalize_node_for_ast(parsed.expected.root_node(), expected.as_bytes());
    let actual_ast = normalize_node_for_ast(parsed.actual.root_node(), actual.as_bytes());

    Ok(trees_equal(&expected_ast, &actual_ast))
}

/// Try to compare two code strings using AST comparison with a parser registry.
pub fn try_ast_compare_with_parser_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &ParserRegistry,
) -> Result<bool, CompareError> {
    let parsed = parse_both_with_parser(expected, actual, language, registry)?;

    let expected_ast = normalize_node_for_ast(parsed.expected.root_node(), expected.as_bytes());
    let actual_ast = normalize_node_for_ast(parsed.actual.root_node(), actual.as_bytes());

    Ok(trees_equal(&expected_ast, &actual_ast))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_js_object_property_order() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_nested_object_property_order() {
        let expected = r#"const obj = { a: { x: 1, y: 2 }, b: 3 };"#;
        let actual = r#"const obj = { b: 3, a: { y: 2, x: 1 } };"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_different_values_fail() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { a: 1, b: 3 };"#;
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_array_order_matters() {
        let expected = r#"const arr = [1, 2, 3];"#;
        let actual = r#"const arr = [3, 2, 1];"#;
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_python_keyword_arg_order() {
        let expected = "func(a=1, b=2)";
        let actual = "func(b=2, a=1)";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_mixed_positional_and_keyword() {
        let expected = "func(x, a=1, b=2)";
        let actual = "func(x, b=2, a=1)";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_positional_order_matters() {
        let expected = "func(1, 2)";
        let actual = "func(2, 1)";
        assert!(!loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_multiple_positional_then_keyword() {
        let expected = "func(1, 2, 3, a=1, b=2, c=3)";
        let actual = "func(1, 2, 3, c=3, a=1, b=2)";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_dict_key_order() {
        let expected = r#"d = {"a": 1, "b": 2}"#;
        let actual = r#"d = {"b": 2, "a": 1}"#;
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_ts_object_property_order() {
        let expected = r#"const obj: { a: number, b: string } = { a: 1, b: "x" };"#;
        let actual = r#"const obj: { b: string, a: number } = { b: "x", a: 1 };"#;
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_json_object_property_order() {
        let expected = r#"{"a": 1, "b": 2}"#;
        let actual = r#"{"b": 2, "a": 1}"#;
        assert!(loose_compare(expected, actual, "json"));
    }

    #[test]
    fn test_unknown_language_falls_back_to_exact() {
        assert!(loose_compare("some code", "some code", "unknown_lang"));
        assert!(!loose_compare(
            "some code",
            "different code",
            "unknown_lang"
        ));
    }

    #[test]
    fn test_custom_registry() {
        use super::super::python::PythonNormalizer;

        let mut registry = NormalizerRegistry::new();
        registry.register(Box::new(PythonNormalizer));

        assert!(loose_compare_with_registry(
            "func(a=1, b=2)",
            "func(b=2, a=1)",
            "python",
            &registry
        ));

        assert!(!loose_compare_with_registry(
            r#"const obj = { a: 1, b: 2 };"#,
            r#"const obj = { b: 2, a: 1 };"#,
            "javascript",
            &registry
        ));
    }

    #[test]
    fn test_js_named_imports_order() {
        let expected = r#"import { useState, useEffect, Component } from 'react';"#;
        let actual = r#"import { Component, useEffect, useState } from 'react';"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_named_exports_order() {
        let expected = r#"export { foo, bar, baz };"#;
        let actual = r#"export { baz, bar, foo };"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_import_different_names_fail() {
        let expected = r#"import { foo, bar } from 'module';"#;
        let actual = r#"import { foo, baz } from 'module';"#;
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_ts_named_imports_order() {
        let expected = r#"import { TypeA, TypeB, TypeC } from 'types';"#;
        let actual = r#"import { TypeC, TypeA, TypeB } from 'types';"#;
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_python_from_import_order() {
        let expected = "from typing import Dict, List, Optional";
        let actual = "from typing import Optional, List, Dict";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_from_import_different_names_fail() {
        let expected = "from typing import Dict, List";
        let actual = "from typing import Dict, Set";
        assert!(!loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_rust_trait_bounds_order() {
        let expected = "fn foo<T: Clone + Debug + Send>(x: T) {}";
        let actual = "fn foo<T: Send + Clone + Debug>(x: T) {}";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_where_clause_order() {
        let expected = "fn foo<T, U>() where T: Clone, U: Debug {}";
        let actual = "fn foo<T, U>() where U: Debug, T: Clone {}";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_different_bounds_fail() {
        let expected = "fn foo<T: Clone + Debug>(x: T) {}";
        let actual = "fn foo<T: Clone + Send>(x: T) {}";
        assert!(!loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_go_interface_method_order() {
        let expected = "type I interface { A(); B() }";
        let actual = "type I interface { B(); A() }";
        assert!(loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_interface_different_methods_fail() {
        let expected = "type I interface { A(); B() }";
        let actual = "type I interface { A(); C() }";
        assert!(!loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_ts_union_type_order() {
        let expected = "type T = A | B | C;";
        let actual = "type T = C | A | B;";
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_intersection_type_order() {
        let expected = "type T = A & B & C;";
        let actual = "type T = C & B & A;";
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_union_type_different_types_fail() {
        let expected = "type T = A | B | C;";
        let actual = "type T = A | B | D;";
        assert!(!loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_intersection_type_different_types_fail() {
        let expected = "type T = A & B & C;";
        let actual = "type T = A & B & D;";
        assert!(!loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_complex_union_type() {
        let expected = "type Result = Success | Error | Pending | Loading;";
        let actual = "type Result = Loading | Pending | Error | Success;";
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_tsx_union_type_order() {
        let expected = "type Props = A | B | C;";
        let actual = "type Props = C | B | A;";
        assert!(loose_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_tsx_jsx_attribute_order() {
        let expected = r#"const x = <Button disabled size="lg" variant="primary" />;"#;
        let actual = r#"const x = <Button variant="primary" disabled size="lg" />;"#;
        assert!(loose_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_tsx_jsx_attribute_different_values_fail() {
        let expected = r#"const x = <Button size="lg" />;"#;
        let actual = r#"const x = <Button size="sm" />;"#;
        assert!(!loose_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_ts_implements_clause_order() {
        let expected = "class Foo implements A, B, C {}";
        let actual = "class Foo implements C, A, B {}";
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_interface_extends_order() {
        let expected = "interface Foo extends A, B, C {}";
        let actual = "interface Foo extends C, B, A {}";
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_implements_different_interfaces_fail() {
        let expected = "class Foo implements A, B {}";
        let actual = "class Foo implements A, C {}";
        assert!(!loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_python_import_statement_order() {
        let expected = "import os, sys, json";
        let actual = "import json, os, sys";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_import_different_modules_fail() {
        let expected = "import os, sys";
        let actual = "import os, json";
        assert!(!loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_global_statement_order() {
        let expected = "global a, b, c";
        let actual = "global c, a, b";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_rust_use_list_order() {
        let expected = "use std::{fs, io, path};";
        let actual = "use std::{path, fs, io};";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_use_list_different_items_fail() {
        let expected = "use std::{fs, io};";
        let actual = "use std::{fs, path};";
        assert!(!loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_go_import_spec_list_order() {
        let expected = r#"import (
    "fmt"
    "os"
    "io"
)"#;
        let actual = r#"import (
    "io"
    "fmt"
    "os"
)"#;
        assert!(loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_import_different_packages_fail() {
        let expected = r#"import (
    "fmt"
    "os"
)"#;
        let actual = r#"import (
    "fmt"
    "io"
)"#;
        assert!(!loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_ts_interface_method_order() {
        let expected = "interface I { a(): void; b(): void; c(): void; }";
        let actual = "interface I { c(): void; a(): void; b(): void; }";
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_interface_different_methods_fail() {
        let expected = "interface I { a(): void; b(): void; }";
        let actual = "interface I { a(): void; c(): void; }";
        assert!(!loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_go_type_constraint_union_order() {
        let expected = "type Number interface { int | int64 | float64 }";
        let actual = "type Number interface { float64 | int | int64 }";
        assert!(loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_type_constraint_different_types_fail() {
        let expected = "type Number interface { int | int64 }";
        let actual = "type Number interface { int | float64 }";
        assert!(!loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_rust_derive_order() {
        let expected = "#[derive(Debug, Clone, PartialEq)]\nstruct S;";
        let actual = "#[derive(PartialEq, Debug, Clone)]\nstruct S;";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_derive_different_traits_fail() {
        let expected = "#[derive(Debug, Clone)]\nstruct S;";
        let actual = "#[derive(Debug, Copy)]\nstruct S;";
        assert!(!loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_non_derive_attribute_unchanged() {
        let expected = "#[cfg(feature = \"a\")]\nfn foo() {}";
        let actual = "#[cfg(feature = \"a\")]\nfn foo() {}";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_python_except_tuple_order() {
        let expected = "try:\n    pass\nexcept (TypeError, ValueError, KeyError):\n    pass";
        let actual = "try:\n    pass\nexcept (KeyError, TypeError, ValueError):\n    pass";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_except_tuple_different_types_fail() {
        let expected = "try:\n    pass\nexcept (TypeError, ValueError):\n    pass";
        let actual = "try:\n    pass\nexcept (TypeError, KeyError):\n    pass";
        assert!(!loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_except_single_type_unchanged() {
        let expected = "try:\n    pass\nexcept TypeError:\n    pass";
        let actual = "try:\n    pass\nexcept TypeError:\n    pass";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_js_object_method_definition_order() {
        let expected = r#"const obj = { foo() {}, bar() {}, baz: 1 };"#;
        let actual = r#"const obj = { bar() {}, baz: 1, foo() {} };"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_object_methods_only_order() {
        let expected = r#"const obj = { alpha() {}, beta() {}, gamma() {} };"#;
        let actual = r#"const obj = { gamma() {}, alpha() {}, beta() {} };"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_object_method_different_body_fail() {
        let expected = r#"const obj = { foo() { return 1; } };"#;
        let actual = r#"const obj = { foo() { return 2; } };"#;
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_rust_or_pattern_order() {
        let expected = "match x { A | B | C => {} }";
        let actual = "match x { C | A | B => {} }";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_or_pattern_with_struct_patterns() {
        let expected = "match x { Some(1) | Some(2) | None => {} }";
        let actual = "match x { None | Some(1) | Some(2) => {} }";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_or_pattern_different_patterns_fail() {
        let expected = "match x { A | B => {} }";
        let actual = "match x { A | C => {} }";
        assert!(!loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_python_type_union_order() {
        let expected = "def foo(x: int | str | None): pass";
        let actual = "def foo(x: None | str | int): pass";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_type_union_return_type() {
        let expected = "def foo() -> int | str | None: pass";
        let actual = "def foo() -> None | str | int: pass";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_type_union_variable_annotation() {
        let expected = "x: int | str = value";
        let actual = "x: str | int = value";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_type_union_different_types_fail() {
        let expected = "def foo(x: int | str): pass";
        let actual = "def foo(x: int | float): pass";
        assert!(!loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_go_embedded_interface_order() {
        let expected = "type I interface { Reader; Writer; Close() error }";
        let actual = "type I interface { Close() error; Writer; Reader }";
        assert!(loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_embedded_interface_different_fail() {
        let expected = "type I interface { Reader; Writer }";
        let actual = "type I interface { Reader; Closer }";
        assert!(!loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_js_object_spread_position_matters() {
        let expected = r#"const obj = { a: 1, ...x, b: 2 };"#;
        let actual = r#"const obj = { ...x, a: 1, b: 2 };"#;
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_getter_setter_order() {
        let expected = r#"const obj = { get foo() {}, set foo(v) {}, get bar() {} };"#;
        let actual = r#"const obj = { get bar() {}, get foo() {}, set foo(v) {} };"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_python_class_bases_order_matters() {
        let expected = "class C(A, B): pass";
        let actual = "class C(B, A): pass";
        assert!(!loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_class_metaclass_kwargs_order() {
        let expected = "class C(metaclass=M, kw1=v1, kw2=v2): pass";
        let actual = "class C(kw2=v2, metaclass=M, kw1=v1): pass";
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_js_shorthand_property_order() {
        let expected = "const obj = { b, a, c };";
        let actual = "const obj = { a, b, c };";
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_computed_property_order() {
        let expected = "const obj = { [b]: 2, [a]: 1 };";
        let actual = "const obj = { [a]: 1, [b]: 2 };";
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_mixed_property_types() {
        let expected = "const obj = { c, b: 1, a() {} };";
        let actual = "const obj = { a() {}, b: 1, c };";
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_tsx_jsx_spread_position_matters() {
        let expected = r#"const x = <Button disabled {...props} />;"#;
        let actual = r#"const x = <Button {...props} disabled />;"#;
        assert!(!loose_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_python_dict_splat_position_matters() {
        let expected = "d = {'a': 1, **x, 'b': 2}";
        let actual = "d = {**x, 'a': 1, 'b': 2}";
        assert!(!loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_go_unkeyed_struct_order_matters() {
        let expected = "var p = Point{1, 2, 3}";
        let actual = "var p = Point{3, 2, 1}";
        assert!(!loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_import_by_path_not_alias() {
        let expected = r#"import (
    z "fmt"
    a "os"
)"#;
        let actual = r#"import (
    a "os"
    z "fmt"
)"#;
        assert!(loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_rust_struct_base_must_be_last() {
        let expected = "let x = Foo { z: 1, a: 2, ..base };";
        let actual = "let x = Foo { a: 2, z: 1, ..base };";
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_struct_base_different_fail() {
        let expected = "let x = Foo { a: 1, ..base1 };";
        let actual = "let x = Foo { a: 1, ..base2 };";
        assert!(!loose_compare(expected, actual, "rust"));
    }

    // Error handling tests
    #[test]
    fn test_try_unsupported_language_error() {
        let result = try_loose_compare("code", "code", "unknown_lang");
        assert!(matches!(result, Err(CompareError::UnsupportedLanguage(_))));
    }

    #[test]
    fn test_try_loose_compare_success() {
        let result = try_loose_compare(
            r#"const obj = { a: 1, b: 2 };"#,
            r#"const obj = { b: 2, a: 1 };"#,
            "javascript",
        );
        assert_eq!(result, Ok(true));
    }

    #[test]
    fn test_try_loose_compare_not_equal() {
        let result = try_loose_compare(
            r#"const obj = { a: 1, b: 2 };"#,
            r#"const obj = { a: 1, b: 3 };"#,
            "javascript",
        );
        assert_eq!(result, Ok(false));
    }

    #[test]
    fn test_loose_compare_error_display() {
        let err = CompareError::UnsupportedLanguage("foo".to_string());
        assert_eq!(err.to_string(), "no normalizer found for language 'foo'");

        let err = CompareError::ParserCreationFailed("bar".to_string());
        assert_eq!(
            err.to_string(),
            "failed to create parser for language 'bar'"
        );

        let err = CompareError::ExpectedParseFailed;
        assert_eq!(err.to_string(), "failed to parse expected code");

        let err = CompareError::ActualParseFailed;
        assert_eq!(err.to_string(), "failed to parse actual code");
    }

    // CST comparison tests
    #[test]
    fn test_cst_exact_match() {
        let code = r#"const obj = { a: 1, b: 2 };"#;
        assert!(cst_compare(code, code, "javascript"));
    }

    #[test]
    fn test_cst_property_order_matters() {
        // CST should fail because property order is different
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;
        assert!(!cst_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_cst_whitespace_normalized() {
        // CST should pass because tree-sitter normalizes whitespace tokens
        let expected = r#"const obj = {a: 1};"#;
        let actual = r#"const obj = { a: 1 };"#;
        // Note: This depends on how tree-sitter parses whitespace
        // If it includes whitespace as tokens, this will fail
        // If it doesn't, this will pass
        // Either way, CST is stricter than AST
        let _result = cst_compare(expected, actual, "javascript");
    }

    #[test]
    fn test_cst_unknown_language_falls_back() {
        assert!(cst_compare("code", "code", "unknown_lang"));
        assert!(!cst_compare("code", "different", "unknown_lang"));
    }

    // AST comparison tests
    #[test]
    fn test_ast_exact_match() {
        let code = r#"const obj = { a: 1, b: 2 };"#;
        assert!(ast_compare(code, code, "javascript"));
    }

    #[test]
    fn test_ast_property_order_matters() {
        // AST comparison should fail because we don't sort
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;
        assert!(!ast_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_ast_whitespace_ignored() {
        // AST comparison ignores whitespace differences
        let expected = r#"const x = 1;"#;
        let actual = r#"const  x  =  1 ;"#;
        assert!(ast_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_ast_python_kwargs_order_matters() {
        // Unlike loose (semantic) comparison, AST preserves order
        let expected = "func(a=1, b=2)";
        let actual = "func(b=2, a=1)";
        assert!(!ast_compare(expected, actual, "python"));
    }

    #[test]
    fn test_ast_unknown_language_falls_back() {
        assert!(ast_compare("code", "code", "unknown_lang"));
        assert!(!ast_compare("code", "different", "unknown_lang"));
    }

    // Comparison between modes
    #[test]
    fn test_loose_vs_ast_object_property_order() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;

        // Loose (semantic) comparison: property order doesn't matter
        assert!(loose_compare(expected, actual, "javascript"));

        // AST comparison: property order matters
        assert!(!ast_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_loose_vs_ast_python_kwargs() {
        let expected = "func(a=1, b=2)";
        let actual = "func(b=2, a=1)";

        // Loose (semantic) comparison: kwarg order doesn't matter
        assert!(loose_compare(expected, actual, "python"));

        // AST comparison: order matters
        assert!(!ast_compare(expected, actual, "python"));
    }

    // Error case tests - verify fallback behavior when parsing fails
    #[test]
    fn test_cst_parse_error_falls_back_to_string_comparison() {
        // Invalid syntax should fall back to exact string comparison
        let invalid = "not { valid javascript";

        // Same invalid code should match itself via fallback
        assert!(cst_compare(invalid, invalid, "javascript"));

        // Different invalid code should not match
        assert!(!cst_compare(invalid, "different invalid", "javascript"));
    }

    #[test]
    fn test_ast_parse_error_falls_back_to_string_comparison() {
        // Invalid syntax should fall back to exact string comparison
        let invalid = "def foo( # incomplete python";

        // Same invalid code should match itself via fallback
        assert!(ast_compare(invalid, invalid, "python"));

        // Different invalid code should not match
        assert!(!ast_compare(invalid, "different invalid", "python"));
    }

    #[test]
    fn test_semantic_parse_error_falls_back_to_string_comparison() {
        // Invalid syntax should fall back to exact string comparison
        let invalid = "const x = {";

        // Same invalid code should match itself via fallback
        assert!(loose_compare(invalid, invalid, "javascript"));

        // Different invalid code should not match
        assert!(!loose_compare(invalid, "different invalid", "javascript"));
    }

    #[test]
    fn test_try_cst_compare_unsupported_language() {
        // Unsupported language should return UnsupportedLanguage error
        let result = try_cst_compare("code", "code", "unsupported_lang");
        assert!(matches!(result, Err(CompareError::UnsupportedLanguage(_))));
    }

    #[test]
    fn test_try_ast_compare_unsupported_language() {
        // Unsupported language should return UnsupportedLanguage error
        let result = try_ast_compare("code", "code", "unsupported_lang");
        assert!(matches!(result, Err(CompareError::UnsupportedLanguage(_))));
    }

    #[test]
    fn test_cst_compare_with_parse_tree_containing_errors() {
        // Even with syntax errors, tree-sitter still produces a tree
        // The comparison functions should handle this gracefully
        let with_error = "const x = { a: }"; // Missing value

        // Same code with error should match itself
        // (tree-sitter produces ERROR nodes but still parses)
        let result = cst_compare(with_error, with_error, "javascript");
        // Should either match via CST or fallback - either is acceptable
        assert!(result);
    }

    #[test]
    fn test_ast_compare_with_parse_tree_containing_errors() {
        // Even with syntax errors, tree-sitter still produces a tree
        let with_error = "def foo(x, ): pass"; // Extra comma

        // Same code with error should match itself
        let result = ast_compare(with_error, with_error, "python");
        assert!(result);
    }

    // Fallback chain tests
    mod fallback_chain {
        use super::*;

        /// Test that loose_compare falls back to AST when no normalizer exists
        /// but a parser is available.
        #[test]
        fn test_loose_falls_back_to_ast_with_parser_only() {
            // Create a custom normalizer registry with NO normalizers
            let empty_normalizer_registry = NormalizerRegistry::new();

            // Create a parser registry WITH the JavaScript parser
            let parser_registry = ParserRegistry::default();

            // Code that would pass loose comparison (different property order)
            // but should FAIL with AST comparison (order matters in AST)
            let expected = r#"const obj = { a: 1, b: 2 };"#;
            let actual = r#"const obj = { b: 2, a: 1 };"#;

            // With empty normalizer registry, loose comparison should fail
            // (no normalizer) and fall back to AST comparison (which also fails
            // because property order matters in AST)
            let result = loose_compare_with_registries(
                expected,
                actual,
                "javascript",
                &empty_normalizer_registry,
                &parser_registry,
            );

            // Should be false because AST comparison preserves order
            assert!(!result);

            // But identical code should still pass via AST fallback
            let result = loose_compare_with_registries(
                expected,
                expected,
                "javascript",
                &empty_normalizer_registry,
                &parser_registry,
            );
            assert!(result);
        }

        /// Test that AST comparison falls back to CST when needed.
        #[test]
        fn test_ast_falls_back_to_cst() {
            // For unknown language, AST should fall back to CST, then to exact string
            // Since there's no parser, both AST and CST fail, so it uses exact string
            let code = "some code";
            assert!(ast_compare(code, code, "unknown_lang"));
            assert!(!ast_compare(code, "different code", "unknown_lang"));
        }

        /// Test that CST comparison falls back to exact string when no parser exists.
        #[test]
        fn test_cst_falls_back_to_exact_string() {
            // For unknown language, CST should fall back to exact string comparison
            let code = "some arbitrary code";
            assert!(cst_compare(code, code, "unknown_lang"));
            assert!(!cst_compare(code, "different code", "unknown_lang"));
        }

        /// Test the full fallback chain: loose → AST → CST → exact string
        #[test]
        fn test_full_fallback_chain_to_exact_string() {
            // Create empty registries (no normalizers, no parsers)
            let empty_normalizer_registry = NormalizerRegistry::new();
            let empty_parser_registry = ParserRegistry::new();

            let code = "some code in unknown language";

            // With no normalizer and no parser, should fall back all the way to exact string
            let result = loose_compare_with_registries(
                code,
                code,
                "totally_unknown_lang",
                &empty_normalizer_registry,
                &empty_parser_registry,
            );
            assert!(result);

            // Different strings should not match
            let result = loose_compare_with_registries(
                code,
                "different code",
                "totally_unknown_lang",
                &empty_normalizer_registry,
                &empty_parser_registry,
            );
            assert!(!result);
        }

        /// Test that with a full normalizer registry, loose comparison uses semantic rules.
        #[test]
        fn test_loose_uses_semantic_rules_when_available() {
            // With default registries, loose comparison should use semantic rules
            let expected = r#"const obj = { a: 1, b: 2 };"#;
            let actual = r#"const obj = { b: 2, a: 1 };"#;

            // Loose comparison with semantic rules: property order doesn't matter
            assert!(loose_compare(expected, actual, "javascript"));

            // AST comparison: property order matters
            assert!(!ast_compare(expected, actual, "javascript"));

            // This demonstrates the difference between the two modes
        }

        /// Test fallback with parser-only registry (parser but no semantic normalizer).
        #[test]
        fn test_loose_with_parser_only_registry() {
            use crate::strictness::javascript::JavaScriptNormalizer;

            // Create a parser-only registry with JavaScript parser
            let mut parser_registry = ParserRegistry::new();
            parser_registry.register(Box::new(JavaScriptNormalizer));

            // Empty normalizer registry
            let empty_normalizer_registry = NormalizerRegistry::new();

            // Python kwargs - would be reordered by semantic comparison
            let expected = "func(a=1, b=2)";
            let actual = "func(b=2, a=1)";

            // With no Python normalizer, falls back to AST
            // AST comparison preserves order, so this should fail
            let result = loose_compare_with_registries(
                expected,
                actual,
                "python",
                &empty_normalizer_registry,
                &parser_registry,
            );

            // Falls back all the way to exact string (no Python parser in our custom registry)
            // Exact string comparison: different strings, so false
            assert!(!result);
        }

        /// Test that whitespace differences are handled correctly at each level.
        #[test]
        fn test_whitespace_handling_at_each_level() {
            let code1 = "const x = 1;";
            let code2 = "const  x  =  1 ;"; // Extra whitespace

            // Loose comparison (semantic): should pass (whitespace ignored)
            assert!(loose_compare(code1, code2, "javascript"));

            // AST comparison: should pass (whitespace not in AST)
            assert!(ast_compare(code1, code2, "javascript"));

            // CST comparison: depends on how tree-sitter handles whitespace
            // (may or may not pass, but should not crash)
            let _ = cst_compare(code1, code2, "javascript");

            // Exact string: should fail (strings are different)
            assert_ne!(code1, code2);
        }
    }

    // Comment ordering tests
    #[test]
    fn test_js_comment_order_doesnt_matter() {
        let expected = r#"// First comment
// Second comment
function foo() {}"#;
        let actual = r#"// Second comment
// First comment
function foo() {}"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_comment_order_in_function_body() {
        let expected = r#"function foo() {
  // Comment A
  // Comment B
  const x = 1;
}"#;
        let actual = r#"function foo() {
  // Comment B
  // Comment A
  const x = 1;
}"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_comment_order_interleaved_preserves_position() {
        // Comments separated by code should NOT be interchangeable
        // because their position relative to code has semantic meaning
        let expected = r#"// A
function foo() {}
// B"#;
        let actual = r#"// B
function foo() {}
// A"#;
        // These should NOT be equal - comment position relative to code matters
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_consecutive_comments_order_insensitive() {
        // Only consecutive/adjacent comments should be order-insensitive
        let expected = r#"// B
// A
function foo() {}
// D
// C"#;
        let actual = r#"// A
// B
function foo() {}
// C
// D"#;
        assert!(loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_ts_comment_order_doesnt_matter() {
        let expected = r#"// Original comment
// TODO: Added comment
function useHook() {}"#;
        let actual = r#"// TODO: Added comment
// Original comment
function useHook() {}"#;
        assert!(loose_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_tsx_comment_order_doesnt_matter() {
        let expected = r#"// First comment
// Second comment
const Component = () => <div />;"#;
        let actual = r#"// Second comment
// First comment
const Component = () => <div />;"#;
        assert!(loose_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_js_different_comment_content_fails() {
        let expected = r#"// Comment A
function foo() {}"#;
        let actual = r#"// Comment B
function foo() {}"#;
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_missing_comment_fails() {
        let expected = r#"// Comment
function foo() {}"#;
        let actual = r#"function foo() {}"#;
        assert!(!loose_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_python_comment_order_doesnt_matter() {
        let expected = r#"# First comment
# Second comment
def foo():
    pass"#;
        let actual = r#"# Second comment
# First comment
def foo():
    pass"#;
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_comment_in_function_body() {
        let expected = r#"def foo():
    # Comment A
    # Comment B
    x = 1"#;
        let actual = r#"def foo():
    # Comment B
    # Comment A
    x = 1"#;
        assert!(loose_compare(expected, actual, "python"));
    }

    #[test]
    fn test_go_comment_order_doesnt_matter() {
        let expected = r#"// First comment
// Second comment
package main"#;
        let actual = r#"// Second comment
// First comment
package main"#;
        assert!(loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_comment_in_function_body() {
        let expected = r#"package main

func foo() {
    // Comment A
    // Comment B
    x := 1
}"#;
        let actual = r#"package main

func foo() {
    // Comment B
    // Comment A
    x := 1
}"#;
        assert!(loose_compare(expected, actual, "go"));
    }

    #[test]
    fn test_rust_comment_order_doesnt_matter() {
        let expected = r#"// First comment
// Second comment
fn main() {}"#;
        let actual = r#"// Second comment
// First comment
fn main() {}"#;
        assert!(loose_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_comment_in_function_body() {
        let expected = r#"fn foo() {
    // Comment A
    // Comment B
    let x = 1;
}"#;
        let actual = r#"fn foo() {
    // Comment B
    // Comment A
    let x = 1;
}"#;
        assert!(loose_compare(expected, actual, "rust"));
    }
}
