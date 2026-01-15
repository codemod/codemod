use super::execution_engine::ExecutionResult;
use super::quickjs_adapters::QuickJSResolver;
use crate::ast_grep::serde::JsValue;
use crate::ast_grep::sg_node::{SgNodeRjs, SgRootRjs};
use crate::ast_grep::AstGrepModule;
use crate::metrics::{MetricsContext, MetricsModule};
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::{InMemoryLoader, InMemoryResolver, ModuleResolver};
use crate::utils::quickjs_utils::maybe_promise;
use ast_grep_config::RuleConfig;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::AstGrep;
use ast_grep_language::SupportLang;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use language_core::SemanticProvider;
use rquickjs::{
    async_with, AsyncContext, AsyncRuntime, CatchResultExt, Function, IntoJs, Module, Object,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// In-memory execution options for executing a codemod on a string
pub struct InMemoryExecutionOptions<'a, R> {
    /// The JavaScript codemod source code (not a file path)
    pub codemod_source: &'a str,
    /// The programming language of the source code to transform
    pub language: SupportLang,
    /// The source code to transform
    pub content: &'a str,
    /// Optional module resolver (if None, a no-op resolver is used)
    pub resolver: Option<Arc<R>>,
    /// Optional selector config for pre-filtering
    pub selector_config: Option<Arc<Box<RuleConfig<SupportLang>>>>,
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
}

/// Execute a codemod synchronously by blocking on the async runtime
///
/// This function wraps the async execution in a blocking call, making it
/// suitable for use in synchronous contexts like PostgreSQL extensions.
pub fn execute_codemod_sync<R>(
    options: InMemoryExecutionOptions<R>,
) -> Result<ExecutionResult, ExecutionError>
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
) -> Result<ExecutionResult, ExecutionError>
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

    let ast_grep = AstGrep::new(options.content, options.language);

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

    async_with!(context => |ctx| {
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

            let run_options = Object::new(ctx.clone()).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;
            run_options.set("params", params).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: e.to_string(),
                },
            })?;
            run_options.set("language", &language_str).map_err(|e| ExecutionError::Runtime {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

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

        let result = execute_codemod_sync(InMemoryExecutionOptions {
            codemod_source: codemod_content,
            language: SupportLang::JavaScript,
            content,
            resolver: Some(resolver),
            selector_config: None,
            params: None,
            matrix_values: None,
            file_path: None,
            semantic_provider: None,
            metrics_context: None,
        });

        match result {
            Ok(ExecutionResult::Modified(new_content)) => {
                assert!(new_content.contains("logger.log('Hello, world!')"));
            }
            Ok(other) => panic!("Expected modified result, got: {:?}", other),
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }
}
