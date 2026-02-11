use super::codemod_lang::CodemodLang;
use super::execution_engine::{CodemodOutput, ExecutionResult};
use super::quickjs_adapters::QuickJSResolver;
use super::transform_helpers::{
    build_transform_options, process_transform_result, ModificationCheck,
};
use crate::ast_grep::sg_node::{SgNodeRjs, SgRootRjs};
use crate::ast_grep::AstGrepModule;
use crate::metrics::{MetricsContext, MetricsModule};
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::{InMemoryLoader, InMemoryResolver, ModuleResolver};
use crate::utils::quickjs_utils::maybe_promise;
use ast_grep_config::RuleConfig;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::AstGrep;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use language_core::SemanticProvider;
use rquickjs::{async_with, AsyncContext, AsyncRuntime, CatchResultExt, Function, Module};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Default execution timeout in milliseconds (180s)
const DEFAULT_TIMEOUT_MS: u64 = 180000;

/// Default memory limit in bytes (512 MB)
const DEFAULT_MEMORY_LIMIT: usize = 512 * 1024 * 1024;

/// Default max stack size in bytes (4 MB)
const DEFAULT_MAX_STACK_SIZE: usize = 4 * 1024 * 1024;

/// SHA256 hash type (32 bytes)
pub type Sha256Hash = [u8; 32];

/// In-memory execution options for executing a codemod on a pre-parsed AST
pub struct InMemoryExecutionOptions<'a, R> {
    /// The JavaScript codemod source code (not a file path)
    pub codemod_source: &'a str,
    /// The programming language of the source code to transform
    pub language: CodemodLang,
    /// The pre-parsed AST (allows leveraging AST caching)
    pub ast: AstGrep<StrDoc<CodemodLang>>,
    /// SHA256 hash of the original content (used for modification detection)
    /// If None, any non-null result is considered modified
    pub original_sha256: Option<Sha256Hash>,
    /// Optional module resolver (if None, a no-op resolver is used)
    pub resolver: Option<Arc<R>>,
    /// Optional selector config for pre-filtering
    pub selector_config: Option<Arc<Box<RuleConfig<CodemodLang>>>>,
    /// Optional parameters passed to the codemod
    pub params: Option<HashMap<String, String>>,
    /// Optional matrix values for parameterized codemods
    pub matrix_values: Option<HashMap<String, serde_json::Value>>,
    /// Optional file path for the source code
    pub file_path: Option<&'a str>,
    /// Optional semantic provider for symbol indexing (go-to-definition, find-references)
    pub semantic_provider: Option<Arc<dyn SemanticProvider>>,
    /// Optional metrics context for tracking metrics across execution
    pub metrics_context: Option<MetricsContext>,
    /// Execution timeout in milliseconds (default: 200ms)
    pub timeout_ms: Option<u64>,
    /// Memory limit in bytes (default: 64 MB)
    pub memory_limit: Option<usize>,
}

/// Execute a codemod synchronously by blocking on the async runtime
///
/// This function wraps the async execution in a blocking call, making it
/// suitable for use in synchronous contexts like PostgreSQL extensions.
pub fn execute_codemod_sync<R>(
    options: InMemoryExecutionOptions<R>,
) -> Result<CodemodOutput, ExecutionError>
where
    R: ModuleResolver + 'static,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to create tokio runtime: {e}"),
            },
        })?;

    runtime.block_on(async { execute_codemod_in_memory(options).await })
}

