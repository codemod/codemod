//! Python semantic analysis integration tests.

use ast_grep_language::SupportLang;
use codemod_sandbox::CodemodLang;

// =============================================================================
// Single-file (File Scope) Tests
// =============================================================================

jssg_test! {
    name: test_find_references_variable,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_find_references_variable.js",
    fixture_dir: "python/find_references_variable",
    target: "input.py",
}

jssg_test! {
    name: test_find_references_function,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_find_references_function.js",
    fixture_dir: "python/find_references_function",
    target: "input.py",
}

jssg_test! {
    name: test_find_references_class,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_find_references_class.js",
    fixture_dir: "python/find_references_class",
    target: "input.py",
}

// =============================================================================
// Cross-file (Workspace Scope) Tests
// =============================================================================

jssg_test! {
    name: test_cross_file_definition_workspace_scope,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_cross_file_definition.js",
    fixture_dir: "python/cross_file_definition",
    target: "main.py",
    scope: workspace,
}

jssg_test! {
    name: test_cross_file_references_workspace_scope,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_cross_file_references.js",
    fixture_dir: "python/cross_file_references",
    target: "utils.py",
    preprocess: ["main.py"],
    scope: workspace,
}

jssg_test! {
    name: test_cross_file_references_with_imports,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_cross_file_references_with_imports.js",
    fixture_dir: "python/cross_file_references_with_imports",
    target: "models.py",
    preprocess: ["app.py"],
    scope: workspace,
}

jssg_test! {
    name: test_false_positive_references_with_imports,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_false_positive_references.js",
    fixture_dir: "python/false_positive_references",
    target: "app.py",
    preprocess: ["really_imports.py", "models.py"],
    scope: workspace,
}

// =============================================================================
// Data flow and call graph (Workspace Scope)
// =============================================================================

jssg_test! {
    name: test_py_sql_taint_to_parameterized_queries,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_sql_taint.js",
    fixture_dir: "python/django_flow",
    target: "shop/views.py",
    scope: workspace,
    expected_dir: "python/django_flow_expected",
}

jssg_test! {
    name: test_py_calls_mutations_and_types,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_calls_and_mutations.js",
    fixture_dir: "python/django_flow",
    target: "shop/services.py",
    scope: workspace,
}

jssg_test! {
    name: test_py_fastapi_calls_and_types,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_fastapi_flow.js",
    fixture_dir: "python/fastapi_flow",
    target: "app/main.py",
    scope: workspace,
}

jssg_test! {
    name: test_py_fastapi_taint_into_text,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_fastapi_taint.js",
    fixture_dir: "python/fastapi_flow",
    target: "app/repository.py",
    scope: workspace,
}

jssg_test! {
    name: test_py_drf_views_and_celery_tasks,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "py_drf_celery.js",
    fixture_dir: "python/django_flow",
    target: "shop/api.py",
    scope: workspace,
}
