//! TypeScript semantic analysis integration tests.

use ast_grep_language::SupportLang;
use codemod_sandbox::CodemodLang;

jssg_test! {
    name: test_get_definition_file_scope,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_definition_file_scope.js",
    fixture_dir: "typescript/definition_file_scope",
    target: "input.ts",
}

jssg_test! {
    name: test_find_references_file_scope,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_find_references_file_scope.js",
    fixture_dir: "typescript/find_references_file_scope",
    target: "input.ts",
}

jssg_test! {
    name: test_find_references_function_same_file,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_find_references_function.js",
    fixture_dir: "typescript/find_references_function",
    target: "input.ts",
}

jssg_test! {
    name: test_cross_file_definition_workspace_scope,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_cross_file_definition.js",
    fixture_dir: "typescript/cross_file_definition",
    target: "main.ts",
    scope: workspace,
}

jssg_test! {
    name: test_cross_file_references_workspace_scope,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_cross_file_references.js",
    fixture_dir: "typescript/cross_file_references",
    target: "utils.ts",
    scope: workspace,
}

jssg_test! {
    name: test_find_references_cross_file_with_cache,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_cross_file_references_with_cache.js",
    fixture_dir: "typescript/cross_file_references_with_cache",
    target: "utils.ts",
    preprocess: ["main.ts"],
    scope: workspace,
}

jssg_test! {
    name: test_reexport_import_definition_follows_barrel,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_reexport_import_definition.js",
    fixture_dir: "typescript/reexport_definition",
    target: "packages/web/src/app.ts",
    scope: workspace,
}

jssg_test! {
    name: test_reexport_specifier_definition,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_reexport_specifier_definition.js",
    fixture_dir: "typescript/reexport_definition",
    target: "packages/shared/src/index.ts",
    scope: workspace,
}

jssg_test! {
    name: test_reexport_references_through_barrel,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_reexport_references.js",
    fixture_dir: "typescript/reexport_definition",
    target: "packages/shared/src/dates.ts",
    scope: workspace,
}

jssg_test! {
    name: test_definition_export_name_name_node_and_follow,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_definition_export_name.js",
    fixture_dir: "typescript/definition_exports",
    target: "app.ts",
    scope: workspace,
}
