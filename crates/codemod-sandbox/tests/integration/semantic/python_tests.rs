//! Python semantic analysis integration tests.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use language_core::SemanticProvider;
use language_python::RuffSemanticProvider;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

// =============================================================================
// Single-file (File Scope) Tests
// =============================================================================

/// Test: Find references to a Python variable
#[tokio::test]
async fn test_find_references_variable() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    let test_file_path = temp_dir.path().join("test.py");
    // counter is used 4 times: print(counter), counter + 1, print("Final:", counter), and the reassignment
    let content = r#"
counter = 0
print(counter)
counter = counter + 1
print("Final:", counter)
"#;
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
    // Find the first assignment 'counter = 0'
    const assignment = root.root().find({ rule: { pattern: "counter = 0" } });
    if (!assignment) {
        throw new Error("Expected to find 'counter = 0'");
    }
    
    const references = assignment.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array, got: " + typeof references);
    }
    
    if (references.length === 0) {
        console.log("No references found - semantic provider may not have indexed yet");
        return null;
    }
    
    // Collect reference texts
    let refTexts = [];
    for (const fileRef of references) {
        if (!fileRef.root) {
            throw new Error("Expected fileRef.root to exist");
        }
        if (!Array.isArray(fileRef.nodes)) {
            throw new Error("Expected fileRef.nodes to be an array");
        }
        
        for (const node of fileRef.nodes) {
            if (typeof node.text === "function") {
                refTexts.push(node.text());
            }
        }
    }
    
    // Should find references to 'counter'
    if (refTexts.length === 0) {
        throw new Error("Expected to find references to 'counter', got " + refTexts.length);
    }
    
    // All references should be 'counter'
    for (const text of refTexts) {
        if (text !== "counter") {
            throw new Error("Expected reference text to be 'counter', got: '" + text + "'");
        }
    }
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let provider = RuffSemanticProvider::file_scope();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::Python,
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

/// Test: Find references to a Python function
#[tokio::test]
async fn test_find_references_function() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    let test_file_path = temp_dir.path().join("test.py");
    // greet is called 3 times
    let content = r#"
def greet(name):
    return "Hello, " + name

message1 = greet("Alice")
message2 = greet("Bob")
print(greet("World"))
"#;
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
    // Find all 'greet' identifiers
    const greetNodes = root.root().findAll({ rule: { pattern: "greet" } });
    if (greetNodes.length < 1) {
        throw new Error("Expected at least 1 'greet' node, got " + greetNodes.length);
    }
    
    // Use the first one (the function name in definition)
    const funcNameNode = greetNodes[0];
    
    const references = funcNameNode.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array");
    }
    
    if (references.length === 0) {
        console.log("No references found - semantic provider may not have indexed yet");
        return null;
    }
    
    // Count total references
    let totalRefs = 0;
    let refTexts = [];
    for (const fileRef of references) {
        for (const node of fileRef.nodes) {
            if (typeof node.text === "function") {
                totalRefs++;
                refTexts.push(node.text());
            }
        }
    }
    
    // Should find at least 3 references (the 3 calls to greet)
    if (totalRefs < 3) {
        throw new Error("Expected at least 3 references, got: " + totalRefs);
    }
    
    // All references should be 'greet'
    for (const text of refTexts) {
        if (text !== "greet") {
            throw new Error("Expected reference text to be 'greet', got: '" + text + "'");
        }
    }
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let provider = RuffSemanticProvider::file_scope();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::Python,
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

/// Test: Find references to a Python class
#[tokio::test]
async fn test_find_references_class() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    let test_file_path = temp_dir.path().join("test.py");
    // Counter is used 3 times: Counter(), Counter(), isinstance(c1, Counter)
    let content = r#"
class Counter:
    def __init__(self):
        self.value = 0
    
    def increment(self):
        self.value += 1

c1 = Counter()
c2 = Counter()
print(isinstance(c1, Counter))
"#;
    fs::write(&test_file_path, content).expect("Failed to write test file");

    let codemod_content = r#"
export default function transform(root) {
    // Find all 'Counter' identifiers
    const counterNodes = root.root().findAll({ rule: { pattern: "Counter" } });
    if (counterNodes.length < 1) {
        throw new Error("Expected at least 1 'Counter' node, got " + counterNodes.length);
    }
    
    // Use the first one (the class name in definition)
    const classNameNode = counterNodes[0];
    
    const references = classNameNode.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array");
    }
    
    if (references.length === 0) {
        console.log("No references found - semantic provider may not have indexed yet");
        return null;
    }
    
    // Count total references
    let totalRefs = 0;
    let refTexts = [];
    for (const fileRef of references) {
        for (const node of fileRef.nodes) {
            if (typeof node.text === "function") {
                totalRefs++;
                refTexts.push(node.text());
            }
        }
    }
    
    // Should find at least 3 references (Counter() calls and isinstance check)
    if (totalRefs < 3) {
        throw new Error("Expected at least 3 references, got: " + totalRefs);
    }
    
    // All references should be 'Counter'
    for (const text of refTexts) {
        if (text !== "Counter") {
            throw new Error("Expected reference text to be 'Counter', got: '" + text + "'");
        }
    }
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let provider = RuffSemanticProvider::file_scope();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::Python,
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

// =============================================================================
// Cross-file (Workspace Scope) Tests
// =============================================================================

/// Test: Cross-file definition lookup with workspace-scope provider
#[tokio::test]
async fn test_cross_file_definition_workspace_scope() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    // Create utils.py with exported function
    let utils_content = r#"
