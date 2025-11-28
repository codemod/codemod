//! Common semantic provider tests that apply to all languages.
//! These tests verify behavior when no semantic provider is configured.

use super::fixtures::jssg_test;
use ast_grep_language::SupportLang;

jssg_test! {
    name: test_get_definition_without_provider,
    language: SupportLang::JavaScript,
    codemod: "no_provider_definition.js",
    fixture_dir: "common/without_provider",
    target: "input.js",
    no_provider: true,
}

jssg_test! {
    name: test_find_references_without_provider,
    language: SupportLang::JavaScript,
    codemod: "no_provider_references.js",
    fixture_dir: "common/without_provider",
    target: "input.js",
    no_provider: true,
}

jssg_test! {
    name: test_type_info_without_provider,
    language: SupportLang::JavaScript,
    codemod: "no_provider_type_info.js",
    fixture_dir: "common/without_provider",
    target: "input.js",
    no_provider: true,
}
