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

// Module top level and JSX elements in the call graph. Fixtures come from the
// debarrel and i18n reviews (gap D, visibility).

jssg_test! {
    name: test_callees_module_top_level_side_effects,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_callees_module_top_level.js",
    fixture_dir: "typescript/side_effect_barrel",
    target: "src/icons/register.ts",
    scope: workspace,
}

jssg_test! {
    name: test_callees_module_top_level_calls_imported_function,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_callees_module_top_level.js",
    fixture_dir: "typescript/side_effect_barrel",
    target: "src/App.ts",
    scope: workspace,
}

jssg_test! {
    name: test_callees_module_that_only_declares,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_callees_module_top_level.js",
    fixture_dir: "typescript/side_effect_barrel",
    target: "src/icons/Icon.ts",
    scope: workspace,
}

jssg_test! {
    name: test_callees_jsx_element_is_its_component,
    language: CodemodLang::Static(SupportLang::Tsx),
    codemod: "tsx_callees_jsx_element.js",
    fixture_dir: "typescript/jsx_callees",
    target: "src/Toolbar.tsx",
    scope: workspace,
}

jssg_test! {
    name: test_flow_builtin_models_promise_and_array_callbacks,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_flow_builtin_models.js",
    fixture_dir: "typescript/builtin_models",
    target: "src/models.ts",
    scope: workspace,
}

jssg_test! {
    name: test_flow_dynamic_import_then,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_flow_builtin_models.js",
    fixture_dir: "typescript/builtin_models",
    target: "src/consumer.ts",
    scope: workspace,
}

jssg_test! {
    name: test_flow_stop_at_first_sink,
    language: CodemodLang::Static(SupportLang::Tsx),
    codemod: "tsx_flow_stop_at.js",
    fixture_dir: "typescript/jsx_callees",
    target: "src/Toolbar.tsx",
}

jssg_test! {
    name: test_workspace_package_without_install,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_workspace_package_no_install.js",
    fixture_dir: "typescript/workspace_package_no_install",
    target: "packages/app/src/App.ts",
    scope: workspace,
}

jssg_test! {
    name: test_callees_overloads_resolve_to_implementation,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_callees_overloads.js",
    fixture_dir: "typescript/overloads",
    target: "main.ts",
    scope: workspace,
}

jssg_test! {
    name: test_callees_through_returned_values,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_callees_through_returns.js",
    fixture_dir: "typescript/returns",
    target: "src/app.ts",
    scope: workspace,
}

jssg_test! {
    name: test_callees_jsx_member_tag,
    language: CodemodLang::Static(SupportLang::Tsx),
    codemod: "tsx_callees_jsx_element.js",
    fixture_dir: "typescript/jsx_callees",
    target: "src/Nav.tsx",
    scope: workspace,
}

jssg_test! {
    name: test_resolve_module_resolve_export_and_specifier_for,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_module_resolution.js",
    fixture_dir: "typescript/module_resolution",
    target: "src/pages/home.ts",
    scope: workspace,
}

jssg_test! {
    name: test_has_top_level_effects,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_has_top_level_effects.js",
    fixture_dir: "typescript/side_effect_barrel",
    target: "src/App.ts",
    scope: workspace,
}

jssg_test! {
    name: test_specifier_for_respects_package_exports,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "ts_specifier_package_boundary.js",
    fixture_dir: "typescript/package_boundary",
    target: "packages/app/src/App.ts",
    scope: workspace,
}
