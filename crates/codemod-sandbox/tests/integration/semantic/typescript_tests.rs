//! TypeScript semantic analysis integration tests.

use super::fixtures::jssg_test;
use ast_grep_language::SupportLang;

jssg_test! {
    name: test_get_definition_file_scope,
    language: SupportLang::TypeScript,
    codemod: "ts_definition_file_scope.js",
    fixture_dir: "typescript/definition_file_scope",
    target: "input.ts",
}

jssg_test! {
    name: test_find_references_file_scope,
    language: SupportLang::TypeScript,
    codemod: "ts_find_references_file_scope.js",
    fixture_dir: "typescript/find_references_file_scope",
    target: "input.ts",
}

jssg_test! {
    name: test_find_references_function_same_file,
    language: SupportLang::TypeScript,
    codemod: "ts_find_references_function.js",
    fixture_dir: "typescript/find_references_function",
    target: "input.ts",
}

jssg_test! {
    name: test_cross_file_definition_workspace_scope,
    language: SupportLang::TypeScript,
    codemod: "ts_cross_file_definition.js",
    fixture_dir: "typescript/cross_file_definition",
    target: "main.ts",
    scope: workspace,
}

jssg_test! {
    name: test_cross_file_references_workspace_scope,
    language: SupportLang::TypeScript,
    codemod: "ts_cross_file_references.js",
    fixture_dir: "typescript/cross_file_references",
    target: "utils.ts",
    scope: workspace,
}

jssg_test! {
    name: test_find_references_cross_file_with_cache,
    language: SupportLang::TypeScript,
    codemod: "ts_cross_file_references_with_cache.js",
    fixture_dir: "typescript/cross_file_references_with_cache",
    target: "utils.ts",
    preprocess: ["main.ts"],
    scope: workspace,
}
