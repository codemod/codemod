use super::codemod_lang::CodemodLang;
use super::curated_fs::{CuratedFsConfig, CuratedFsModule, CuratedFsPromisesModule};
use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::AstGrepModule;
use crate::metrics::{MetricsContext, MetricsModule};
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::ModuleResolver;
use crate::utils::quickjs_utils::maybe_promise;
use ast_grep_config::{RuleConfig, SerializableRuleConfig};
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
    pub language: CodemodLang,
    pub resolver: Arc<R>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    /// Directory that the curated `fs` module is constrained to. When
    /// `Some` and the caller hasn't opted into the llrt `Fs` capability,
    /// the codemod's `import "fs"` resolves to a [`CuratedFsModule`]
    /// backed by `vfs::PhysicalFS` at disk root `/`, with reads/writes
    /// prefix-checked against this path.
    pub target_directory: Option<&'a Path>,
}

/// Extract a selector from a codemod module using QuickJS
/// This executes the getSelector function and converts the result to RuleConfig
pub async fn extract_selector_with_quickjs<'a, R>(
    options: SelectorEngineOptions<'a, R>,
) -> Result<Option<Box<RuleConfig<CodemodLang>>>, ExecutionError>
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

    // Track whether the caller opted into llrt's real-disk fs capability
    // so we know whether to install the curated fs below instead.
    let mut fs_capability_enabled = false;

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
                    fs_capability_enabled = true;
                }
                LlrtSupportedModules::ChildProcess => {
                    module_builder.enable_child_process();
                }
                _ => {}
            }
        }
    }

    // If the caller provided a target_directory and didn't explicitly opt
    // into llrt's unrestricted fs, install the curated fs. The script sees
    // real on-disk paths; reads/writes outside `target_directory` are
    // rejected with `EACCES`.
    let curated_fs_target = if !fs_capability_enabled {
        options
            .target_directory
            .map(|p| p.to_string_lossy().into_owned())
    } else {
        None
    };

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

    // Register the curated `fs` / `fs/promises` modules when applicable.
    if curated_fs_target.is_some() {
        built_in_resolver = built_in_resolver.add_name("fs").add_name("fs/promises");
        built_in_loader = built_in_loader
            .with_module("fs", CuratedFsModule)
            .with_module("fs/promises", CuratedFsPromisesModule);
    }

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

        // Install the curated fs config (if applicable) before the codemod
        // module evaluates so its first `import "fs"` resolves cleanly.
        if let Some(target_dir) = curated_fs_target {
            let physical_root: vfs::VfsPath = vfs::PhysicalFS::new(std::path::PathBuf::from("/")).into();
            ctx.store_userdata(CuratedFsConfig::new(target_dir, physical_root))
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to store CuratedFsConfig: {:?}", e),
                    },
                })?;
        }

        // Selector extraction may evaluate the codemod's full module graph.
        // Install a disposable metrics context so modules that import
        // `codemod:metrics` can initialize without requiring transform execution.
        ctx.store_userdata(MetricsContext::new())
            .map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to store MetricsContext: {:?}", e),
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

                let serializable_config: SerializableRuleConfig<CodemodLang> =
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::engine::codemod_lang::CodemodLang;
    use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn selector_extraction_propagates_get_selector_errors() {
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("codemod.js");
        std::fs::write(
            &script_path,
            r#"
            export function getSelector() {
                throw new Error("selector exploded");
            }
            export default function codemod() {}
            "#,
        )
        .unwrap();

        let resolver = Arc::new(OxcResolver::new(dir.path().to_path_buf(), None).unwrap());
        let result = extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &script_path,
            language: "typescript".parse::<CodemodLang>().unwrap(),
            resolver,
            capabilities: None,
            target_directory: Some(dir.path()),
        })
        .await;

        let error = match result {
            Ok(_) => panic!("selector errors should fail extraction"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("selector exploded"),
            "expected selector error to propagate, got: {error}"
        );
    }

    #[tokio::test]
    async fn selector_extraction_supports_const_arrow_get_selector_exports() {
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("codemod.js");
        std::fs::write(
            &script_path,
            r#"
            export const getSelector = () => ({
                rule: { kind: "string_fragment" },
            });
            export default function codemod() {}
            "#,
        )
        .unwrap();

        let resolver = Arc::new(OxcResolver::new(dir.path().to_path_buf(), None).unwrap());
        let result = extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &script_path,
            language: "tsx".parse::<CodemodLang>().unwrap(),
            resolver,
            capabilities: None,
            target_directory: Some(dir.path()),
        })
        .await
        .unwrap();

        assert!(
            result.is_some(),
            "const arrow getSelector exports should be supported"
        );
    }

    #[tokio::test]
    async fn selector_extraction_supports_top_level_metrics_imports() {
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("codemod.js");
        std::fs::write(
            &script_path,
            r#"
            import { useMetricAtom } from "codemod:metrics";

            const selectorMetric = useMetricAtom("selector_metric");

            export const getSelector = () => {
                selectorMetric.increment();
                return { rule: { kind: "string_fragment" } };
            };
            export default function codemod() {}
            "#,
        )
        .unwrap();

        let resolver = Arc::new(OxcResolver::new(dir.path().to_path_buf(), None).unwrap());
        let result = extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &script_path,
            language: "tsx".parse::<CodemodLang>().unwrap(),
            resolver,
            capabilities: None,
            target_directory: Some(dir.path()),
        })
        .await
        .unwrap();

        assert!(
            result.is_some(),
            "selector extraction should support top-level metrics imports"
        );
    }
}