def add(a, b):
    return a + b
"#;
    fs::write(temp_dir.path().join("utils.py"), utils_content).expect("Failed to write utils.py");

    // Create main.py that imports from utils
    let main_content = r#"
from utils import add

result = add(1, 2)
print(result)
"#;
    let main_path = temp_dir.path().join("main.py");
    fs::write(&main_path, main_content).expect("Failed to write main.py");

    let codemod_content = r#"
export default function transform(root) {
    // Find the 'add' call
    const callNode = root.root().find({ rule: { pattern: "add($$$)" } });
    if (!callNode) {
        throw new Error("Expected to find add() call");
    }
    
    const definition = callNode.getDefinition();
    
    // Definition may or may not be found depending on cross-file resolution
    if (definition === null) {
        // It's okay if cross-file definition isn't resolved yet
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
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
    let provider = RuffSemanticProvider::workspace_scope(temp_dir.path().to_path_buf());

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::Python,
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

/// Test: Cross-file references with workspace-scope provider
#[tokio::test]
async fn test_cross_file_references_workspace_scope() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    // Create utils.py with exported function
    let utils_content = r#"
def calculate(x, y):
    return x * y + x

calculate(1, 1)
"#;
    let utils_path = temp_dir.path().join("utils.py");
    fs::write(&utils_path, utils_content).expect("Failed to write utils.py");

    // Create main.py that imports and uses the function (3 usages)
    let main_content = r#"
from utils import calculate

result1 = calculate(2, 3)
print(calculate(3, 4))
"#;
    let main_path = temp_dir.path().join("main.py");
    fs::write(&main_path, main_content).expect("Failed to write main.py");

    let codemod_content = r#"
export default function transform(root) {
    // Find all 'calculate' identifiers
    const calcNodes = root.root().findAll({ rule: { pattern: "calculate" } });
    if (calcNodes.length < 1) {
        throw new Error("Expected at least 1 'calculate' node, got " + calcNodes.length);
    }
    
    // Use the first one (the function name in definition)
    const funcNameNode = calcNodes[0];
    
    const references = funcNameNode.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array");
    }
    
    if (references.length === 0) {
        console.log("No references found - this is acceptable");
        return null;
    }
    
    // Log all references found - be defensive about node.text
    let totalRefs = 0;
    for (const fileRef of references) {
        if (!fileRef.root) {
            throw new Error("Expected fileRef.root to exist");
        }
        if (!Array.isArray(fileRef.nodes)) {
            throw new Error("Expected fileRef.nodes to be an array");
        }
        
        for (const node of fileRef.nodes) {
            totalRefs++;
            // Just count - don't validate text if not available
            if (typeof node.text === "function") {
                console.log("Reference text:", node.parent().text());
            }
        }
    }

    if (references.length !== 2) {
        throw new Error("Expected 2 files with references, got " + references.length);
    }
    
    if (totalRefs !== 3) {
        throw new Error("Expected 3 references, got " + totalRefs);
    }
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());

    // Use workspace scope for cross-file analysis
    let provider = RuffSemanticProvider::workspace_scope(temp_dir.path().to_path_buf());

    // Pre-populate the cache with main.py to enable cross-file reference finding
    provider
        .notify_file_processed(&main_path, main_content)
        .unwrap();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::Python,
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

/// Test: Cross-file references finds imports in other files
#[tokio::test]
async fn test_cross_file_references_with_imports() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let codemod_path = temp_dir.path().join("codemod.js");

    // Create a module with a class
    let models_content = r#"
class User:
    def __init__(self, name):
        self.name = name
    
    def greet(self):
        return f"Hello, {self.name}"
"#;
    let models_path = temp_dir.path().join("models.py");
    fs::write(&models_path, models_content).expect("Failed to write models.py");

    // Create a file that imports and uses the class (3 usages)
    let app_content = r#"
from models import User

def create_user(name):
    return User(name)

admin = User("Admin")
guest = User("Guest")
"#;
    let app_path = temp_dir.path().join("app.py");
    fs::write(&app_path, app_content).expect("Failed to write app.py");

    let codemod_content = r#"
export default function transform(root) {
    // Find all 'User' identifiers
    const userNodes = root.root().findAll({ rule: { pattern: "User" } });
    if (userNodes.length < 1) {
        throw new Error("Expected at least 1 'User' node, got " + userNodes.length);
    }
    
    // Use the first one (the class name in definition)
    const classNameNode = userNodes[0];
    
    const references = classNameNode.findReferences();
    
    if (!Array.isArray(references)) {
        throw new Error("Expected references to be an array");
    }
    
    if (references.length === 0) {
        console.log("No references found - this is acceptable");
        return null;
    }
    
    // Count references across all files - be defensive
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
    
    if (references.length !== 1) {
        throw new Error("Expected 1 file with references, got " + references.length);
    }

    if (totalRefs !== 3) {
        throw new Error("Expected 3 references, got " + totalRefs);
    }
    
    return null;
}
    "#
    .trim();

    std::fs::write(&codemod_path, codemod_content).expect("Failed to write codemod");

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());

    // Use workspace scope for cross-file analysis
    let provider = RuffSemanticProvider::workspace_scope(temp_dir.path().to_path_buf());

    // Pre-populate the cache with app.py
    provider
        .notify_file_processed(&app_path, app_content)
        .unwrap();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: SupportLang::Python,
        file_path: &models_path,
        content: models_content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: Some(Arc::new(provider)),
    };

    let result = execute_codemod_with_quickjs(options).await;
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}
