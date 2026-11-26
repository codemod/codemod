//! JavaScript semantic analysis: one test per fixture.

jssg_test! {
    name: symbols_in_one_file,
    fixture: "javascript/symbols",
    codemod: "js_symbols.js",
    targets: ["input.js"],
    scope: file,
}

jssg_test! {
    name: dataflow_and_call_graph,
    fixture: "javascript/dataflow",
    codemod: "js_dataflow.js",
    targets: ["server.js", "cart.js", "view.jsx", "patterns.js"],
    scope: workspace,
}

jssg_test! {
    name: namespace_member_uses,
    fixture: "javascript/namespace_uses",
    codemod: "js_namespace_uses.js",
    targets: ["utils.js"],
    scope: workspace,
}

jssg_test! {
    name: class_inheritance,
    fixture: "javascript/inheritance",
    codemod: "js_inheritance.js",
    targets: ["main.js"],
    scope: workspace,
}

jssg_test! {
    name: commonjs,
    fixture: "javascript/commonjs",
    codemod: "js_commonjs.js",
    targets: ["main.js"],
    scope: workspace,
}

jssg_test! {
    name: importers,
    fixture: "javascript/importers",
    codemod: "js_importers.js",
    targets: ["src/lib/index.js"],
    scope: workspace,
}

// Transforms: the codemod's output must match the expected file or
// directory. The realistic codemods are the yardstick for the API's
// ergonomics.

jssg_test! {
    name: transform_rename_through_references,
    fixture: "javascript/transform_rename",
    codemod: "js_transform_rename.js",
    target: "input.js",
    scope: file,
    expected: "expected.js",
}

jssg_test! {
    name: codemod_moment_to_immutable_dates,
    fixture: "javascript/codemods/moment_immutable/input",
    codemod: "codemod_moment_immutable.js",
    target: "schedule.js",
    scope: workspace,
    expected_dir: "javascript/codemods/moment_immutable/expected",
}

jssg_test! {
    name: codemod_sql_taint_to_parameterized_queries,
    fixture: "javascript/codemods/sql_taint/input",
    codemod: "codemod_sql_taint.js",
    target: "routes.js",
    scope: workspace,
    expected_dir: "javascript/codemods/sql_taint/expected",
}

jssg_test! {
    name: codemod_safe_delete_uncalled_exports,
    fixture: "javascript/codemods/safe_delete/input",
    codemod: "codemod_safe_delete.js",
    target: "utils.js",
    scope: workspace,
    expected_dir: "javascript/codemods/safe_delete/expected",
}

jssg_test! {
    name: codemod_sync_to_async_propagation,
    fixture: "javascript/codemods/async_propagation/input",
    codemod: "codemod_async_propagation.js",
    target: "config.js",
    scope: workspace,
    expected_dir: "javascript/codemods/async_propagation/expected",
}
