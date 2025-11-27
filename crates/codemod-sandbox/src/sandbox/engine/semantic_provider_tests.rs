use crate::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, JssgExecutionOptions,
};
use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
use ast_grep_language::SupportLang;
use language_javascript::OxcSemanticProvider;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

/// Test that getDefinition() returns null when no semantic provider is configured
#[tokio::test]
async fn test_get_definition_without_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    // Codemod that calls getDefinition()
    let codemod_content = r#"
export default function transform(root) {
  const node = root.root().find({ pattern: "x" });
  if (!node) return null;
  
  const definition = node.getDefinition();
  
  // Should return null when no provider is configured
  if (definition !== null) {
    throw new Error("Expected null, got: " + JSON.stringify(definition));
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
        semantic_provider: None, // No provider
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
  const node = root.root().find({ pattern: "x" });
  if (!node) return null;
  
  const references = node.findReferences();
  
  // Should return empty array when no provider is configured
  if (!Array.isArray(references)) {
    throw new Error("Expected array, got: " + typeof references);
  }
  
  if (references.length !== 0) {
    throw new Error("Expected empty array, got length: " + references.length);
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
        semantic_provider: None, // No provider
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
  const node = root.root().find({ pattern: "x" });
  if (!node) return null;
  
  const typeInfo = node.getType();
  
  // Should return null when no provider is configured
  if (typeInfo !== null) {
    throw new Error("Expected null, got: " + typeInfo);
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
        semantic_provider: None, // No provider
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}

/// Test getDefinition() with lightweight provider returns SgNode and SgRoot
#[tokio::test]
async fn test_get_definition_with_lightweight_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    // Create a test file
    let test_file_path = temp_dir.path().join("test.ts");
    let content = "const x = 1;\nconst y = x + 2;";
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
  // Find 'x' in 'const x'
  const node = root.root().find({ pattern: "const x = 1" });
  if (!node) {
    throw new Error("Could not find 'const x = 1'");
  }
  
  const definition = node.getDefinition();
  
  // Definition should be found (same file)
  if (definition === null) {
    throw new Error("Expected definition, got null");
  }
  
  // Check that we get node and root
  if (!definition.node) {
    throw new Error("Expected definition.node");
  }
  
  if (!definition.root) {
    throw new Error("Expected definition.root");
  }
  
  // The node should have expected methods
  if (typeof definition.node.text !== "function") {
    throw new Error("Expected node to have text() method");
  }
  
  if (typeof definition.root.filename !== "function") {
    throw new Error("Expected root to have filename() method");
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
        language: SupportLang::TypeScript,
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

/// Test findReferences() with lightweight provider returns grouped results
#[tokio::test]
async fn test_find_references_with_lightweight_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    let test_file_path = temp_dir.path().join("test.ts");
    let content = "const x = 1;\nconst y = x + 2;\nconsole.log(x);";
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
  // Find 'x' in 'const x'
  const node = root.root().find({ pattern: "const x = 1" });
  if (!node) {
    throw new Error("Could not find 'const x = 1'");
  }
  
  const references = node.findReferences();
  
  // Should return array of file references
  if (!Array.isArray(references)) {
    throw new Error("Expected array, got: " + typeof references);
  }
  
  // Each entry should have root and nodes
  for (const fileRef of references) {
    if (!fileRef.root) {
      throw new Error("Expected fileRef.root");
    }
    
    if (!Array.isArray(fileRef.nodes)) {
      throw new Error("Expected fileRef.nodes to be array");
    }
    
    // Check that nodes have expected methods
    for (const node of fileRef.nodes) {
      if (typeof node.text !== "function") {
        throw new Error("Expected node to have text() method");
      }
    }
    
    // Check that root has expected methods
    if (typeof fileRef.root.filename !== "function") {
      throw new Error("Expected root to have filename() method");
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
        language: SupportLang::TypeScript,
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

/// Test that cross-file definition lookup works with accurate provider
#[tokio::test]
async fn test_cross_file_definition_with_accurate_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    // Create utils.ts with exported function
    let utils_content = r#"
export function add(a: number, b: number): number {
    return a + b;
}
"#;
    fs::write(temp_dir.path().join("utils.ts"), utils_content).expect("Failed to write utils.ts");

    // Create main.ts that imports from utils
    let main_content = r#"
import { add } from './utils';

const result = add(1, 2);
"#;
    let main_path = temp_dir.path().join("main.ts");
    fs::write(&main_path, main_content).expect("Failed to write main.ts");

    let codemod_content = r#"
export default function transform(root) {
  // Find the 'add' call
  const callNode = root.root().find({ pattern: "add($$$)" });
  if (!callNode) {
    throw new Error("Could not find add() call");
  }
  
  const funcIdent = callNode.field("function");
  if (!funcIdent) {
    throw new Error("Could not find function identifier");
  }
  
  const definition = funcIdent.getDefinition();
  
  // Definition should be found (in utils.ts)
  if (definition === null) {
    throw new Error("Expected definition, got null");
  }
  
  // The definition should have node and root
  if (!definition.node) {
    throw new Error("Expected definition.node");
  }
  
  if (!definition.root) {
    throw new Error("Expected definition.root");
  }
  
  // The root should point to a different file (utils.ts)
  const filename = definition.root.filename();
  if (!filename.includes("utils.ts")) {
    throw new Error("Expected definition to be in utils.ts, got: " + filename);
  }
  
  return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());

    let provider = OxcSemanticProvider::workspace_scope(temp_dir.path().to_path_buf());

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::TypeScript,
        file_path: &main_path,
        content: main_content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: Some(Arc::new(provider)),
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}

/// Test that cross-file references are found with accurate provider
#[tokio::test]
async fn test_cross_file_references_with_accurate_provider() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    // Create utils.ts with exported function
    let utils_content = r#"
export function add(a: number, b: number): number {
    return a + b;
}
"#;
    let utils_path = temp_dir.path().join("utils.ts");
    fs::write(&utils_path, utils_content).expect("Failed to write utils.ts");

    // Create main.ts that imports from utils
    let main_content = r#"
import { add } from './utils';

const result = add(1, 2);
"#;
    fs::write(temp_dir.path().join("main.ts"), main_content).expect("Failed to write main.ts");

    let codemod_content = r#"
export default function transform(root) {
  // Find the 'add' function definition
  const funcNode = root.root().find({ pattern: "function add($$$) { $$$ }" });
  if (!funcNode) {
    throw new Error("Could not find add function");
  }
  
  const nameNode = funcNode.field("name");
  if (!nameNode) {
    throw new Error("Could not find function name");
  }
  
  const references = nameNode.findReferences();
  
  // Should return array of file references
  if (!Array.isArray(references)) {
    throw new Error("Expected array, got: " + typeof references);
  }
  
  // Should have references in multiple files
  if (references.length < 1) {
    throw new Error("Expected at least 1 file with references, got: " + references.length);
  }
  
  // Each entry should have root and nodes
  for (const fileRef of references) {
    if (!fileRef.root) {
      throw new Error("Expected fileRef.root");
    }
    
    if (!Array.isArray(fileRef.nodes)) {
      throw new Error("Expected fileRef.nodes to be array");
    }
    
    // Nodes should be valid SgNode objects
    for (const node of fileRef.nodes) {
      if (typeof node.text !== "function") {
        throw new Error("Expected node to have text() method");
      }
    }
  }
  
  return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());

    let provider = OxcSemanticProvider::workspace_scope(temp_dir.path().to_path_buf());

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::TypeScript,
        file_path: &utils_path,
        content: utils_content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: Some(Arc::new(provider)),
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}
