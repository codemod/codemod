use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::serde::JsValue;
use crate::ast_grep::sg_node::{SgNodeRjs, SgRootRjs};
use crate::ast_grep::AstGrepModule;
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::ModuleResolver;
#[cfg(feature = "native")]
use crate::utils::quickjs_utils::maybe_promise;
use crate::workflow_global::WorkflowGlobalModule;
use ast_grep_config::RuleConfig;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::AstGrep;
use codemod_ast_grep_dynamic_lang::DynamicLang;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use rquickjs::{async_with, AsyncContext, AsyncRuntime};
use rquickjs::{CatchResultExt, Function, Module};
use rquickjs::{IntoJs, Object};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

/// Result of executing a codemod on a single file
#[derive(Debug, Clone)]
pub enum ExecutionResult {
    Modified(String),
    Unmodified,
    Skipped,
}

/// Options for executing a codemod on a single file
pub struct JssgExecutionOptions<'a, R> {
    pub script_path: &'a Path,
    pub resolver: Arc<R>,
    pub language: DynamicLang,
    pub file_path: &'a Path,
    pub content: &'a str,
    pub selector_config: Option<Arc<Box<RuleConfig<DynamicLang>>>>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub matrix_values: Option<HashMap<String, serde_json::Value>>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
}

/// Execute a codemod on string content using QuickJS
/// This is the core execution logic that doesn't touch the filesystem
pub async fn execute_codemod_with_quickjs<'a, R>(
    options: JssgExecutionOptions<'a, R>,
) -> Result<ExecutionResult, ExecutionError>
where
    R: ModuleResolver + 'static,
{
    let script_name = options
        .script_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("main.js");

    let js_code = format!(
        include_str!("scripts/main_script.js.txt"),
        script_name = script_name
    );

    let params: HashMap<String, serde_json::Value> = options.params.unwrap_or_default();

    // Initialize QuickJS runtime and context
    let runtime = AsyncRuntime::new().map_err(|e| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: format!("Failed to create AsyncRuntime: {e}"),
        },
    })?;

    let ast_grep = AstGrep::new(options.content, options.language);

    if let Some(selector_config) = &options.selector_config {
        let matches: Vec<_> = ast_grep
            .root()
            .dfs()
            .filter_map(move |node| selector_config.matcher.match_node(node))
            .collect();

        if matches.is_empty() {
            return Ok(ExecutionResult::Skipped);
        }
    }

    // Set up built-in modules
    let mut module_builder = LlrtModuleBuilder::build();
    if let Some(capabilities) = options.capabilities {
        for capability in capabilities {
            match capability {
                LlrtSupportedModules::Fetch => {
                    module_builder.enable_fetch();
                }
                LlrtSupportedModules::Fs => {
                    module_builder.enable_fs();
                }
                LlrtSupportedModules::ChildProcess => {
                    module_builder.enable_child_process();
                }
                _ => {}
            }
        }
    }
    let (mut built_in_resolver, mut built_in_loader, global_attachment) =
        module_builder.builder.build();
    // Add AstGrepModule
    built_in_resolver = built_in_resolver.add_name("codemod:ast-grep");
    built_in_loader = built_in_loader.with_module("codemod:ast-grep", AstGrepModule);

    // Add WorkflowGlobalModule (step outputs)
    let step_outputs_path = std::env::temp_dir().join("codemod_step_outputs.txt");
    std::env::set_var("STEP_OUTPUTS", &step_outputs_path);
    built_in_resolver = built_in_resolver.add_name("codemod:workflow");
    built_in_loader = built_in_loader.with_module("codemod:workflow", WorkflowGlobalModule);

    let fs_resolver = QuickJSResolver::new(Arc::clone(&options.resolver));
    let fs_loader = QuickJSLoader;

    // Combine resolvers and loaders
    runtime
        .set_loader(
            (built_in_resolver, fs_resolver),
            (built_in_loader, fs_loader),
        )
        .await;

    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::ContextCreationFailed {
                message: format!("Failed to create AsyncContext: {e}"),
            },
        })?;

    // Execute JavaScript code
    async_with!(context => |ctx| {
        global_attachment.attach(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach global modules: {e}"),
            },
        })?;

        let execution = async {
            let module = Module::declare(ctx.clone(), "__codemod_entry.js", js_code)
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to declare module: {e}"),
                    },
                })?;

            // Evaluate module.
            let (evaluated, _) = module
                .eval()
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;

            while ctx.execute_pending_job() {}

            // Get the default export.
            let namespace = evaluated
                .namespace()
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            let parsed_content =
                SgRootRjs::try_new_from_ast_grep(ast_grep, Some(options.file_path.to_string_lossy().to_string())).map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            // Calculate matches inside the JS context
            let matches: Option<Vec<SgNodeRjs<'_>>> = if let Some(selector_config) = &options.selector_config {
                let root_node = parsed_content.root(ctx.clone()).map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;
                let ast_matches: Vec<_> = root_node.inner_node.dfs()
                    .filter_map(|node| selector_config.matcher.match_node(node))
                    .collect();

                if ast_matches.is_empty() {
                    return Ok(ExecutionResult::Skipped);
                }

                Some(ast_matches.into_iter().map(|node_match| SgNodeRjs {
                    root: Arc::clone(&parsed_content.inner),
                    inner_node: node_match,
                    _phantom: PhantomData,
                }).collect())
            } else {
                None
            };

            let language_str = options.language.name();

            let run_options = Object::new(ctx.clone()).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;

            let params_js = params.into_iter()
                .map(|(k, v)| (k, JsValue(v)))
                .collect::<HashMap<String, JsValue>>();
            run_options.set("params", params_js).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;

            run_options.set("language", language_str).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;
            run_options.set("matches", matches).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;

            let matrix_values_js = options.matrix_values
                .map(|input| input.into_iter()
                .map(|(k, v)| (k, JsValue(v)))
                .collect::<HashMap<String, JsValue>>());

            run_options.set("matrixValues", matrix_values_js).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;

            let run_options_qjs = run_options.into_js(&ctx);

            let func = namespace
                .get::<_, Function>("executeCodemod")
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            // Call it and return value.
            let result_obj_promise = func.call((parsed_content, run_options_qjs)).catch(&ctx).map_err(|e| {
                ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                }
            })?;
            let result_obj = maybe_promise(result_obj_promise)
                .await
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            if result_obj.is_string() {
                let new_content = result_obj.get::<String>().unwrap();
                if new_content == options.content {
                    Ok(ExecutionResult::Unmodified)
                } else {
                    Ok(ExecutionResult::Modified(new_content))
                }
            } else if result_obj.is_null() || result_obj.is_undefined() {
                Ok(ExecutionResult::Unmodified)
            } else {
                Err(ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                        message: "Invalid result type".to_string(),
                    },
                })
            }
        };
        execution.await
    })
    .await
}

