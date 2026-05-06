//! JavaScript semantic analysis integration tests.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, ExecutionResult, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use codemod_sandbox::CodemodLang;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

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

jssg_test! {
    name: test_definition_resolve_external_false,
    language: CodemodLang::Static(SupportLang::JavaScript),
    codemod: "js_definition_resolve_external.js",
    fixture_dir: "javascript/definition_resolve_external",
    target: "input.js",
    preprocess: ["input.js"],
}

#[tokio::test]
async fn test_execute_codemod_with_nested_relative_imports() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_root = temp_dir.path();

    let case_dir = workspace_root.join("cases").join("example");
    let helpers_dir = case_dir.join("helpers");
    let fixtures_dir = case_dir.join("tests").join("fixtures");
    let shared_dir = workspace_root.join("shared");

    fs::create_dir_all(&helpers_dir).unwrap();
    fs::create_dir_all(&fixtures_dir).unwrap();
    fs::create_dir_all(&shared_dir).unwrap();

    fs::write(
        shared_dir.join("marker.js"),
        "export const sharedMarker = 'shared-marker';\n",
    )
    .unwrap();
    fs::write(
        helpers_dir.join("runtime-check.js"),
        "export const localMarker = 'local-marker';\n",
    )
    .unwrap();
    fs::write(
        case_dir.join("codemod.js"),
        r#"
import { localMarker } from "./helpers/runtime-check";
import { sharedMarker } from "../../shared/marker.js";

export default function transform(root) {
  if (localMarker !== "local-marker") {
    throw new Error("failed to resolve sibling helper import");
  }

  if (sharedMarker !== "shared-marker") {
    throw new Error("failed to resolve parent traversal helper import");
  }

  const program = root.root();
  if (!program) {
    throw new Error("expected parsed root");
  }

  return null;
}
"#,
    )
    .unwrap();
    fs::write(fixtures_dir.join("input.js"), "console.log('hello');\n").unwrap();

    let script_path = case_dir.join("codemod.js");
    let target_path = fixtures_dir.join("input.js");
    let content = fs::read_to_string(&target_path).unwrap();
    let resolver = Arc::new(OxcResolver::new(workspace_root.to_path_buf(), None).unwrap());

    let result = execute_codemod_with_quickjs(JssgExecutionOptions {
        script_path: &script_path,
        resolver,
        language: CodemodLang::Static(SupportLang::JavaScript),
        file_path: &target_path,
        content: &content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: None,
        metrics_context: None,
        shared_state_context: None,
        runtime_event_callback: None,
        cancellation_flag: None,
        test_mode: false,
        dry_run: false,
        target_directory: target_path.parent().unwrap(),
    })
    .await;

    match result {
        Ok(output) => {
            assert!(
                matches!(
                    output.primary,
                    ExecutionResult::Unmodified | ExecutionResult::Skipped
                ),
                "Expected codemod to complete without edits, got {:?}",
                output.primary
            );
        }
        Err(err) => panic!("Execution failed: {:?}", err),
    }
}
