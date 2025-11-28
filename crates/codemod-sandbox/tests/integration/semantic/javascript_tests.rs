//! JavaScript semantic analysis integration tests.

use super::fixtures::jssg_test;
use ast_grep_language::SupportLang;

jssg_test! {
    name: test_find_references_same_file_variable,
    language: SupportLang::JavaScript,
    codemod: "js_find_references_variable.js",
    fixture_dir: "javascript/find_references_variable",
    target: "input.js",
}

jssg_test! {
    name: test_find_references_transform_rename,
    language: SupportLang::JavaScript,
    codemod: "js_transform_rename.js",
    fixture_dir: "javascript/transform_rename",
    target: "input.js",
    expected: "expected.js",
}

jssg_test! {
    name: test_definition_kind_local,
    language: SupportLang::JavaScript,
    codemod: "js_definition_kind_local.js",
    fixture_dir: "javascript/definition_kind_local",
    target: "input.js",
    preprocess: ["input.js"],
}

jssg_test! {
    name: test_definition_resolve_external_false,
    language: SupportLang::JavaScript,
    codemod: "js_definition_resolve_external.js",
    fixture_dir: "javascript/definition_resolve_external",
    target: "input.js",
    preprocess: ["input.js"],
}
