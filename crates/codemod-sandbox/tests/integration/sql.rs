use ast_grep_core::{AstGrep, Language};
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, ExecutionResult, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use codemod_sandbox::{scan_file_with_combined_scan, with_combined_scan, CodemodLang};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires downloaded SQL parser artifacts or CODEMOD_PARSER_CACHE_DIR"]
async fn sql_table_rename_preserves_non_table_identifiers_and_is_idempotent() {
    let language: CodemodLang = "sql".parse().unwrap();
    assert_eq!(CodemodLang::from_path("query.sql"), Some(language));

    let ast = AstGrep::new("SELECT id FROM legacy_users;", language);
    let matched = ast.root().find("SELECT $COLUMN FROM $TABLE").unwrap();
    assert_eq!(
        matched.get_env().get_match("TABLE").unwrap().text(),
        "legacy_users"
    );

    let dir = tempfile::tempdir().unwrap();
    let script_path = dir.path().join("codemod.ts");
    let file_path = dir.path().join("query.sql");
    std::fs::write(
        &script_path,
        include_str!("../../../cli/src/templates/js-astgrep/scripts/codemod.sql.ts"),
    )
    .unwrap();
    let input = include_str!("../../../cli/src/templates/js-astgrep/tests/fixtures/input.sql");
    let expected =
        include_str!("../../../cli/src/templates/js-astgrep/tests/fixtures/expected.sql");
    let rules_path = dir.path().join("config.yml");
    std::fs::write(
        &rules_path,
        include_str!("../../../cli/src/templates/astgrep-yaml/rules/config.sql.yml"),
    )
    .unwrap();
    std::fs::write(&file_path, input).unwrap();
    let (_, modified, fixed) = with_combined_scan(rules_path.to_str().unwrap(), |scan| {
        Ok(scan_file_with_combined_scan(
            &file_path,
            &scan.combined_scan,
            true,
        )?)
    })
    .unwrap();
    assert!(modified);
    assert_eq!(fixed.unwrap(), expected);
    let resolver = Arc::new(OxcResolver::new(dir.path().to_path_buf(), None).unwrap());

    for content in [input, expected] {
        std::fs::write(&file_path, content).unwrap();
        let output = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &script_path,
            resolver: resolver.clone(),
            language,
            file_path: &file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            llm_request_handler: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: dir.path(),
        })
        .await
        .unwrap();

        if content == input {
            let ExecutionResult::Modified(modified) = output.primary else {
                panic!("SQL table references should be renamed");
            };
            assert_eq!(modified.content, expected);
        } else {
            assert!(matches!(output.primary, ExecutionResult::Unmodified));
        }
    }
}
