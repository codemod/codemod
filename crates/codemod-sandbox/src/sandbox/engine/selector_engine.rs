use super::codemod_lang::CodemodLang;
use super::curated_fs::{CuratedFsConfig, CuratedFsModule, CuratedFsPromisesModule};
use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::AstGrepModule;
use crate::llm::{LlmModule, LlmRuntimeContext};
use crate::metrics::{MetricsContext, MetricsModule};
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::{InMemoryLoader, InMemoryResolver, ModuleResolver};
use crate::sandbox::runtime_module::{RuntimeHooksContext, RuntimeModule};
use crate::utils::quickjs_utils::maybe_promise;
use ast_grep_config::{RuleConfig, SerializableRuleConfig};
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use rquickjs::loader::Loader;
use rquickjs::{AsyncContext, AsyncRuntime, async_with};
use rquickjs::{CatchResultExt, Function, Module};
use rquickjs::{FromJs, IntoJs};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// Options for extracting a selector directly from a bundled codemod source.
/// This avoids materializing registry source as a temporary file.
pub struct InMemorySelectorEngineOptions<'a> {
    pub codemod_source: &'a str,
    pub language: CodemodLang,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    pub timeout_ms: Option<u64>,
    pub memory_limit: Option<usize>,
    pub cancellation_flag: Option<Arc<AtomicBool>>,
}

const DEFAULT_SELECTOR_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_SELECTOR_MEMORY_LIMIT: usize = 128 * 1024 * 1024;
const DEFAULT_SELECTOR_MAX_STACK_SIZE: usize = 4 * 1024 * 1024;

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

    extract_selector_with_loader(
        script_name,
        options.language,
        options.capabilities,
        options.target_directory,
        QuickJSResolver::new(Arc::clone(&options.resolver)),
        QuickJSLoader,
        DEFAULT_SELECTOR_TIMEOUT_MS,
        DEFAULT_SELECTOR_MEMORY_LIMIT,
        None,
    )
    .await
}

/// Extract a selector from an already-bundled codemod source without filesystem I/O.
pub async fn extract_selector_from_source(
    options: InMemorySelectorEngineOptions<'_>,
) -> Result<Option<Box<RuleConfig<CodemodLang>>>, ExecutionError> {
    let script_name = "__codemod_script.js";
    let mut resolver = InMemoryResolver::new();
    resolver.set_source(script_name.to_string(), options.codemod_source.to_string());
    let resolver = Arc::new(resolver);

    extract_selector_with_loader(
        script_name,
        options.language,
        options.capabilities,
        None,
        QuickJSResolver::new(Arc::clone(&resolver)),
        InMemoryLoader::new(resolver),
        options.timeout_ms.unwrap_or(DEFAULT_SELECTOR_TIMEOUT_MS),
        options
            .memory_limit
            .unwrap_or(DEFAULT_SELECTOR_MEMORY_LIMIT),
        options.cancellation_flag,
    )
    .await
}

/// Synchronous wrapper for hosts such as PostgreSQL extensions.
pub fn extract_selector_from_source_sync(
    options: InMemorySelectorEngineOptions<'_>,
) -> Result<Option<Box<RuleConfig<CodemodLang>>>, ExecutionError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to create tokio runtime: {e}"),
            },
        })?;

    runtime.block_on(extract_selector_from_source(options))
}

