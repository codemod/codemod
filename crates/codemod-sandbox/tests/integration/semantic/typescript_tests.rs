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

/// `SgRoot.write()` on a definition root writes to disk by default. With
/// `stage_writes` the edit comes back as a secondary change and disk is
/// untouched, which is what a host that stages and commits edits itself needs.
mod staged_writes {
    use super::super::fixtures::{
        create_js_provider, load_codemod, setup_test_workspace, ProviderScope,
    };
    use ast_grep_language::SupportLang;
    use codemod_sandbox::sandbox::engine::execution_engine::{
        execute_codemod_with_quickjs, ExecutionResult, JssgExecutionOptions,
    };
    use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
    use codemod_sandbox::CodemodLang;
    use std::sync::Arc;

    async fn run(stage_writes: bool) -> (tempfile::TempDir, Vec<(String, ExecutionResult)>) {
        let (temp_dir, files) = setup_test_workspace("typescript/cross_file_definition");
        let codemod_path = temp_dir.path().join("codemod.js");
        std::fs::write(&codemod_path, load_codemod("ts_cross_file_write.js")).expect("codemod");
        let main = files.get("main.ts").expect("main.ts").clone();
        let content = std::fs::read_to_string(&main).expect("main content");
        let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
        let provider = create_js_provider(ProviderScope::Workspace, Some(temp_dir.path()));

        let output = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: CodemodLang::Static(SupportLang::TypeScript),
            file_path: &main,
            content: &content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: Some(provider),
            metrics_context: None,
            llm_request_handler: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            stage_writes,
            target_directory: temp_dir.path(),
        })
        .await
        .expect("transform runs");
        let secondary = output
            .secondary
            .into_iter()
            .map(|change| (change.path.to_string_lossy().into_owned(), change.result))
            .collect();
        (temp_dir, secondary)
    }

    #[tokio::test]
    async fn direct_writes_hit_disk_and_produce_no_secondary_change() {
        let (temp_dir, secondary) = run(false).await;
        assert!(secondary.is_empty(), "{secondary:?}");
        let utils = std::fs::read_to_string(temp_dir.path().join("utils.ts")).expect("utils");
        assert!(utils.contains("function sum"), "{utils}");
    }

    #[tokio::test]
    async fn staged_writes_return_the_edit_and_leave_disk_untouched() {
        let (temp_dir, secondary) = run(true).await;
        let utils = std::fs::read_to_string(temp_dir.path().join("utils.ts")).expect("utils");
        assert!(
            utils.contains("function add"),
            "disk must not change: {utils}"
        );
        assert_eq!(secondary.len(), 1, "{secondary:?}");
        let (path, result) = &secondary[0];
        assert!(path.ends_with("utils.ts"), "{path}");
        match result {
            ExecutionResult::Modified(modified) => {
                assert!(
                    modified.content.contains("function sum"),
                    "{}",
                    modified.content
                );
                assert_eq!(modified.rename_to, None);
            }
            other => panic!("expected a modified secondary result, got {other:?}"),
        }
    }
}
