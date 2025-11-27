//! Common semantic provider tests that apply to all languages.
//! These tests verify behavior when no semantic provider is configured.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

/// Test that getDefinition() returns null when no semantic provider is configured
#[tokio::test]
async fn test_get_definition_without_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    let codemod_content = r#"
export default function transform(root) {
  const node = root.root().find({ rule: { pattern: "x" } });
  if (!node) {
    throw new Error("Expected to find 'x' node");
  }
  
  const definition = node.getDefinition();
  
  // Should return null when no provider is configured
  if (definition !== null) {
    throw new Error("Expected null when no semantic provider is configured, got: " + JSON.stringify(definition));
  }
  
  return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let file_path = Path::new("test.js");
    let content = "const x = 1;\nconst y = x + 2;";

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::JavaScript,
        file_path,
        content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: None,
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}

/// Test that findReferences() returns empty array when no semantic provider is configured
#[tokio::test]
async fn test_find_references_without_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    let codemod_content = r#"
export default function transform(root) {
  const node = root.root().find({ rule: { pattern: "x" } });
  if (!node) {
    throw new Error("Expected to find 'x' node");
  }
  
  const references = node.findReferences();
  
  // Should return empty array when no provider is configured
  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array, got: " + typeof references);
  }
  
  if (references.length !== 0) {
    throw new Error("Expected empty array when no semantic provider is configured, got length: " + references.length);
  }
  
  return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let file_path = Path::new("test.js");
    let content = "const x = 1;\nconst y = x + 2;";

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::JavaScript,
        file_path,
        content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: None,
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}

/// Test that getType() returns null when no semantic provider is configured
#[tokio::test]
async fn test_get_type_without_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    let codemod_content = r#"
export default function transform(root) {
  const node = root.root().find({ rule: { pattern: "x" } });
  if (!node) {
    throw new Error("Expected to find 'x' node");
  }
  
  const typeInfo = node.getType();
  
  // Should return null when no provider is configured
  if (typeInfo !== null) {
    throw new Error("Expected null when no semantic provider is configured, got: " + typeInfo);
  }
  
  return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let file_path = Path::new("test.js");
    let content = "const x = 1;\nconst y = x + 2;";

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::JavaScript,
        file_path,
        content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: None,
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}
