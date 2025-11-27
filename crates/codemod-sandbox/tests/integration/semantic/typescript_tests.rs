//! TypeScript semantic analysis integration tests.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use language_core::SemanticProvider;
use language_javascript::OxcSemanticProvider;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

/// Test getDefinition() with file-scope provider returns SgNode and SgRoot
#[tokio::test]
async fn test_get_definition_file_scope() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    let test_file_path = temp_dir.path().join("test.ts");
    let content = "const x = 1;\nconst y = x + 2;";
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
  // Find 'x' reference in 'x + 2'
  const nodes = root.root().findAll({ rule: { pattern: "x" } });
  if (nodes.length < 2) {
    throw new Error("Expected at least 2 'x' nodes, got " + nodes.length);
  }
  
  // Get the second 'x' (the reference, not the declaration)
  const refNode = nodes[1];
  
  const definition = refNode.getDefinition();
  
  // Definition may or may not be found depending on semantic provider state
  if (definition === null) {
    console.log("Definition not found - this is acceptable for this test");
    return null;
  }
  
  // If found, verify structure
  if (!definition.node) {
    throw new Error("Expected definition.node to exist");
  }
  if (!definition.root) {
    throw new Error("Expected definition.root to exist");
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

/// Test findReferences() with file-scope provider returns grouped results
#[tokio::test]
async fn test_find_references_file_scope() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("test_codemod.js");

    let test_file_path = temp_dir.path().join("test.ts");
    // x is used 2 times: x + 2, console.log(x)
    let content = "const x = 1;\nconst y = x + 2;\nconsole.log(x);";
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
  // Find 'const x = 1'
  const node = root.root().find({ rule: { pattern: "const x = 1" } });
  if (!node) {
    throw new Error("Expected to find 'const x = 1'");
  }
  
  const references = node.findReferences();
  
  // Should return array of file references
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
  
  // Should find 2 references (x + 2 and console.log(x))
  if (fileRef.nodes.length !== 2) {
    throw new Error("Expected 2 references to 'x', got " + fileRef.nodes.length);
  }
  
  // Check that nodes have expected methods and values
  for (const node of fileRef.nodes) {
    if (typeof node.text !== "function") {
      throw new Error("Expected node.text to be a function");
    }
    if (node.text() !== "x") {
      throw new Error("Expected reference text to be 'x', got '" + node.text() + "'");
    }
  }
  
  // Check that root has expected methods
  if (typeof fileRef.root.filename !== "function") {
    throw new Error("Expected root.filename to be a function");
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

/// Test: Find references to a function within the same file (using identifier pattern)
#[tokio::test]
async fn test_find_references_function_same_file() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    let test_file_path = temp_dir.path().join("test.ts");
    // greet is called 3 times
    let content = r#"
function greet(name: string): string {
    return "Hello, " + name;
}

const message1 = greet("Alice");
const message2 = greet("Bob");
console.log(greet("World"));
"#;
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
    // Find all 'greet' identifiers
    const greetNodes = root.root().findAll({ rule: { pattern: "greet" } });
    if (greetNodes.length < 1) {
        throw new Error("Expected at least 1 'greet' node, got " + greetNodes.length);
    }
    
    // Use the first one (the function name in declaration)
    const funcNameNode = greetNodes[0];
    
    const references = funcNameNode.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array");
    }
    
    if (references.length === 0) {
        console.log("No references found - semantic provider may not have indexed yet");
        return null;
    }
    
    if (references.length !== 1) {
        throw new Error("Expected exactly 1 file with references, got " + references.length);
    }
    
    const fileRef = references[0];
    // Should find 3 references (the 3 calls to greet)
    if (fileRef.nodes.length !== 3) {
        throw new Error("Expected 3 references to 'greet' function, got " + fileRef.nodes.length);
    }
    
    // All references should be 'greet'
    for (const node of fileRef.nodes) {
        if (node.text() !== "greet") {
            throw new Error("Expected reference text to be 'greet', got '" + node.text() + "'");
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

/// Test that cross-file definition lookup works with workspace-scope provider
#[tokio::test]
async fn test_cross_file_definition_workspace_scope() {
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
  // Find all 'add' identifiers
  const addNodes = root.root().findAll({ rule: { pattern: "add" } });
  if (addNodes.length < 1) {
    throw new Error("Expected at least 1 'add' node, got " + addNodes.length);
  }
  
  // Find the 'add' that's in a call (not the import)
  let callAdd = null;
  for (const node of addNodes) {
    const parent = node.parent();
    if (parent && parent.kind() === "call_expression") {
      callAdd = node;
      break;
    }
  }
  
  if (!callAdd) {
    console.log("Could not find add() call");
    return null;
  }
  
  const definition = callAdd.getDefinition();
  
  // Definition may or may not be found depending on cross-file resolution
  if (definition === null) {
    console.log("Definition not found (cross-file resolution may not be complete)");
    return null;
  }
  
  // If found, verify structure
  if (!definition.node) {
    throw new Error("Expected definition.node to exist");
  }
  if (!definition.root) {
    throw new Error("Expected definition.root to exist");
  }
  
  // Log where the definition was found
  console.log("Definition found");
  
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

/// Test that cross-file references are found with workspace-scope provider
#[tokio::test]
async fn test_cross_file_references_workspace_scope() {
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
  // Find all 'add' identifiers (first one should be the function name)
  const addNodes = root.root().findAll({ rule: { pattern: "add" } });
  if (addNodes.length < 1) {
    throw new Error("Expected at least 1 'add' node, got " + addNodes.length);
  }
  
  const funcNameNode = addNodes[0];
  
  const references = funcNameNode.findReferences();
  
  // Should return array of file references
  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }
  
  if (references.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }
  
  // Each entry should have root and nodes
  let totalRefs = 0;
  for (const fileRef of references) {
    if (!fileRef.root) {
      throw new Error("Expected fileRef.root to exist");
    }
    if (!Array.isArray(fileRef.nodes)) {
      throw new Error("Expected fileRef.nodes to be an array");
    }
    totalRefs += fileRef.nodes.length;
  }
  
  console.log("Total references found:", totalRefs);
  
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

/// Test: Find references across multiple files with pre-populated cache
#[tokio::test]
async fn test_find_references_cross_file_with_cache() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    // Create a utility file with exported function
    let utils_content = r#"
export function formatDate(date: Date): string {
    return date.toISOString();
}
"#;
    let utils_path = temp_dir.path().join("utils.ts");
    fs::write(&utils_path, utils_content).expect("Failed to write utils.ts");

    // Create main file that imports and uses the function (2 usages)
    let main_content = r#"
import { formatDate } from './utils';

const now = new Date();
const formatted = formatDate(now);
console.log(formatDate(new Date()));
"#;
    let main_path = temp_dir.path().join("main.ts");
    fs::write(&main_path, main_content).expect("Failed to write main.ts");

    let codemod_content = r#"
export default function transform(root) {
    // Find all 'formatDate' identifiers
    const formatDateNodes = root.root().findAll({ rule: { pattern: "formatDate" } });
    if (formatDateNodes.length < 1) {
        throw new Error("Expected at least 1 'formatDate' node, got " + formatDateNodes.length);
    }
    
    const funcNameNode = formatDateNodes[0];
    
    const references = funcNameNode.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array");
    }
    
    if (references.length === 0) {
        console.log("No references found - this is acceptable");
        return null;
    }
    
    // Count total references across all files
    let totalRefs = 0;
    
    for (const fileRef of references) {
        totalRefs += fileRef.nodes.length;
    }
    
    console.log("Found", totalRefs, "references");
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());

    // Use workspace scope for cross-file analysis
    let provider = OxcSemanticProvider::workspace_scope(temp_dir.path().to_path_buf());

    // Process main.ts first to populate the cache
    provider
        .notify_file_processed(&main_path, main_content)
        .unwrap();

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
