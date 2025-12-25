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
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[cfg(feature = "native")]
use gag::BufferRedirect;

use crate::ast_grep::serde::JsValue;
use crate::workflow_global::WorkflowGlobalModule;

pub struct SelectorEngineOptions<'a, R> {
    pub script_path: &'a Path,
    pub language: SupportLang,
    pub resolver: Arc<R>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    pub console_log_collector: Option<Box<dyn FnMut(String) + Send + Sync>>,
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

    let fs_resolver = QuickJSResolver::new(Arc::clone(&options.resolver));
    let fs_loader = QuickJSLoader;

    // Combine resolvers and loaders
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
            // Capture stdout during JSSG execution
            // Note: This may fail in parallel execution contexts, so we handle it gracefully
            #[cfg(feature = "native")]
            let mut redirect = BufferRedirect::stdout().ok();

            let module = Module::declare(ctx.clone(), "__selector_extractor.js", js_code)
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

            let params_qjs = params.into_js(&ctx);

            ctx.globals()
                .set("CODEMOD_PARAMS", params_qjs)
                .map_err(|e| {
                    let error_msg = format!("Failed to set params global variable: {e}");
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

            ctx.globals()
                .set("CODEMOD_LANGUAGE", options.language.to_string())
                .map_err(|e| {
                    let error_msg = format!("Failed to set language global variable: {e}");
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

            let func = namespace
                .get::<_, Function>("runSelector")
                .catch(&ctx)
                .map_err(|e| {
                    let error_msg = e.to_string();
                    if let Some(ref collector) = console_log_collector {
                        if let Ok(mut collector) = collector.lock() {
                            collector(format!("ERROR: Failed to get runSelector function: {}", error_msg));
                        }
                    }
                    ExecutionError::Runtime {
                        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                            message: error_msg,
                        },
                    }
                })?;

            // Call it and return value.
            let result_obj_promise = func.call(()).catch(&ctx).map_err(|e| {
                let error_msg = e.to_string();
                if let Some(ref collector) = console_log_collector {
                    if let Ok(mut collector) = collector.lock() {
                        collector(format!("ERROR: Failed to call runSelector: {}", error_msg));
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

            if result_obj.is_null() || result_obj.is_undefined() {
                return Ok(None);
            }

            if result_obj.is_object() {
                // Convert the JavaScript object to a RuleConfig
                let js_value = JsValue::from_js(&ctx, result_obj)
                    .map_err(|e| {
                        let error_msg = format!("Failed to convert JS value: {e}");
                        if let Some(ref collector) = console_log_collector {
                            if let Ok(mut collector) = collector.lock() {
                                collector(format!("ERROR: {}", error_msg));
                            }
                        }
                        ExecutionError::Runtime {
                            source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                                message: error_msg,
                            },
                        }
                    })?;

                let serializable_config: SerializableRuleConfig<SupportLang> =
                    serde_json::from_value(js_value.0)
                        .map_err(|e| {
                            let error_msg = format!("Failed to deserialize rule config: {e}");
                            if let Some(ref collector) = console_log_collector {
                                if let Ok(mut collector) = collector.lock() {
                                    collector(format!("ERROR: {}", error_msg));
                                }
                            }
                            ExecutionError::Runtime {
                                source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                                    message: error_msg,
                                },
                            }
                        })?;

                let rule_config = RuleConfig::try_from(serializable_config, &Default::default())
                    .map_err(|e| {
                        let error_msg = format!("Failed to create RuleConfig: {e}");
                        if let Some(ref collector) = console_log_collector {
                            if let Ok(mut collector) = collector.lock() {
                                collector(format!("ERROR: {}", error_msg));
                            }
                        }
                        ExecutionError::Runtime {
                            source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                                message: error_msg,
                            },
                        }
                    })?;

                Ok(Some(Box::new(rule_config)))
            } else {
                let error_msg = "Invalid selector result type - expected object or null".to_string();
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