/// Execute a codemod with simplified options (async version)
///
/// This function executes the codemod entirely in memory without filesystem access.
pub async fn execute_codemod_in_memory<R>(
    options: InMemoryExecutionOptions<'_, R>,
) -> Result<CodemodOutput, ExecutionError>
where
    R: ModuleResolver + 'static,
{
    let script_name = "__codemod_script.js";

    let mut resolver = InMemoryResolver::new();
    resolver.set_source(script_name.to_string(), options.codemod_source.to_string());
    let resolver_arc = Arc::new(resolver);

    let js_code = format!(
        include_str!("scripts/main_script.js.txt"),
        script_name = script_name
    );

    let params: HashMap<String, String> = options.params.unwrap_or_default();

    let runtime = AsyncRuntime::new().map_err(|e| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: format!("Failed to create AsyncRuntime: {e}"),
        },
    })?;

    let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let memory_limit = options.memory_limit.unwrap_or(DEFAULT_MEMORY_LIMIT);

    runtime.set_memory_limit(memory_limit).await;
    runtime.set_max_stack_size(DEFAULT_MAX_STACK_SIZE).await;
    let start_time = Instant::now();
    let timeout_exceeded = Arc::new(AtomicBool::new(false));
    let timeout_exceeded_clone = Arc::clone(&timeout_exceeded);

    runtime
        .set_interrupt_handler(Some(Box::new(move || {
            if start_time.elapsed().as_millis() as u64 > timeout_ms {
                timeout_exceeded_clone.store(true, Ordering::SeqCst);
                true // Interrupt execution
            } else {
                false // Continue execution
            }
        })))
        .await;

    // Use the pre-parsed AST from options (allows AST caching)
    let ast_grep = options.ast;

    let module_builder = LlrtModuleBuilder::build();
    let (mut built_in_resolver, mut built_in_loader, global_attachment) =
        module_builder.builder.build();

    built_in_resolver = built_in_resolver.add_name("codemod:ast-grep");
    built_in_loader = built_in_loader.with_module("codemod:ast-grep", AstGrepModule);

    built_in_resolver = built_in_resolver.add_name("codemod:metrics");
    built_in_loader = built_in_loader.with_module("codemod:metrics", MetricsModule);

    let in_memory_resolver = QuickJSResolver::new(Arc::clone(&resolver_arc));
    let noop_loader = InMemoryLoader::new(Arc::clone(&resolver_arc));

    runtime
        .set_loader(
            (built_in_resolver, in_memory_resolver),
            (built_in_loader, noop_loader),
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

    let timeout_exceeded_check = Arc::clone(&timeout_exceeded);

    let result = async_with!(context => |ctx| {
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

            let (evaluated, _) = module
                .eval()
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;

            while ctx.execute_pending_job() {}

            let namespace = evaluated
                .namespace()
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            let parsed_content =
                SgRootRjs::try_new_with_semantic(ast_grep, options.file_path.map(|p| p.to_string()), options.semantic_provider, options.file_path.map(|p| p.to_string())).map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            // Keep a reference to read rename_to after JS execution
            let sg_root_inner = Arc::clone(&parsed_content.inner);

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

            let language_str = options.language.to_string();

            // Convert String params to serde_json::Value for the shared helper
            let params_json = params.into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect::<HashMap<String, serde_json::Value>>();

            let run_options_qjs = build_transform_options(
                &ctx,
                params_json,
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

            process_transform_result(
                &result_obj,
                &sg_root_inner,
                ModificationCheck::Sha256(options.original_sha256),
            )
        };
        execution.await
    })
    .await;

    if timeout_exceeded_check.load(Ordering::SeqCst) {
        return Err(ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::ExecutionTimeout { timeout_ms },
        });
    }

    result.map(|primary| CodemodOutput {
        primary,
        secondary: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::errors::RuntimeError;
    use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
    use ast_grep_language::SupportLang;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn js_lang() -> CodemodLang {
        CodemodLang::Static(SupportLang::JavaScript)
    }

    fn compute_sha256(content: &str) -> Sha256Hash {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hasher.finalize().into()
    }

    #[test]
    fn test_execute_codemod_sync_timeout() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        // Create a codemod with an infinite loop
        let codemod_content = r#"
export default function transform(root) {
  while (true) {
    // Infinite loop to trigger timeout
  }
  return root.root().text();
}
        "#
        .trim();

        fs::write(temp_dir.path().join("timeout_codemod.js"), codemod_content)
            .expect("Failed to write codemod file");

        let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
        let content = "const x = 1;";
        let ast = AstGrep::new(content, js_lang());

        let result = execute_codemod_sync(InMemoryExecutionOptions {
            codemod_source: codemod_content,
            language: js_lang(),
            ast,
            original_sha256: Some(compute_sha256(content)),
            resolver: Some(resolver),
            selector_config: None,
            params: None,
            matrix_values: None,
            file_path: None,
            semantic_provider: None,
            metrics_context: None,
            timeout_ms: Some(50), // 50ms timeout for faster test
            memory_limit: None,
        });

        match result {
            Err(ExecutionError::Runtime {
                source: RuntimeError::ExecutionTimeout { timeout_ms },
            }) => {
                assert_eq!(timeout_ms, 50);
            }
            Ok(output) => panic!(
                "Expected timeout error, but got success: {:?}",
                output.primary
            ),
            Err(e) => panic!("Expected timeout error, got different error: {:?}", e),
        }
    }

    #[test]
    fn test_execute_codemod_sync_simple() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let codemod_content = r#"
export default function transform(root) {
  const rootNode = root.root();
  const nodes = rootNode.findAll({
    rule: { pattern: "console.log($ARG)" }
  });
  const edits = nodes.map(node => {
    const arg = node.getMatch("ARG").text();
    return node.replace(`logger.log(${arg})`);
  });
  return rootNode.commitEdits(edits);
}
        "#
        .trim();

        fs::write(temp_dir.path().join("test_codemod.js"), codemod_content)
            .expect("Failed to write codemod file");

        let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
        let content = "console.log('Hello, world!');";
        let ast = AstGrep::new(content, js_lang());

        let result = execute_codemod_sync(InMemoryExecutionOptions {
            codemod_source: codemod_content,
            language: js_lang(),
            ast,
            original_sha256: Some(compute_sha256(content)),
            resolver: Some(resolver),
            selector_config: None,
            params: None,
            matrix_values: None,
            file_path: None,
            semantic_provider: None,
            metrics_context: None,
            timeout_ms: None,
            memory_limit: None,
        });

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Modified(modified) => {
                    assert!(modified.content.contains("logger.log('Hello, world!')"));
                    assert!(modified.rename_to.is_none());
                }
                other => panic!("Expected modified result, got: {:?}", other),
            },
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }
}
