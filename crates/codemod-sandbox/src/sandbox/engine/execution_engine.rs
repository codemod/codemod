use super::codemod_lang::CodemodLang;
use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use super::transform_helpers::{
    build_transform_options, process_transform_result, ModificationCheck,
};
use crate::ast_grep::sg_node::{SgNodeRjs, SgRootRjs};
use crate::ast_grep::AstGrepModule;
use crate::metrics::{MetricsContext, MetricsModule};
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::ModuleResolver;
use crate::utils::quickjs_utils::maybe_promise;
use crate::workflow_global::WorkflowGlobalModule;
use ast_grep_config::RuleConfig;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::AstGrep;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use language_core::SemanticProvider;
use rquickjs::{async_with, AsyncContext, AsyncRuntime};
use rquickjs::{CatchResultExt, Function, Module};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Flag indicating whether execution is in test mode.
/// When in test mode, `jssgTransform` becomes a no-op.
#[derive(Debug, Clone)]
pub struct ExecutionModeFlag {
    pub test_mode: bool,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for ExecutionModeFlag {
    type Changed<'to> = ExecutionModeFlag;
}

/// Execution context passed to `jssgTransform` via QuickJS userdata.
/// Contains the params and matrixValues from the parent codemod execution.
#[derive(Debug, Clone)]
pub struct JssgExecutionContext {
    pub params: HashMap<String, serde_json::Value>,
    pub matrix_values: Option<HashMap<String, serde_json::Value>>,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for JssgExecutionContext {
    type Changed<'to> = JssgExecutionContext;
}

/// Details of a modified file
#[derive(Debug, Clone)]
pub struct ModifiedResult {
    pub content: String,
    pub rename_to: Option<PathBuf>,
}

/// Result of executing a codemod on a single file
#[derive(Debug, Clone)]
pub enum ExecutionResult {
    Modified(ModifiedResult),
    Unmodified,
    Skipped,
}

/// A file change produced by `jssgTransform` (secondary output)
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub result: ExecutionResult,
}

/// Output of a codemod execution including both the primary result
/// and any secondary file changes produced by `jssgTransform`.
#[derive(Debug, Clone)]
pub struct CodemodOutput {
    pub primary: ExecutionResult,
    pub secondary: Vec<FileChange>,
}

/// Shared accumulator for file changes produced by `jssgTransform`.
/// Stored as QuickJS userdata so the JS-facing function can push changes
/// without touching the filesystem.
#[derive(Debug, Clone, Default)]
pub struct JssgFileChanges {
    pub changes: Arc<Mutex<Vec<FileChange>>>,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for JssgFileChanges {
    type Changed<'to> = JssgFileChanges;
}

/// Options for executing a codemod on a single file
pub struct JssgExecutionOptions<'a, R> {
    pub script_path: &'a Path,
    pub resolver: Arc<R>,
    pub language: CodemodLang,
    pub file_path: &'a Path,
    pub content: &'a str,
    pub selector_config: Option<Arc<Box<RuleConfig<CodemodLang>>>>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub matrix_values: Option<HashMap<String, serde_json::Value>>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    /// Optional semantic provider for symbol indexing (go-to-definition, find-references)
    pub semantic_provider: Option<Arc<dyn SemanticProvider>>,
    /// Optional metrics context for tracking metrics across execution
    pub metrics_context: Option<MetricsContext>,
    /// Whether this is a test execution (jssgTransform becomes a no-op)
    pub test_mode: bool,
}

/// Execute a codemod on string content using QuickJS
/// This is the core execution logic that doesn't touch the filesystem
pub async fn execute_codemod_with_quickjs<'a, R>(
    options: JssgExecutionOptions<'a, R>,
) -> Result<CodemodOutput, ExecutionError>
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

    // Create AstGrep instance for the SgRootRjs
    let ast_grep = AstGrep::new(options.content, options.language);

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
    built_in_resolver = built_in_resolver.add_name("codemod:workflow");
    built_in_loader = built_in_loader.with_module("codemod:workflow", WorkflowGlobalModule);

    // Add MetricsModule (metrics tracking)
    built_in_resolver = built_in_resolver.add_name("codemod:metrics");
    built_in_loader = built_in_loader.with_module("codemod:metrics", MetricsModule);

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

    // Capture metrics context for use inside async block
    let metrics_context = options.metrics_context.clone();
    let test_mode = options.test_mode;

    // Execute JavaScript code
    async_with!(context => |ctx| {
        // Store execution mode flag in runtime userdata
        ctx.store_userdata(ExecutionModeFlag { test_mode }).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store ExecutionModeFlag: {:?}", e),
            },
        })?;

        // Store shared accumulator for jssgTransform file changes
        let jssg_file_changes = JssgFileChanges::default();
        ctx.store_userdata(jssg_file_changes.clone()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store JssgFileChanges: {:?}", e),
            },
        })?;

        // Store jssg execution context so jssgTransform can access params/matrixValues
        ctx.store_userdata(JssgExecutionContext {
            params: params.clone(),
            matrix_values: options.matrix_values.clone(),
        }).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store JssgExecutionContext: {:?}", e),
            },
        })?;

        // Store metrics context in runtime userdata if provided (must be done inside async_with)
        if let Some(ref metrics_ctx) = metrics_context {
            ctx.store_userdata(metrics_ctx.clone()).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to store MetricsContext: {:?}", e),
                },
            })?;
        }

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

            let file_path_str = options.file_path.to_string_lossy().to_string();
            let parsed_content =
                SgRootRjs::try_new_with_semantic(
                    ast_grep,
                    Some(file_path_str.clone()),
                    options.semantic_provider.clone(),
                    Some(file_path_str), // Pass current file path for write() validation
                ).map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            // Keep a reference to read rename_to after JS execution
            let sg_root_inner = Arc::clone(&parsed_content.inner);

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
                    return Ok(CodemodOutput { primary: ExecutionResult::Skipped, secondary: vec![] });
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

            let run_options_qjs = build_transform_options(
                &ctx,
                params,
                &language_str,
                options.matrix_values,
                matches,
            )?;

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

            let primary = process_transform_result(
                &result_obj,
                &sg_root_inner,
                ModificationCheck::StringEquality { original_content: options.content },
            )?;

            let secondary = jssg_file_changes.changes.lock()
                .map(|guard| guard.clone())
                .unwrap_or_default();

            Ok(CodemodOutput { primary, secondary })
        };
        execution.await
    })
    .await
}

