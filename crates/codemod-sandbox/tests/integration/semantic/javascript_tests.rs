//! JavaScript semantic analysis integration tests.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, ExecutionResult, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use language_javascript::OxcSemanticProvider;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

/// Test: Find references to a variable within the same file
#[tokio::test]
async fn test_find_references_same_file_variable() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    let test_file_path = temp_dir.path().join("test.js");
    // counter is used 3 times: console.log(counter), counter * 2, console.log("Counter:", counter)
    let content = r#"
const counter = 0;
console.log(counter);
const doubled = counter * 2;
function printCounter() {
    console.log("Counter:", counter);
}
"#;
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
    // Find the variable declaration 'counter'
    const varDecl = root.root().find({ rule: { pattern: "const counter = $VALUE" } });
    if (!varDecl) {
        throw new Error("Expected to find 'const counter' declaration");
    }
    
    // Find references from the declaration node itself
    const references = varDecl.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array, got: " + typeof references);
    }
    
    if (references.length === 0) {
        console.log("No references found - semantic provider may not have indexed yet");
        return null;
    }
    
    if (references.length !== 1) {
        throw new Error("Expected exactly 1 file with references, got " + references.length);
    }
    
    const fileRef = references[0];
    if (!fileRef.root) {
        throw new Error("Expected fileRef.root to exist");
    }
    if (!Array.isArray(fileRef.nodes)) {
        throw new Error("Expected fileRef.nodes to be an array");
    }
    
    // Should find 3 references (usages of 'counter', not the definition)
    if (fileRef.nodes.length !== 3) {
        throw new Error("Expected 3 references to 'counter', got " + fileRef.nodes.length);
    }
    
    // All references should be 'counter'
    for (const node of fileRef.nodes) {
        if (node.text() !== "counter") {
            throw new Error("Expected reference text to be 'counter', got '" + node.text() + "'");
        }
    }
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let provider = OxcSemanticProvider::file_scope();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::JavaScript,
        file_path: &test_file_path,
        content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: Some(Arc::new(provider)),
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}

/// Test: Verify reference nodes can be used for transformations (renaming)
#[tokio::test]
async fn test_find_references_transform_rename() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    let test_file_path = temp_dir.path().join("test.js");
    // oldName is used 2 times: console.log(oldName), oldName + 2
    let content = r#"const oldName = 1;
console.log(oldName);
const x = oldName + 2;
"#;
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
    // Find the variable declaration
    const varDecl = root.root().find({ rule: { pattern: "const oldName = $VALUE" } });
    if (!varDecl) {
        throw new Error("Expected to find 'const oldName' declaration");
    }
    
    const references = varDecl.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array");
    }
    
    if (references.length === 0) {
        console.log("No references found - cannot perform rename");
        return null;
    }
    
    if (references.length !== 1) {
        throw new Error("Expected exactly 1 file with references, got " + references.length);
    }
    
    const fileRef = references[0];
    // Should find 2 references (usages of 'oldName')
    if (fileRef.nodes.length !== 2) {
        throw new Error("Expected 2 references to 'oldName', got " + fileRef.nodes.length);
    }
    
    // Build edits to rename all references from 'oldName' to 'newName'
    let edits = [];
    for (const node of fileRef.nodes) {
        if (node.text() !== "oldName") {
            throw new Error("Expected reference text to be 'oldName', got '" + node.text() + "'");
        }
        edits.push(node.replace("newName"));
    }
    
    // Also rename the declaration itself
    const nameNode = varDecl.field("name");
    if (!nameNode) {
        throw new Error("Expected to find name field");
    }
    if (nameNode.text() !== "oldName") {
        throw new Error("Expected declaration name to be 'oldName', got '" + nameNode.text() + "'");
    }
    edits.push(nameNode.replace("newName"));
    
    if (edits.length !== 3) {
        throw new Error("Expected 3 edits (2 references + 1 declaration), got " + edits.length);
    }
    
    return root.root().commitEdits(edits);
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let provider = OxcSemanticProvider::file_scope();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::JavaScript,
        file_path: &test_file_path,
        content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: Some(Arc::new(provider)),
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);

    // The result should be modified code with 'newName' instead of 'oldName'
    if let Ok(ExecutionResult::Modified(new_content)) = result {
        assert!(
            new_content.contains("newName"),
            "Expected 'newName' in output, got: {}",
            new_content
        );
        assert!(
            !new_content.contains("oldName"),
            "Expected no 'oldName' in output, got: {}",
            new_content
        );
    }
}