/// Options for executing a standalone JavaScript file
pub struct SimpleJsExecutionOptions<'a, R> {
    pub script_path: &'a Path,
    pub resolver: Arc<R>,
}

/// Execute a standalone JavaScript file with QuickJS (like node script.js)
/// This provides all LLRT capabilities and ast-grep module but doesn't
/// impose any file transformation workflow
pub async fn execute_js_with_quickjs<'a, R>(
    options: SimpleJsExecutionOptions<'a, R>,
) -> Result<(), ExecutionError>
where
    R: ModuleResolver + 'static,
{
    let script_name = options
        .script_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("script.js");

    let js_code = format!(
        include_str!("scripts/exec_script.txt"),
        script_name = script_name
    );

    let runtime = AsyncRuntime::new().map_err(|e| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: format!("Failed to create AsyncRuntime: {e}"),
        },
    })?;

    let mut module_builder = LlrtModuleBuilder::build();
    module_builder.enable_fetch();
    module_builder.enable_fs();
    module_builder.enable_child_process();

    let (mut built_in_resolver, mut built_in_loader, global_attachment) =
        module_builder.builder.build();

    built_in_resolver = built_in_resolver.add_name("codemod:ast-grep");
    built_in_loader = built_in_loader.with_module("codemod:ast-grep", AstGrepModule);

    let fs_resolver = QuickJSResolver::new(Arc::clone(&options.resolver));
    let fs_loader = QuickJSLoader;

    runtime
        .set_loader(
            (built_in_resolver, fs_resolver),
            (built_in_loader, fs_loader),
        )
        .await;

    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::ContextCreationFailed {
                message: format!("Failed to create AsyncContext: {e}"),
            },
        })?;

    // Execute JavaScript code
    async_with!(context => |ctx| {
        global_attachment.attach(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach global modules: {e}"),
            },
        })?;

        let execution = async {
            // Place the entry module in the same directory as the script for proper resolution
            let entry_module_path = options
                .script_path
                .parent()
                .unwrap_or(Path::new("."))
                .join("__codemod_exec_entry.js");
            let entry_module_name = entry_module_path
                .to_str()
                .unwrap_or("__codemod_exec_entry.js");

            let module = Module::declare(ctx.clone(), entry_module_name, js_code)
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to declare module: {e}"),
                    },
                })?;

            // Evaluate module
            let _ = module
                .eval()
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                        message: e.to_string(),
                    },
                })?;

            while ctx.execute_pending_job() {}

            Ok(())
        };
        execution.await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
    use crate::tree_sitter::load_tree_sitter;
    use codemod_ast_grep_dynamic_lang::supported_langs::SupportedLanguage;
    use std::fs;
    use std::path::Path;
    use std::str::FromStr;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create a temporary codemod file and test directory
    fn setup_test_codemod(codemod_content: &str) -> (TempDir, std::path::PathBuf) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let codemod_path = temp_dir.path().join("test_codemod.js");
        fs::write(&codemod_path, codemod_content).expect("Failed to write codemod file");
        (temp_dir, codemod_path)
    }

    /// Create a simple console.log to logger.log codemod for testing
    fn create_test_codemod() -> String {
        r#"
export default function transform(root) {
  const rootNode = root.root();

  const nodes = rootNode.findAll({
    rule: {
      any: [
        { pattern: "console.log($ARG)" },
        { pattern: "console.debug($ARG)" },
      ]
    },
  });

  const edits = nodes.map(node => {
    const arg = node.getMatch("ARG").text();
    return node.replace(`logger.log(${arg})`);
  });

  const newSource = rootNode.commitEdits(edits);
  return newSource;
}
        "#
        .trim()
        .to_string()
    }

    #[tokio::test]
    async fn test_execute_codemod_with_modifications() {
        let codemod_content = create_test_codemod();
        let (_temp_dir, codemod_path) = setup_test_codemod(&codemod_content);

        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());
        let file_path = Path::new("test.js");
        let content = r#"
