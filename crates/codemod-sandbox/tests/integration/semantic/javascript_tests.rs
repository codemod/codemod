//! JavaScript semantic analysis integration tests.

use ast_grep_language::SupportLang;
use codemod_sandbox::CodemodLang;

jssg_test! {
    name: test_find_references_same_file_variable,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_find_references_variable.js",
    fixture_dir: "javascript/find_references_variable",
    target: "input.js",
}

jssg_test! {
    name: test_find_references_transform_rename,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_transform_rename.js",
    fixture_dir: "javascript/transform_rename",
    target: "input.js",
    expected: "expected.js",
}

jssg_test! {
    name: test_definition_kind_local,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_definition_kind_local.js",
    fixture_dir: "javascript/definition_kind_local",
    target: "input.js",
    preprocess: ["input.js"],
}

// Data flow and call graph. Each codemod answers one question a codemod
// author would ask, and throws when the answer is not the expected one.

jssg_test! {
    name: test_flow_where_does_this_argument_come_from,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_flow_argument_source.js",
    fixture_dir: "javascript/dataflow",
    target: "server.js",
}

jssg_test! {
    name: test_flow_calls_receiving_a_value_derived_from_req_body,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_flow_tainted_calls.js",
    fixture_dir: "javascript/dataflow",
    target: "server.js",
}

jssg_test! {
    name: test_call_graph_functions_transitively_called,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_callees_transitive.js",
    fixture_dir: "javascript/dataflow",
    target: "server.js",
    scope: workspace,
}

jssg_test! {
    name: test_mutations_is_this_variable_mutated_after,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_mutations_after.js",
    fixture_dir: "javascript/dataflow",
    target: "cart.js",
}

jssg_test! {
    name: test_flow_what_flows_into_this_jsx_prop,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_flow_jsx_prop.js",
    fixture_dir: "javascript/dataflow",
    target: "view.jsx",
}

jssg_test! {
    name: test_dataflow_mistakes_throw_with_a_reason,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_dataflow_errors.js",
    fixture_dir: "javascript/dataflow",
    target: "cart.js",
}

// Realistic codemods built on data flow and the call graph. They are the
// yardstick for the API's ergonomics: each rewrites its fixture and the
// result must match the expected directory.

jssg_test! {
    name: test_codemod_moment_to_immutable_dates,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "codemod_moment_immutable.js",
    fixture_dir: "javascript/codemods/moment_immutable/input",
    target: "schedule.js",
    scope: workspace,
    expected_dir: "javascript/codemods/moment_immutable/expected",
}

jssg_test! {
    name: test_codemod_sql_taint_to_parameterized_queries,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "codemod_sql_taint.js",
    fixture_dir: "javascript/codemods/sql_taint/input",
    target: "routes.js",
    scope: workspace,
    expected_dir: "javascript/codemods/sql_taint/expected",
}

jssg_test! {
    name: test_codemod_safe_delete_uncalled_exports,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "codemod_safe_delete.js",
    fixture_dir: "javascript/codemods/safe_delete/input",
    target: "utils.js",
    scope: workspace,
    expected_dir: "javascript/codemods/safe_delete/expected",
}

jssg_test! {
    name: test_codemod_sync_to_async_propagation,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "codemod_async_propagation.js",
    fixture_dir: "javascript/codemods/async_propagation/input",
    target: "config.js",
    scope: workspace,
    expected_dir: "javascript/codemods/async_propagation/expected",
}

jssg_test! {
    name: test_flow_destructuring_assignment_and_class_fields,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_flow_destructuring_and_fields.js",
    fixture_dir: "javascript/dataflow",
    target: "patterns.js",
}

jssg_test! {
    name: test_call_graph_stop_at,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_call_graph_stop_at.js",
    fixture_dir: "javascript/dataflow",
    target: "server.js",
    scope: workspace,
}

jssg_test! {
    name: test_call_graph_commonjs,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_callees_commonjs.js",
    fixture_dir: "javascript/commonjs",
    target: "main.js",
    scope: workspace,
}

jssg_test! {
    name: test_definition_resolve_external_false,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_definition_resolve_external.js",
    fixture_dir: "javascript/definition_resolve_external",
    target: "input.js",
    preprocess: ["input.js"],
}
