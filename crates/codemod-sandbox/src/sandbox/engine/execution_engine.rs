use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::serde::JsValue;
use crate::ast_grep::sg_node::{SgNodeRjs, SgRootRjs};
use crate::ast_grep::AstGrepModule;
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::ModuleResolver;
use crate::utils::quickjs_utils::maybe_promise;
use crate::workflow_global::WorkflowGlobalModule;
use ast_grep_config::RuleConfig;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::AstGrep;
use ast_grep_language::SupportLang;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use language_core::SemanticProvider;
use rquickjs::{async_with, AsyncContext, AsyncRuntime};
use rquickjs::{CatchResultExt, Function, Module};
use rquickjs::{IntoJs, Object};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[cfg(feature = "native")]
use gag::BufferRedirect;

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
    pub language: SupportLang,
    pub file_path: &'a Path,
    pub content: &'a str,
    pub selector_config: Option<Arc<Box<RuleConfig<SupportLang>>>>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub matrix_values: Option<HashMap<String, serde_json::Value>>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    /// Optional semantic provider for symbol indexing (go-to-definition, find-references)
    pub semantic_provider: Option<Arc<dyn SemanticProvider>>,
    pub console_log_collector: Option<Box<dyn FnMut(String) + Send + Sync>>,
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

    // Wrap console_log_collector in Arc<Mutex<...>> for use in closures
    let console_log_collector = options
        .console_log_collector
        .map(|collector| Arc::new(Mutex::new(collector)));

    // Initialize QuickJS runtime and context
    let runtime = AsyncRuntime::new().map_err(|e| {
        let error_msg = format!("Failed to create AsyncRuntime: {e}");
        if let Some(ref collector) = console_log_collector {
            if let Ok(mut collector) = collector.lock() {
                collector(format!("ERROR: {}", error_msg));
            }
        }
        ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: error_msg,
            },
        }
    })?;

    // Create AstGrep instance for the SgRootRjs
    let ast_grep = AstGrep::new(options.content, options.language);

    // Set up built-in modules
    // Convert Arc<Mutex<...>> back to Option<Box<dyn FnMut(String) + Send + Sync>> for LlrtModuleBuilder
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

        let console_log_collector = console_log_collector.clone();
        let execution = async {
            // Capture stdout during JSSG execution
            // Note: This may fail in parallel execution contexts, so we handle it gracefully
            #[cfg(feature = "native")]
            let mut redirect = BufferRedirect::stdout().ok();

            let module = Module::declare(ctx.clone(), "__codemod_entry.js", js_code)
                .catch(&ctx)
                .map_err(|e| {
                    let error_msg = format!("Failed to declare module: {e}");
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            // Evaluate module.
            let (evaluated, _) = module
                .eval()
                .catch(&ctx)
                .map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to evaluate module: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            while ctx.execute_pending_job() {}

            // Get the default export.
            let namespace = evaluated
                .namespace()
                .catch(&ctx)
                .map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to get namespace: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            let file_path_str = options.file_path.to_string_lossy().to_string();
            let parsed_content =
                SgRootRjs::try_new_with_semantic(
                    ast_grep,
                    Some(file_path_str.clone()),
                    options.semantic_provider.clone(),
                    Some(file_path_str), // Pass current file path for write() validation
                ).map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to parse content: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            // Calculate matches inside the JS context
            let matches: Option<Vec<SgNodeRjs<'_>>> = if let Some(selector_config) = &options.selector_config {
                let root_node = parsed_content.root(ctx.clone()).map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to get root node: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
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

            let language_str = options.language.to_string();

            let run_options = Object::new(ctx.clone()).map_err(|e| {
                let error_msg = e.to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: Failed to create run options: {}", error_msg));
                    }
                }
                ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: error_msg,
                    },
                }
            })?;

            let params_js = params.into_iter()
                .map(|(k, v)| (k, JsValue(v)))
                .collect::<HashMap<String, JsValue>>();
            run_options.set("params", params_js).map_err(|e| {
                let error_msg = e.to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: Failed to set params: {}", error_msg));
                    }
                }
                ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: error_msg,
                    },
                }
            })?;

            run_options.set("language", &language_str).map_err(|e| {
                let error_msg = e.to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: Failed to set language: {}", error_msg));
                    }
                }
                ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: error_msg,
                    },
                }
            })?;
            run_options.set("matches", matches).map_err(|e| {
                let error_msg = e.to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: Failed to set matches: {}", error_msg));
                    }
                }
                ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: error_msg,
                    },
                }
            })?;

            let matrix_values_js = options.matrix_values
                .map(|input| input.into_iter()
                .map(|(k, v)| (k, JsValue(v)))
                .collect::<HashMap<String, JsValue>>());

            run_options.set("matrixValues", matrix_values_js).map_err(|e| {
                let error_msg = e.to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: Failed to set matrix values: {}", error_msg));
                    }
                }
                ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: error_msg,
                    },
                }
            })?;

            let run_options_qjs = run_options.into_js(&ctx);

            let func = namespace
                .get::<_, Function>("executeCodemod")
                .catch(&ctx)
                .map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to get executeCodemod function: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            // Call it and return value.
            let result_obj_promise = func.call((parsed_content, run_options_qjs)).catch(&ctx).map_err(|e| {
                let error_msg = e.to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: Failed to call executeCodemod: {}", error_msg));
                    }
                }
                ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: error_msg,
                    },
                }
            })?;
            let result_obj = maybe_promise(result_obj_promise)
                .await
                .catch(&ctx)
                .map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to resolve promise: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            // Flush stdout before reading captured output
            #[cfg(feature = "native")]
            if let Some(ref mut redirect) = redirect {
                std::io::stdout().flush().ok();
                // Read captured stdout output
                let mut captured = String::new();
                if let Err(e) = redirect.read_to_string(&mut captured) {
                    let error_msg = format!("Failed to read captured stdout: {e}");
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: {}", error_msg));
                        }
                    }
                } else if !captured.is_empty() {
                    // Pass captured stdout to console_log_collector line by line
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            for line in captured.lines() {
                                collector(line.to_string());
                            }
                            // If captured ends with newline, also send empty line for last newline
                            if captured.ends_with('\n') {
                                collector(String::new());
                            }
                        }
                    }
                }
            }

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
                let error_msg = "Invalid result type".to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: {}", error_msg));
                    }
                }
                Err(ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                        message: error_msg,
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
    pub console_log_collector: Option<Box<dyn FnMut(String) + Send + Sync>>,
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

    // Wrap console_log_collector in Arc<Mutex<...>> for use in closures
    let console_log_collector = options
        .console_log_collector
        .map(|collector| Arc::new(Mutex::new(collector)));

    // Initialize QuickJS runtime and context
    let runtime = AsyncRuntime::new().map_err(|e| {
        let error_msg = format!("Failed to create AsyncRuntime: {e}");
        if let Some(ref collector) = console_log_collector {
            if let Ok(mut collector) = collector.lock() {
                collector(format!("ERROR: {}", error_msg));
            }
        }
        ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: error_msg,
            },
        }
    })?;

    // Convert Arc<Mutex<...>> back to Option<Box<dyn FnMut(String) + Send + Sync>> for LlrtModuleBuilder
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

    let context = AsyncContext::full(&runtime).await.map_err(|e| {
        let error_msg = format!("Failed to create AsyncContext: {e}");
        if let Some(ref collector) = console_log_collector {
            if let Ok(mut collector) = collector.lock() {
                collector(format!("ERROR: {}", error_msg));
            }
        }
        ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::ContextCreationFailed {
                message: error_msg,
            },
        }
    })?;

    // Clone Arc for use in closure
    let console_log_collector_clone = console_log_collector.clone();

    // Execute JavaScript code
    async_with!(context => |ctx| {
        let console_log_collector = console_log_collector_clone.clone();
        global_attachment.attach(&ctx).map_err(|e| {
            let error_msg = format!("Failed to attach global modules: {e}");
            if let Some(ref collector) = console_log_collector {
                if let Ok(mut collector) = collector.lock() {
                    collector(format!("ERROR: {}", error_msg));
                }
            }
            ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: error_msg,
                },
            }
        })?;

        let console_log_collector = console_log_collector.clone();
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
                .map_err(|e| {
                    let error_msg = format!("Failed to declare module: {e}");
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            // Evaluate module
            let _ = module
                .eval()
                .catch(&ctx)
                .map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to evaluate module: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                            message: error_msg,
                        },
                    }
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
    use ast_grep_language::SupportLang;
    use std::fs;
    use std::path::Path;
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
            console_log_collector: None,
        };

        let result = execute_codemod_with_quickjs(options).await;

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
            console_log_collector: None,
        };

        let result = execute_codemod_with_quickjs(options).await;

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
            console_log_collector: None,
        };

        let result = execute_codemod_with_quickjs(options).await;

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
            console_log_collector: None,
        };

        let result = execute_codemod_with_quickjs(options).await;

        assert!(result.is_err());
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
            console_log_collector: None,
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

        let options = JssgExecutionOptions {
            script_path: nonexistent_path,
            resolver,
            language: SupportLang::JavaScript,
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            console_log_collector: None,
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