function example() {
    console.log("Hello, world!");
    console.debug("Debug message");
    console.info("Info message");
}
        "#
        .trim();

        let _ = load_tree_sitter(&[SupportedLanguage::Javascript], None).await;

        let result = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: DynamicLang::from_str("javascript").unwrap(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
        })
        .await;

        match result {
            Ok(ExecutionResult::Modified(new_content)) => {
                assert!(new_content.contains("logger.log(\"Hello, world!\")"));
                assert!(new_content.contains("logger.log(\"Debug message\")"));
                // console.info should remain unchanged
                assert!(new_content.contains("console.info(\"Info message\")"));
            }
            Ok(other) => panic!("Expected modified result, got: {:?}", other),
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_execute_codemod_no_modifications() {
        let codemod_content = create_test_codemod();
        let (_temp_dir, codemod_path) = setup_test_codemod(&codemod_content);

        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());
        let file_path = Path::new("test.js");
        let content = r#"
function example() {
    console.info("Info message");
    console.warn("Warning message");
}
        "#
        .trim();

        let _ = load_tree_sitter(&[SupportedLanguage::Javascript], None).await;

        let result = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: DynamicLang::from_str("javascript").unwrap(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
        })
        .await;

        match result {
            Ok(ExecutionResult::Unmodified) => {
                // Expected behavior - no console.log or console.debug found
            }
            Ok(other) => panic!("Expected unmodified result, got: {:?}", other),
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_execute_codemod_null_return() {
        let codemod_content = r#"
export default function transform(root) {
  return null;
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());
        let file_path = Path::new("test.js");
        let content = r#"
function example() {
    console.log("Hello, world!");
}
        "#
        .trim();

        let _ = load_tree_sitter(&[SupportedLanguage::Javascript], None).await;

        let result = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: DynamicLang::from_str("javascript").unwrap(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
        })
        .await;

        match result {
            Ok(ExecutionResult::Unmodified) => {
                // Expected behavior - codemod returned null
            }
            Ok(other) => panic!("Expected unmodified result, got: {:?}", other),
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_execute_codemod_error_handling() {
        let codemod_content = r#"
export default function transform(root) {
  throw new Error("Test error");
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());
        let file_path = Path::new("test.js");
        let content = r#"
function example() {
    console.log("Hello, world!");
}
        "#
        .trim();

        let _ = load_tree_sitter(&[SupportedLanguage::Javascript], None).await;

        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: DynamicLang::from_str("javascript").unwrap(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
        };

        let result = execute_codemod_with_quickjs(options).await;

        match result {
            Ok(ExecutionResult::Unmodified) => {
                // Expected behavior - error is caught and null is returned
            }
            Ok(other) => panic!(
                "Expected unmodified result due to error handling, got: {:?}",
                other
            ),
            Err(e) => panic!("Expected success with error handling, got error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_execute_codemod_invalid_return_type() {
        let codemod_content = r#"
export default function transform(root) {
  return 42;
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());
        let file_path = Path::new("test.js");
        let content = r#"
function example() {
    console.log("Hello, world!");
}
        "#
        .trim();

        let _ = load_tree_sitter(&[SupportedLanguage::Javascript], None).await;

        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: DynamicLang::from_str("javascript").unwrap(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
        };

        let result = execute_codemod_with_quickjs(options).await;

        match result {
            Err(ExecutionError::Runtime { source }) => {
                assert!(source.to_string().contains("Invalid result type"));
            }
            Ok(other) => panic!(
                "Expected runtime error for invalid return type, got: {:?}",
                other
            ),
            Err(e) => panic!("Expected specific runtime error, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_execute_codemod_nonexistent_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
        let nonexistent_path = Path::new("/nonexistent/path/codemod.js");
        let file_path = Path::new("test.js");
        let content = r#"
function example() {
    console.log("Hello, world!");
}
        "#
        .trim();

        let _ = load_tree_sitter(&[SupportedLanguage::Javascript], None).await;

        let options = JssgExecutionOptions {
            script_path: nonexistent_path,
            resolver,
            language: DynamicLang::from_str("javascript").unwrap(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
        };

        let result = execute_codemod_with_quickjs(options).await;

        // Should fail due to file not found
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execution_result_debug_clone() {
        let result1 = ExecutionResult::Modified("test".to_string());
        let result2 = result1.clone();

        match (result1, result2) {
            (ExecutionResult::Modified(content1), ExecutionResult::Modified(content2)) => {
                assert_eq!(content1, content2);
            }
            _ => panic!("Clone should preserve the variant and content"),
        }

        let result3 = ExecutionResult::Unmodified;
        let result4 = result3.clone();

        match (result3, result4) {
            (ExecutionResult::Unmodified, ExecutionResult::Unmodified) => {
                // Expected
            }
            _ => panic!("Clone should preserve the Unmodified variant"),
        }

        let result5 = ExecutionResult::Skipped;
        let result6 = result5.clone();

        match (result5, result6) {
            (ExecutionResult::Skipped, ExecutionResult::Skipped) => {
                // Expected
            }
            _ => panic!("Clone should preserve the Skipped variant"),
        }
    }
}