/// Options for executing a standalone JavaScript file
pub struct SimpleJsExecutionOptions<'a, R> {
    pub script_path: &'a Path,
    pub resolver: Arc<R>,
    /// Optional metrics context for tracking metrics across execution
    pub metrics_context: Option<MetricsContext>,
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

    built_in_resolver = built_in_resolver.add_name("codemod:metrics");
    built_in_loader = built_in_loader.with_module("codemod:metrics", MetricsModule);

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

    let metrics_context = options.metrics_context.clone();

    // Execute JavaScript code
    async_with!(context => |ctx| {
        if let Some(ref metrics_ctx) = metrics_context {
            ctx.store_userdata(metrics_ctx.clone()).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to store MetricsContext: {:?}", e),
                },
            })?;
        }

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
    use ast_grep_language::SupportLang;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn js_lang() -> CodemodLang {
        CodemodLang::Static(SupportLang::JavaScript)
    }

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
            language: js_lang(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            test_mode: false,
        };

        let result = execute_codemod_with_quickjs(options).await;

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Modified(modified) => {
                    assert!(modified.content.contains("logger.log(\"Hello, world!\")"));
                    assert!(modified.content.contains("logger.log(\"Debug message\")"));
                    // console.info should remain unchanged
                    assert!(modified.content.contains("console.info(\"Info message\")"));
                    assert!(modified.rename_to.is_none());
                }
                other => panic!("Expected modified result, got: {:?}", other),
            },
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
            language: js_lang(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            test_mode: false,
        };

        let result = execute_codemod_with_quickjs(options).await;

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Unmodified => {
                    // Expected behavior - no console.log or console.debug found
                }
                other => panic!("Expected unmodified result, got: {:?}", other),
            },
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
            language: js_lang(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            test_mode: false,
        };

        let result = execute_codemod_with_quickjs(options).await;

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Unmodified => {
                    // Expected behavior - codemod returned null
                }
                other => panic!("Expected unmodified result, got: {:?}", other),
            },
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
            language: js_lang(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            test_mode: false,
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
            language: js_lang(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            test_mode: false,
        };

        let result = execute_codemod_with_quickjs(options).await;

        match result {
            Err(ExecutionError::Runtime { source }) => {
                assert!(source.to_string().contains("Invalid result type"));
            }
            Ok(output) => panic!(
                "Expected runtime error for invalid return type, got: {:?}",
                output.primary
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
            language: js_lang(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            test_mode: false,
        };

        let result = execute_codemod_with_quickjs(options).await;

        // Should fail due to file not found
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execution_result_debug_clone() {
        let result1 = ExecutionResult::Modified(ModifiedResult {
            content: "test".to_string(),
            rename_to: None,
        });
        let result2 = result1.clone();

        match (result1, result2) {
            (ExecutionResult::Modified(m1), ExecutionResult::Modified(m2)) => {
                assert_eq!(m1.content, m2.content);
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

    #[tokio::test]
    async fn test_execute_codemod_with_metrics() {
        use crate::metrics::MetricsContext;

        // Create a metrics context for this test
        let metrics_ctx = MetricsContext::new();

        // Simpler codemod that just counts console.log calls
        let codemod_content = r#"
import { useMetricAtom } from "codemod:metrics";

const callMetric = useMetricAtom("call-count");

export default function transform(root) {
  const rootNode = root.root();

  // Find all console.log calls
  const calls = rootNode.findAll({
    rule: {
      pattern: "console.log($$$ARGS)",
    },
  });

  // Count each call
  for (const call of calls) {
    callMetric.increment({ method: "console.log" });
  }

  return null;
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());

        // Test with JS content that has console.log calls
        let content = r#"
function example() {
    console.log("Hello");
    console.log("World");
    console.log(1, 2, 3);
}
        "#
        .trim();

        let file_path = Path::new("test.js");

        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: js_lang(),
            file_path,
            content,
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: Some(metrics_ctx.clone()),
            test_mode: false,
        };

        let result = execute_codemod_with_quickjs(options).await;

        // Should return Unmodified since we return null
        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Unmodified => {
                    // Expected
                }
                other => panic!("Expected unmodified result, got: {:?}", other),
            },
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }

        // Now check the metrics
        let all_metrics = metrics_ctx.get_all();
        assert!(
            all_metrics.contains_key("call-count"),
            "call-count metric should exist"
        );

        let call_count_entries = all_metrics.get("call-count").unwrap();
        let console_log_entry = call_count_entries
            .iter()
            .find(|e| e.cardinality.get("method") == Some(&"console.log".to_string()));
        assert!(
            console_log_entry.is_some(),
            "Should have a console.log metric entry"
        );
        assert_eq!(
            console_log_entry.unwrap().count,
            3,
            "Should have counted 3 console.log calls"
        );
    }
}
