//! Semantic comparison functions.

use std::fmt;
use tree_sitter::Node;

use super::registry::NormalizerRegistry;
use super::traits::{NormalizedNode, SemanticNormalizer};
use super::utils::extract_sort_key;

/// Errors that can occur during semantic comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticCompareError {
    /// No normalizer found for the given language.
    UnsupportedLanguage(String),
    /// Failed to create a parser for the language.
    ParserCreationFailed(String),
    /// Failed to parse the expected code.
    ExpectedParseFailed,
    /// Failed to parse the actual code.
    ActualParseFailed,
}

impl fmt::Display for SemanticCompareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedLanguage(lang) => {
                write!(f, "no semantic normalizer found for language '{}'", lang)
            }
            Self::ParserCreationFailed(lang) => {
                write!(f, "failed to create parser for language '{}'", lang)
            }
            Self::ExpectedParseFailed => write!(f, "failed to parse expected code"),
            Self::ActualParseFailed => write!(f, "failed to parse actual code"),
        }
    }
}

impl std::error::Error for SemanticCompareError {}

/// Compare two code strings for semantic equivalence.
///
/// Uses the default normalizer registry. Falls back to exact string comparison
/// if semantic comparison fails for any reason.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
///
/// # Returns
/// `true` if the code is semantically equivalent, `false` otherwise.
/// Falls back to exact string comparison if no normalizer is found or parsing fails.
pub fn semantic_compare(expected: &str, actual: &str, language: &str) -> bool {
    semantic_compare_with_registry(
        expected,
        actual,
        language,
        NormalizerRegistry::default_ref(),
    )
}

