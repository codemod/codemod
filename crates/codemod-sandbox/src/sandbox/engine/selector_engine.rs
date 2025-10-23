use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::AstGrepModule;
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::ModuleResolver;
use crate::utils::quickjs_utils::maybe_promise;
use ast_grep_config::{RuleConfig, SerializableRuleConfig};
use ast_grep_language::SupportLang;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use rquickjs::{async_with, AsyncContext, AsyncRuntime};
use rquickjs::{CatchResultExt, Function, Module};
use rquickjs::{FromJs, IntoJs};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::ast_grep::serde::JsValue;
use crate::workflow_global::WorkflowGlobalModule;

pub struct SelectorEngineOptions<'a, R> {
    pub script_path: &'a Path,
    pub language: SupportLang,
    pub resolver: Arc<R>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
}

/// Extract a selector from a codemod module using QuickJS
/// This executes the getSelector function and converts the result to RuleConfig
pub async fn extract_selector_with_quickjs<'a, R>(
    options: SelectorEngineOptions<'a, R>,
) -> Result<Option<Box<RuleConfig<SupportLang>>>, ExecutionError>
where
    R: ModuleResolver + 'static,
{
    let script_name = options
        .script_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("main.js");

    let js_code = format!(
        include_str!("scripts/extract_selector_script.js.txt"),
        script_name = script_name
    );

    // TODO: Add params to the codemod
    let params: HashMap<String, String> = HashMap::new();

    // Initialize QuickJS runtime and context
    let runtime = AsyncRuntime::new().map_err(|e| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: format!("Failed to create AsyncRuntime: {e}"),
        },
    })?;

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

    // Add WorkflowGlobalModule
    let workflow_global_path = std::env::temp_dir().join("codemod_workflow_global.txt");
    std::env::set_var("WORKFLOW_GLOBAL", &workflow_global_path);
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
            let module = Module::declare(ctx.clone(), "__selector_extractor.js", js_code)
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to declare module: {e}"),
                    },
                })?;

            let params_qjs = params.into_js(&ctx);

            ctx.globals()
                .set("CODEMOD_PARAMS", params_qjs)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to set params global variable: {e}"),
                    },
                })?;

            ctx.globals()
                .set("CODEMOD_LANGUAGE", options.language.to_string())
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to set language global variable: {e}"),
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

            let func = namespace
                .get::<_, Function>("runSelector")
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            // Call it and return value.
            let result_obj_promise = func.call(()).catch(&ctx).map_err(|e| {
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

            if result_obj.is_null() || result_obj.is_undefined() {
                return Ok(None);
            }

            if result_obj.is_object() {
                // Convert the JavaScript object to a RuleConfig
                let js_value = JsValue::from_js(&ctx, result_obj)
                    .map_err(|e| ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                            message: format!("Failed to convert JS value: {e}"),
                        },
                    })?;

                let serializable_config: SerializableRuleConfig<SupportLang> =
                    serde_json::from_value(js_value.0)
                        .map_err(|e| ExecutionError::Runtime {
                            source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                                message: format!("Failed to deserialize rule config: {e}"),
                            },
                        })?;

                let rule_config = RuleConfig::try_from(serializable_config, &Default::default())
                    .map_err(|e| ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                            message: format!("Failed to create RuleConfig: {e}"),
                        },
                    })?;

                Ok(Some(Box::new(rule_config)))
            } else {
                Err(ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                        message: "Invalid selector result type - expected object or null".to_string(),
                    },
                })
            }
        };
        execution.await
    })
    .await
}
