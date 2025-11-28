//! Python semantic analysis integration tests.

use super::fixtures::jssg_test;
use ast_grep_language::SupportLang;

// =============================================================================
// Single-file (File Scope) Tests
// =============================================================================

jssg_test! {
    name: test_find_references_variable,
    language: SupportLang::Python,
    codemod: "py_find_references_variable.js",
    fixture_dir: "python/find_references_variable",
    target: "input.py",
}

jssg_test! {
    name: test_find_references_function,
    language: SupportLang::Python,
    codemod: "py_find_references_function.js",
    fixture_dir: "python/find_references_function",
    target: "input.py",
}

jssg_test! {
    name: test_find_references_class,
    language: SupportLang::Python,
    codemod: "py_find_references_class.js",
    fixture_dir: "python/find_references_class",
    target: "input.py",
}

// =============================================================================
// Cross-file (Workspace Scope) Tests
// =============================================================================

jssg_test! {
    name: test_cross_file_definition_workspace_scope,
    language: SupportLang::Python,
    codemod: "py_cross_file_definition.js",
    fixture_dir: "python/cross_file_definition",
    target: "main.py",
    scope: workspace,
}

jssg_test! {
    name: test_cross_file_references_workspace_scope,
    language: SupportLang::Python,
    codemod: "py_cross_file_references.js",
    fixture_dir: "python/cross_file_references",
    target: "utils.py",
    preprocess: ["main.py"],
    scope: workspace,
}

jssg_test! {
    name: test_cross_file_references_with_imports,
    language: SupportLang::Python,
    codemod: "py_cross_file_references_with_imports.js",
    fixture_dir: "python/cross_file_references_with_imports",
    target: "models.py",
    preprocess: ["app.py"],
    scope: workspace,
}

jssg_test! {
    name: test_false_positive_references_with_imports,
    language: SupportLang::Python,
    codemod: "py_false_positive_references.js",
    fixture_dir: "python/false_positive_references",
    target: "app.py",
    preprocess: ["really_imports.py", "models.py"],
    scope: workspace,
}