/// Compare two code strings for semantic equivalence using a custom registry.
///
/// Falls back to exact string comparison if semantic comparison fails.
///
/// # Arguments
/// * `expected` - The expected code string
/// * `actual` - The actual code string to compare
/// * `language` - Language identifier (e.g., "javascript", "python")
/// * `registry` - The normalizer registry to use
///
/// # Returns
/// `true` if the code is semantically equivalent, `false` otherwise.
/// Falls back to exact string comparison if no normalizer is found or parsing fails.
pub fn semantic_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> bool {
    try_semantic_compare_with_registry(expected, actual, language, registry)
        .unwrap_or_else(|_| expected == actual)
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
pub fn try_semantic_compare(
    expected: &str,
    actual: &str,
    language: &str,
) -> Result<bool, SemanticCompareError> {
    try_semantic_compare_with_registry(
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
pub fn try_semantic_compare_with_registry(
    expected: &str,
    actual: &str,
    language: &str,
    registry: &NormalizerRegistry,
) -> Result<bool, SemanticCompareError> {
    let normalizer = registry
        .get(language)
        .ok_or_else(|| SemanticCompareError::UnsupportedLanguage(language.to_string()))?;

    let mut parser = normalizer
        .get_parser()
        .ok_or_else(|| SemanticCompareError::ParserCreationFailed(language.to_string()))?;

    let expected_tree = parser
        .parse(expected, None)
        .ok_or(SemanticCompareError::ExpectedParseFailed)?;

    let actual_tree = parser
        .parse(actual, None)
        .ok_or(SemanticCompareError::ActualParseFailed)?;

    let expected_normalized =
        normalize_node(expected_tree.root_node(), expected.as_bytes(), normalizer);
    let actual_normalized = normalize_node(actual_tree.root_node(), actual.as_bytes(), normalizer);

    Ok(trees_equal(&expected_normalized, &actual_normalized))
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

    // Let the normalizer handle it first (no clone needed - takes ownership)
    let (mut children, handled) = normalizer.normalize_children(&kind, children);

    // Apply default sorting only if normalizer didn't handle it and node type is unordered
    if !handled && normalizer.unordered_node_types().contains(node.kind()) {
        children.sort_by_key(extract_sort_key);
    }

    NormalizedNode::new(kind, children)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_js_object_property_order() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_nested_object_property_order() {
        let expected = r#"const obj = { a: { x: 1, y: 2 }, b: 3 };"#;
        let actual = r#"const obj = { b: 3, a: { y: 2, x: 1 } };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_different_values_fail() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { a: 1, b: 3 };"#;
        assert!(!semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_array_order_matters() {
        let expected = r#"const arr = [1, 2, 3];"#;
        let actual = r#"const arr = [3, 2, 1];"#;
        assert!(!semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_python_keyword_arg_order() {
        let expected = "func(a=1, b=2)";
        let actual = "func(b=2, a=1)";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_mixed_positional_and_keyword() {
        let expected = "func(x, a=1, b=2)";
        let actual = "func(x, b=2, a=1)";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_positional_order_matters() {
        let expected = "func(1, 2)";
        let actual = "func(2, 1)";
        assert!(!semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_multiple_positional_then_keyword() {
        let expected = "func(1, 2, 3, a=1, b=2, c=3)";
        let actual = "func(1, 2, 3, c=3, a=1, b=2)";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_dict_key_order() {
        let expected = r#"d = {"a": 1, "b": 2}"#;
        let actual = r#"d = {"b": 2, "a": 1}"#;
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_ts_object_property_order() {
        let expected = r#"const obj: { a: number, b: string } = { a: 1, b: "x" };"#;
        let actual = r#"const obj: { b: string, a: number } = { b: "x", a: 1 };"#;
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_json_object_property_order() {
        let expected = r#"{"a": 1, "b": 2}"#;
        let actual = r#"{"b": 2, "a": 1}"#;
        assert!(semantic_compare(expected, actual, "json"));
    }

    #[test]
    fn test_unknown_language_falls_back_to_exact() {
        assert!(semantic_compare("some code", "some code", "unknown_lang"));
        assert!(!semantic_compare(
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

        assert!(semantic_compare_with_registry(
            "func(a=1, b=2)",
            "func(b=2, a=1)",
            "python",
            &registry
        ));

        assert!(!semantic_compare_with_registry(
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
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_named_exports_order() {
        let expected = r#"export { foo, bar, baz };"#;
        let actual = r#"export { baz, bar, foo };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_import_different_names_fail() {
        let expected = r#"import { foo, bar } from 'module';"#;
        let actual = r#"import { foo, baz } from 'module';"#;
        assert!(!semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_ts_named_imports_order() {
        let expected = r#"import { TypeA, TypeB, TypeC } from 'types';"#;
        let actual = r#"import { TypeC, TypeA, TypeB } from 'types';"#;
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_python_from_import_order() {
        let expected = "from typing import Dict, List, Optional";
        let actual = "from typing import Optional, List, Dict";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_from_import_different_names_fail() {
        let expected = "from typing import Dict, List";
        let actual = "from typing import Dict, Set";
        assert!(!semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_rust_trait_bounds_order() {
        let expected = "fn foo<T: Clone + Debug + Send>(x: T) {}";
        let actual = "fn foo<T: Send + Clone + Debug>(x: T) {}";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_where_clause_order() {
        let expected = "fn foo<T, U>() where T: Clone, U: Debug {}";
        let actual = "fn foo<T, U>() where U: Debug, T: Clone {}";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_different_bounds_fail() {
        let expected = "fn foo<T: Clone + Debug>(x: T) {}";
        let actual = "fn foo<T: Clone + Send>(x: T) {}";
        assert!(!semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_go_interface_method_order() {
        let expected = "type I interface { A(); B() }";
        let actual = "type I interface { B(); A() }";
        assert!(semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_interface_different_methods_fail() {
        let expected = "type I interface { A(); B() }";
        let actual = "type I interface { A(); C() }";
        assert!(!semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_ts_union_type_order() {
        let expected = "type T = A | B | C;";
        let actual = "type T = C | A | B;";
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_intersection_type_order() {
        let expected = "type T = A & B & C;";
        let actual = "type T = C & B & A;";
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_union_type_different_types_fail() {
        let expected = "type T = A | B | C;";
        let actual = "type T = A | B | D;";
        assert!(!semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_intersection_type_different_types_fail() {
        let expected = "type T = A & B & C;";
        let actual = "type T = A & B & D;";
        assert!(!semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_complex_union_type() {
        let expected = "type Result = Success | Error | Pending | Loading;";
        let actual = "type Result = Loading | Pending | Error | Success;";
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_tsx_union_type_order() {
        let expected = "type Props = A | B | C;";
        let actual = "type Props = C | B | A;";
        assert!(semantic_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_tsx_jsx_attribute_order() {
        let expected = r#"const x = <Button disabled size="lg" variant="primary" />;"#;
        let actual = r#"const x = <Button variant="primary" disabled size="lg" />;"#;
        assert!(semantic_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_tsx_jsx_attribute_different_values_fail() {
        let expected = r#"const x = <Button size="lg" />;"#;
        let actual = r#"const x = <Button size="sm" />;"#;
        assert!(!semantic_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_ts_implements_clause_order() {
        let expected = "class Foo implements A, B, C {}";
        let actual = "class Foo implements C, A, B {}";
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_interface_extends_order() {
        let expected = "interface Foo extends A, B, C {}";
        let actual = "interface Foo extends C, B, A {}";
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_implements_different_interfaces_fail() {
        let expected = "class Foo implements A, B {}";
        let actual = "class Foo implements A, C {}";
        assert!(!semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_python_import_statement_order() {
        let expected = "import os, sys, json";
        let actual = "import json, os, sys";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_import_different_modules_fail() {
        let expected = "import os, sys";
        let actual = "import os, json";
        assert!(!semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_global_statement_order() {
        let expected = "global a, b, c";
        let actual = "global c, a, b";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_rust_use_list_order() {
        let expected = "use std::{fs, io, path};";
        let actual = "use std::{path, fs, io};";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_use_list_different_items_fail() {
        let expected = "use std::{fs, io};";
        let actual = "use std::{fs, path};";
        assert!(!semantic_compare(expected, actual, "rust"));
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
        assert!(semantic_compare(expected, actual, "go"));
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
        assert!(!semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_ts_interface_method_order() {
        let expected = "interface I { a(): void; b(): void; c(): void; }";
        let actual = "interface I { c(): void; a(): void; b(): void; }";
        assert!(semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_ts_interface_different_methods_fail() {
        let expected = "interface I { a(): void; b(): void; }";
        let actual = "interface I { a(): void; c(): void; }";
        assert!(!semantic_compare(expected, actual, "typescript"));
    }

    #[test]
    fn test_go_type_constraint_union_order() {
        let expected = "type Number interface { int | int64 | float64 }";
        let actual = "type Number interface { float64 | int | int64 }";
        assert!(semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_type_constraint_different_types_fail() {
        let expected = "type Number interface { int | int64 }";
        let actual = "type Number interface { int | float64 }";
        assert!(!semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_rust_derive_order() {
        let expected = "#[derive(Debug, Clone, PartialEq)]\nstruct S;";
        let actual = "#[derive(PartialEq, Debug, Clone)]\nstruct S;";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_derive_different_traits_fail() {
        let expected = "#[derive(Debug, Clone)]\nstruct S;";
        let actual = "#[derive(Debug, Copy)]\nstruct S;";
        assert!(!semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_non_derive_attribute_unchanged() {
        let expected = "#[cfg(feature = \"a\")]\nfn foo() {}";
        let actual = "#[cfg(feature = \"a\")]\nfn foo() {}";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_python_except_tuple_order() {
        let expected = "try:\n    pass\nexcept (TypeError, ValueError, KeyError):\n    pass";
        let actual = "try:\n    pass\nexcept (KeyError, TypeError, ValueError):\n    pass";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_except_tuple_different_types_fail() {
        let expected = "try:\n    pass\nexcept (TypeError, ValueError):\n    pass";
        let actual = "try:\n    pass\nexcept (TypeError, KeyError):\n    pass";
        assert!(!semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_except_single_type_unchanged() {
        let expected = "try:\n    pass\nexcept TypeError:\n    pass";
        let actual = "try:\n    pass\nexcept TypeError:\n    pass";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_js_object_method_definition_order() {
        let expected = r#"const obj = { foo() {}, bar() {}, baz: 1 };"#;
        let actual = r#"const obj = { bar() {}, baz: 1, foo() {} };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_object_methods_only_order() {
        let expected = r#"const obj = { alpha() {}, beta() {}, gamma() {} };"#;
        let actual = r#"const obj = { gamma() {}, alpha() {}, beta() {} };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_object_method_different_body_fail() {
        let expected = r#"const obj = { foo() { return 1; } };"#;
        let actual = r#"const obj = { foo() { return 2; } };"#;
        assert!(!semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_rust_or_pattern_order() {
        let expected = "match x { A | B | C => {} }";
        let actual = "match x { C | A | B => {} }";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_or_pattern_with_struct_patterns() {
        let expected = "match x { Some(1) | Some(2) | None => {} }";
        let actual = "match x { None | Some(1) | Some(2) => {} }";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_or_pattern_different_patterns_fail() {
        let expected = "match x { A | B => {} }";
        let actual = "match x { A | C => {} }";
        assert!(!semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_python_type_union_order() {
        let expected = "def foo(x: int | str | None): pass";
        let actual = "def foo(x: None | str | int): pass";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_type_union_return_type() {
        let expected = "def foo() -> int | str | None: pass";
        let actual = "def foo() -> None | str | int: pass";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_type_union_variable_annotation() {
        let expected = "x: int | str = value";
        let actual = "x: str | int = value";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_type_union_different_types_fail() {
        let expected = "def foo(x: int | str): pass";
        let actual = "def foo(x: int | float): pass";
        assert!(!semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_go_embedded_interface_order() {
        let expected = "type I interface { Reader; Writer; Close() error }";
        let actual = "type I interface { Close() error; Writer; Reader }";
        assert!(semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_go_embedded_interface_different_fail() {
        let expected = "type I interface { Reader; Writer }";
        let actual = "type I interface { Reader; Closer }";
        assert!(!semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_js_object_spread_position_matters() {
        let expected = r#"const obj = { a: 1, ...x, b: 2 };"#;
        let actual = r#"const obj = { ...x, a: 1, b: 2 };"#;
        assert!(!semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_getter_setter_order() {
        let expected = r#"const obj = { get foo() {}, set foo(v) {}, get bar() {} };"#;
        let actual = r#"const obj = { get bar() {}, get foo() {}, set foo(v) {} };"#;
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_python_class_bases_order_matters() {
        let expected = "class C(A, B): pass";
        let actual = "class C(B, A): pass";
        assert!(!semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_python_class_metaclass_kwargs_order() {
        let expected = "class C(metaclass=M, kw1=v1, kw2=v2): pass";
        let actual = "class C(kw2=v2, metaclass=M, kw1=v1): pass";
        assert!(semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_js_shorthand_property_order() {
        let expected = "const obj = { b, a, c };";
        let actual = "const obj = { a, b, c };";
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_computed_property_order() {
        let expected = "const obj = { [b]: 2, [a]: 1 };";
        let actual = "const obj = { [a]: 1, [b]: 2 };";
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_js_mixed_property_types() {
        let expected = "const obj = { c, b: 1, a() {} };";
        let actual = "const obj = { a() {}, b: 1, c };";
        assert!(semantic_compare(expected, actual, "javascript"));
    }

    #[test]
    fn test_tsx_jsx_spread_position_matters() {
        let expected = r#"const x = <Button disabled {...props} />;"#;
        let actual = r#"const x = <Button {...props} disabled />;"#;
        assert!(!semantic_compare(expected, actual, "tsx"));
    }

    #[test]
    fn test_python_dict_splat_position_matters() {
        let expected = "d = {'a': 1, **x, 'b': 2}";
        let actual = "d = {**x, 'a': 1, 'b': 2}";
        assert!(!semantic_compare(expected, actual, "python"));
    }

    #[test]
    fn test_go_unkeyed_struct_order_matters() {
        let expected = "var p = Point{1, 2, 3}";
        let actual = "var p = Point{3, 2, 1}";
        assert!(!semantic_compare(expected, actual, "go"));
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
        assert!(semantic_compare(expected, actual, "go"));
    }

    #[test]
    fn test_rust_struct_base_must_be_last() {
        let expected = "let x = Foo { z: 1, a: 2, ..base };";
        let actual = "let x = Foo { a: 2, z: 1, ..base };";
        assert!(semantic_compare(expected, actual, "rust"));
    }

    #[test]
    fn test_rust_struct_base_different_fail() {
        let expected = "let x = Foo { a: 1, ..base1 };";
        let actual = "let x = Foo { a: 1, ..base2 };";
        assert!(!semantic_compare(expected, actual, "rust"));
    }

    // Error handling tests
    #[test]
    fn test_try_unsupported_language_error() {
        let result = try_semantic_compare("code", "code", "unknown_lang");
        assert!(matches!(
            result,
            Err(SemanticCompareError::UnsupportedLanguage(_))
        ));
    }

    #[test]
    fn test_try_semantic_compare_success() {
        let result = try_semantic_compare(
            r#"const obj = { a: 1, b: 2 };"#,
            r#"const obj = { b: 2, a: 1 };"#,
            "javascript",
        );
        assert_eq!(result, Ok(true));
    }

    #[test]
    fn test_try_semantic_compare_not_equal() {
        let result = try_semantic_compare(
            r#"const obj = { a: 1, b: 2 };"#,
            r#"const obj = { a: 1, b: 3 };"#,
            "javascript",
        );
        assert_eq!(result, Ok(false));
    }

    #[test]
    fn test_semantic_compare_error_display() {
        let err = SemanticCompareError::UnsupportedLanguage("foo".to_string());
        assert_eq!(
            err.to_string(),
            "no semantic normalizer found for language 'foo'"
        );

        let err = SemanticCompareError::ParserCreationFailed("bar".to_string());
        assert_eq!(
            err.to_string(),
            "failed to create parser for language 'bar'"
        );

        let err = SemanticCompareError::ExpectedParseFailed;
        assert_eq!(err.to_string(), "failed to parse expected code");

        let err = SemanticCompareError::ActualParseFailed;
        assert_eq!(err.to_string(), "failed to parse actual code");
    }
}
