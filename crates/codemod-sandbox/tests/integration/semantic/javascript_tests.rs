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
    name: test_definition_resolve_external_false,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_definition_resolve_external.js",
    fixture_dir: "javascript/definition_resolve_external",
    target: "input.js",
    preprocess: ["input.js"],
}
