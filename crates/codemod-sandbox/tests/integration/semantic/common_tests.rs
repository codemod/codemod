//! Semantic queries with no semantic provider configured.

jssg_test! {
    name: without_provider,
    fixture: "common/without_provider",
    codemod: "no_provider.js",
    targets: ["input.js"],
    scope: none,
}