#[allow(clippy::too_many_arguments)]
async fn extract_selector_with_loader<R, L>(
    script_name: &str,
    language: CodemodLang,
    capabilities: Option<HashSet<LlrtSupportedModules>>,
    target_directory: Option<&Path>,
    module_resolver: QuickJSResolver<R>,
    module_loader: L,
    timeout_ms: u64,
    memory_limit: usize,
    cancellation_flag: Option<Arc<AtomicBool>>,
) -> Result<Option<Box<RuleConfig<CodemodLang>>>, ExecutionError>
where
    R: ModuleResolver + 'static,
    L: Loader + 'static,
{
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

    runtime.set_memory_limit(memory_limit).await;
    runtime
        .set_max_stack_size(DEFAULT_SELECTOR_MAX_STACK_SIZE)
        .await;
    let started_at = Instant::now();
    let timeout_exceeded = Arc::new(AtomicBool::new(false));
    let timeout_exceeded_for_interrupt = Arc::clone(&timeout_exceeded);
    let cancellation_observed = Arc::new(AtomicBool::new(false));
    let cancellation_observed_for_interrupt = Arc::clone(&cancellation_observed);
    let cancellation_flag_for_runtime = cancellation_flag.clone();
    runtime
        .set_interrupt_handler(Some(Box::new(move || {
            if cancellation_flag
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                cancellation_observed_for_interrupt.store(true, Ordering::SeqCst);
                return true;
            }
            if started_at.elapsed() >= Duration::from_millis(timeout_ms) {
                timeout_exceeded_for_interrupt.store(true, Ordering::SeqCst);
                return true;
            }
            false
        })))
        .await;

    // Track whether the caller opted into llrt's real-disk fs capability
    // so we know whether to install the curated fs below instead.
    let mut fs_capability_enabled = false;
    let llm_capability_enabled = capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.contains(&LlrtSupportedModules::Fetch));

    // Set up built-in modules
    let mut module_builder = LlrtModuleBuilder::build();
    if let Some(capabilities) = capabilities {
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

    // Selector extraction evaluates the codemod's complete module graph. Make
    // the default sandboxed fs import available even for bundled in-memory
    // sources, while keeping it isolated from the host filesystem.
    let curated_fs_config = if !fs_capability_enabled {
        Some(if let Some(target_directory) = target_directory {
            (
                target_directory.to_string_lossy().into_owned(),
                vfs::PhysicalFS::new(std::path::PathBuf::from("/")).into(),
            )
        } else {
            ("/__selector__".to_string(), vfs::MemoryFS::new().into())
        })
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

    // Selector extraction evaluates the codemod's module graph, so it must
    // expose the same runtime imports as normal codemod execution.
    built_in_resolver = built_in_resolver.add_name("codemod:runtime");
    built_in_loader = built_in_loader.with_module("codemod:runtime", RuntimeModule);

    if llm_capability_enabled {
        built_in_resolver = built_in_resolver.add_name("codemod:llm");
        built_in_loader = built_in_loader.with_module("codemod:llm", LlmModule);
    }

    // Register the curated `fs` / `fs/promises` modules when applicable.
    if curated_fs_config.is_some() {
        built_in_resolver = built_in_resolver.add_name("fs").add_name("fs/promises");
        built_in_loader = built_in_loader
            .with_module("fs", CuratedFsModule)
            .with_module("fs/promises", CuratedFsPromisesModule);
    }

    // Combine resolvers and loaders
    runtime
        .set_loader(
            (built_in_resolver, module_resolver),
            (built_in_loader, module_loader),
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
    let execution = async_with!(context => |ctx| {
        global_attachment.attach(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach global modules: {e}"),
            },
        })?;

        // Install the curated fs config (if applicable) before the codemod
        // module evaluates so its first `import "fs"` resolves cleanly.
        if let Some((target_dir, fs_root)) = curated_fs_config {
            ctx.store_userdata(CuratedFsConfig::new(target_dir, fs_root))
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
        ctx.store_userdata(LlmRuntimeContext::default())
            .map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to store LlmRuntimeContext: {:?}", e),
                },
            })?;
        ctx.store_userdata(RuntimeHooksContext::new(
            None,
            cancellation_flag_for_runtime,
        ))
            .map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to store RuntimeHooksContext: {:?}", e),
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
                .set("CODEMOD_LANGUAGE", language.to_string())
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
                    source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                        message: e.to_string(),
                    },
                }
            })?;
            let result_obj = maybe_promise(result_obj_promise)
                .await
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
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
    });
    let result = match tokio::time::timeout(Duration::from_millis(timeout_ms), execution).await {
        Ok(result) => result,
        Err(_) => {
            timeout_exceeded.store(true, Ordering::SeqCst);
            Err(crate::sandbox::errors::RuntimeError::ExecutionTimeout { timeout_ms }.into())
        }
    };

    if cancellation_observed.load(Ordering::SeqCst) {
        return Err(crate::sandbox::errors::RuntimeError::ExecutionCancelled.into());
    }
    if timeout_exceeded.load(Ordering::SeqCst) {
        return Err(crate::sandbox::errors::RuntimeError::ExecutionTimeout { timeout_ms }.into());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::engine::codemod_lang::CodemodLang;
    use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn in_memory_options(source: &str) -> InMemorySelectorEngineOptions<'_> {
        InMemorySelectorEngineOptions {
            codemod_source: source,
            language: "csharp".parse::<CodemodLang>().unwrap(),
            capabilities: None,
            timeout_ms: None,
            memory_limit: None,
            cancellation_flag: None,
        }
    }

    #[tokio::test]
    async fn selector_extraction_supports_bundled_source_without_a_file() {
        let result = extract_selector_from_source(in_memory_options(
            r#"
            import runtime from "codemod:runtime";
            runtime.setCurrentUnit("selector");

            export function getSelector() {
                return { rule: { kind: "invocation_expression" } };
            }
            export default function codemod() {}
            "#,
        ))
        .await
        .unwrap();

        assert!(result.is_some());
    }

    #[test]
    fn selector_extraction_supports_synchronous_hosts() {
        let result = extract_selector_from_source_sync(in_memory_options(
            r#"
            export function getSelector() {
                return { rule: { kind: "binary_expression" } };
            }
            export default function codemod() {}
            "#,
        ))
        .unwrap();

        assert!(result.is_some());
    }

    #[tokio::test]
    async fn selector_extraction_from_source_preserves_no_selector_behavior() {
        let result =
            extract_selector_from_source(in_memory_options("export default function codemod() {}"))
                .await
                .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn selector_extraction_from_source_supports_sandboxed_fs_imports() {
        let result = extract_selector_from_source(in_memory_options(
            r#"
            import { readdirSync } from "fs";

            export function getSelector() {
                return { rule: { kind: "constructor_declaration" } };
            }
            export default function codemod() {
                return readdirSync(".");
            }
            "#,
        ))
        .await
        .unwrap();

        assert!(result.is_some());
    }

    #[tokio::test]
    async fn selector_extraction_from_source_is_time_bounded() {
        let mut options = in_memory_options(
            r#"
            export function getSelector() {
                while (true) {}
            }
            export default function codemod() {}
            "#,
        );
        options.timeout_ms = Some(10);

        let error = match extract_selector_from_source(options).await {
            Ok(_) => panic!("an infinite selector should time out"),
            Err(error) => error,
        };

        assert!(
            error.to_string().contains("exceeded 10ms limit"),
            "expected a selector timeout, got: {error}"
        );
    }

    #[tokio::test]
    async fn selector_extraction_from_source_observes_cancellation() {
        let mut options = in_memory_options(
            r#"
            export function getSelector() {
                while (true) {}
            }
            export default function codemod() {}
            "#,
        );
        options.cancellation_flag = Some(Arc::new(AtomicBool::new(true)));

        let error = match extract_selector_from_source(options).await {
            Ok(_) => panic!("a cancelled selector should stop"),
            Err(error) => error,
        };

        assert_eq!(error.to_string(), "Runtime error Execution cancelled");
    }

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

    #[tokio::test]
    async fn selector_extraction_supports_runtime_imports() {
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("codemod.js");
        std::fs::write(
            &script_path,
            r#"
            import runtime, { progress } from "codemod:runtime";

            runtime.setCurrentUnit("selector");

            export const getSelector = () => {
                progress("building selector");
                return { rule: { kind: "binary_expression" } };
            };
            export default function codemod() {}
            "#,
        )
        .unwrap();

        let resolver = Arc::new(OxcResolver::new(dir.path().to_path_buf(), None).unwrap());
        let result = extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &script_path,
            language: "csharp".parse::<CodemodLang>().unwrap(),
            resolver,
            capabilities: None,
            target_directory: Some(dir.path()),
        })
        .await
        .unwrap();

        assert!(
            result.is_some(),
            "selector extraction should support codemod:runtime imports"
        );
    }
}
