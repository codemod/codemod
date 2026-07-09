use super::codemod_lang::CodemodLang;
use super::curated_fs::{
    normalize_virtual_absolute_path, CuratedFsConfig, CuratedFsModule, CuratedFsPromisesModule,
    FileFetcher,
};
use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use super::transform_helpers::{
    build_transform_options, process_transform_result, ModificationCheck,
};
use crate::ast_grep::sg_node::{SgNodeRjs, SgRootRjs};
use crate::ast_grep::AstGrepModule;
use crate::metrics::{MetricsContext, MetricsModule};
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::resolvers::ModuleResolver;
use crate::sandbox::runtime_module::{
    RuntimeEvent, RuntimeEventCallback, RuntimeEventKind, RuntimeHooksContext, RuntimeModule,
};
use crate::utils::quickjs_utils::maybe_promise;
use crate::workflow_global::{SharedStateContext, WorkflowGlobalModule};
use ast_grep_config::RuleConfig;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::AstGrep;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use language_core::SemanticProvider;
use rquickjs::prelude::Rest;
use rquickjs::{async_with, AsyncContext, AsyncRuntime, Ctx, Object, Type, Value};
use rquickjs::{CatchResultExt, Function, Module};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
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

#[derive(Debug, Clone, Copy)]
pub struct DryRunExecutionFlag(pub bool);

unsafe impl<'js> rquickjs::JsLifetime<'js> for DryRunExecutionFlag {
    type Changed<'to> = DryRunExecutionFlag;
}

fn install_console_bridge(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let console = Object::new(ctx.clone())?;
    console.set("log", Function::new(ctx.clone(), console_log)?)?;
    console.set("info", Function::new(ctx.clone(), console_log)?)?;
    console.set("debug", Function::new(ctx.clone(), console_log)?)?;
    console.set("warn", Function::new(ctx.clone(), console_warn)?)?;
    console.set("error", Function::new(ctx.clone(), console_warn)?)?;
    ctx.globals().set("console", console)?;
    Ok(())
}

fn console_log<'js>(ctx: Ctx<'js>, items: Rest<Value<'js>>) -> rquickjs::Result<()> {
    emit_console_event(ctx, RuntimeEventKind::Progress, items)
}

fn console_warn<'js>(ctx: Ctx<'js>, items: Rest<Value<'js>>) -> rquickjs::Result<()> {
    emit_console_event(ctx, RuntimeEventKind::Warn, items)
}

fn emit_console_event<'js>(
    ctx: Ctx<'js>,
    kind: RuntimeEventKind,
    items: Rest<Value<'js>>,
) -> rquickjs::Result<()> {
    let runtime_hooks_context = ctx
        .userdata::<RuntimeHooksContext>()
        .ok_or_else(|| rquickjs::Exception::throw_message(&ctx, "RuntimeHooksContext not found"))?
        .clone();
    runtime_hooks_context.emit(RuntimeEvent {
        kind,
        message: console_items_to_string(&ctx, items)?,
        meta: Some("console".to_string()),
    });
    Ok(())
}

fn console_items_to_string<'js>(
    ctx: &Ctx<'js>,
    items: Rest<Value<'js>>,
) -> rquickjs::Result<String> {
    let mut parts = Vec::new();
    for item in items.0 {
        parts.push(console_value_to_string(ctx, item)?);
    }
    Ok(parts.join(" "))
}

fn console_value_to_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<String> {
    match value.type_of() {
        Type::Uninitialized | Type::Undefined => Ok("undefined".to_string()),
        Type::Null => Ok("null".to_string()),
        Type::Bool => Ok(value.as_bool().unwrap_or_default().to_string()),
        Type::Int => Ok(value.as_int().unwrap_or_default().to_string()),
        Type::Float => Ok(value.as_float().unwrap_or_default().to_string()),
        Type::String => value
            .as_string()
            .map(|value| value.to_string())
            .transpose()
            .map(|value| value.unwrap_or_default()),
        _ => Ok(ctx
            .json_stringify(value)?
            .map(|json| json.to_string())
            .transpose()?
            .unwrap_or_else(|| "<unprintable>".to_string())),
    }
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

/// The target directory that the codemod is running against.
/// Stored as QuickJS userdata so `jssgTransform` and `rename()` can
/// validate that file paths stay within this directory.
#[derive(Debug, Clone)]
pub struct TargetDirectory(pub PathBuf);

unsafe impl<'js> rquickjs::JsLifetime<'js> for TargetDirectory {
    type Changed<'to> = TargetDirectory;
}

/// Validate that `path` resolves within the target directory stored in QuickJS userdata.
/// `caller` is used in error messages (e.g. "jssgTransform()" or "rename()").
/// If the file doesn't exist yet (e.g. rename target), the parent directory is canonicalized instead.
/// Returns `Ok(())` if no `TargetDirectory` userdata is set (e.g. test / in-memory contexts).
pub fn validate_path_within_target<'js>(
    ctx: &rquickjs::Ctx<'js>,
    path: &Path,
    caller: &str,
) -> rquickjs::Result<()> {
    if let Some(target_dir) = ctx.userdata::<TargetDirectory>() {
        let canonical_target = target_dir
            .0
            .canonicalize()
            .unwrap_or_else(|_| target_dir.0.clone());
        let canonical_path = path.canonicalize().unwrap_or_else(|_| {
            // File may not exist yet (e.g. rename target); canonicalize the parent instead
            if let Some(parent) = path.parent() {
                let canonical_parent = parent
                    .canonicalize()
                    .unwrap_or_else(|_| parent.to_path_buf());
                canonical_parent.join(path.file_name().unwrap_or_default())
            } else {
                path.to_path_buf()
            }
        });
        if !canonical_path.starts_with(&canonical_target) {
            return Err(rquickjs::Exception::throw_message(
                ctx,
                &format!(
                    "{} path '{}' is outside the target directory '{}'",
                    caller,
                    path.display(),
                    target_dir.0.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Options for executing a codemod on a single file
pub struct JssgExecutionOptions<'a, R> {
    pub script_path: &'a Path,
    pub resolver: Arc<R>,
    pub language: CodemodLang,
    pub file_path: &'a Path,
    pub content: &'a str,
    pub selector_config: Option<Arc<RuleConfig<CodemodLang>>>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub matrix_values: Option<HashMap<String, serde_json::Value>>,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    /// Optional semantic provider for symbol indexing (go-to-definition, find-references)
    pub semantic_provider: Option<Arc<dyn SemanticProvider>>,
    /// Optional metrics context for tracking metrics across execution
    pub metrics_context: Option<MetricsContext>,
    /// Optional shared state context for cross-thread state communication
    pub shared_state_context: Option<SharedStateContext>,
    /// Optional runtime event callback for codemod:runtime hook emissions
    pub runtime_event_callback: Option<RuntimeEventCallback>,
    /// Optional cancellation flag exposed to codemod:runtime.isCanceled()
    pub cancellation_flag: Option<Arc<AtomicBool>>,
    /// Whether this is a test execution (jssgTransform becomes a no-op)
    pub test_mode: bool,
    /// Whether this is a dry-run execution (passed to codemod via options.dryRun)
    pub dry_run: bool,
    /// The target directory the codemod is running against.
    /// Used to validate that `jssgTransform` and `rename()` only access files within this directory.
    pub target_directory: &'a Path,
}

struct DryRunDiskFetcher {
    target_directory: PathBuf,
}

impl FileFetcher for DryRunDiskFetcher {
    fn fetch(&self, path: &str) -> std::result::Result<Option<Vec<u8>>, String> {
        let candidate = self.candidate(path);
        match std::fs::read(candidate) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn metadata(&self, path: &str) -> std::result::Result<Option<vfs::VfsMetadata>, String> {
        let candidate = self.candidate(path);
        match std::fs::metadata(candidate) {
            Ok(meta) => Ok(Some(vfs::VfsMetadata {
                file_type: if meta.is_dir() {
                    vfs::VfsFileType::Directory
                } else {
                    vfs::VfsFileType::File
                },
                len: meta.len(),
                created: meta.created().ok(),
                modified: meta.modified().ok(),
                accessed: meta.accessed().ok(),
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn read_dir(&self, path: &str) -> std::result::Result<Option<Vec<String>>, String> {
        let candidate = self.candidate(path);
        match std::fs::read_dir(candidate) {
            Ok(entries) => entries
                .map(|entry| {
                    entry
                        .map_err(|error| error.to_string())
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                })
                .collect::<std::result::Result<Vec<_>, _>>()
                .map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl DryRunDiskFetcher {
    fn candidate(&self, path: &str) -> PathBuf {
        let path = normalize_virtual_absolute_path(path);
        let target = normalize_virtual_absolute_path(&self.target_directory.to_string_lossy());
        let relative = path
            .strip_prefix(&target)
            .unwrap_or(&path)
            .trim_start_matches('/');
        self.target_directory.join(relative)
    }
}

fn seed_dry_run_current_file(
    root: &vfs::VfsPath,
    _target_directory: &Path,
    file_path: &Path,
    content: &str,
) {
    let file_path = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.to_path_buf());
    let relative = normalize_virtual_absolute_path(&file_path.to_string_lossy());
    if let Some(parent) = Path::new(&relative).parent() {
        let parent = parent.to_string_lossy();
        if !parent.is_empty() {
            if let Ok(parent_vfs) = root.join(parent.trim_start_matches('/')) {
                let _ = parent_vfs.create_dir_all();
            }
        }
    }
    if let Ok(file) = root.join(relative.trim_start_matches('/')) {
        if let Ok(mut writer) = file.create_file() {
            let _ = writer.write_all(content.as_bytes());
        }
    }
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
    let canonical_target_directory = options
        .target_directory
        .canonicalize()
        .unwrap_or_else(|_| options.target_directory.to_path_buf());

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
                LlrtSupportedModules::Fs if !options.dry_run => {
                    module_builder.enable_fs();
                    fs_capability_enabled = true;
                }
                LlrtSupportedModules::Fs => {}
                LlrtSupportedModules::ChildProcess if !options.dry_run => {
                    module_builder.enable_child_process();
                }
                LlrtSupportedModules::ChildProcess => {}
                _ => {}
            }
        }
    }

    // Install the curated `fs` when the caller gave us a target directory
    // and didn't explicitly opt into the unrestricted llrt fs.
    let curated_fs_target = if !fs_capability_enabled {
        Some(canonical_target_directory.to_string_lossy().into_owned())
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

    // Add RuntimeModule (progress/failure hooks)
    built_in_resolver = built_in_resolver.add_name("codemod:runtime");
    built_in_loader = built_in_loader.with_module("codemod:runtime", RuntimeModule);

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

    // Capture metrics context and shared state context for use inside async block
    let metrics_context = options.metrics_context.clone();
    let shared_state_context = options.shared_state_context.clone();
    let runtime_hooks_context = RuntimeHooksContext::new(
        options.runtime_event_callback.clone(),
        options.cancellation_flag.clone(),
    );
    let test_mode = options.test_mode;

    // Execute JavaScript code
    async_with!(context => |ctx| {
        // Store execution mode flag in runtime userdata
        ctx.store_userdata(ExecutionModeFlag { test_mode }).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store ExecutionModeFlag: {:?}", e),
            },
        })?;

        ctx.store_userdata(DryRunExecutionFlag(options.dry_run)).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store DryRunExecutionFlag: {:?}", e),
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

        // Store target directory in runtime userdata if provided
        ctx.store_userdata(TargetDirectory(options.target_directory.to_path_buf())).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store TargetDirectory: {:?}", e),
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

        // Always store a SharedStateContext so codemod:workflow functions work
        ctx.store_userdata(shared_state_context.unwrap_or_default()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store SharedStateContext: {:?}", e),
            },
        })?;

        ctx.store_userdata(runtime_hooks_context.clone()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store RuntimeHooksContext: {:?}", e),
            },
        })?;

        if let Some(target_dir) = curated_fs_target.clone() {
            let cfg = if options.dry_run {
                let target_path = std::path::PathBuf::from(&target_dir);
                let memory_root: vfs::VfsPath = vfs::MemoryFS::new().into();
                seed_dry_run_current_file(
                    &memory_root,
                    &target_path,
                    options.file_path,
                    options.content,
                );
                CuratedFsConfig::new(target_dir.clone(), memory_root).with_fetcher(Arc::new(
                    DryRunDiskFetcher {
                        target_directory: target_path,
                    },
                ))
            } else {
                let physical_root: vfs::VfsPath = vfs::PhysicalFS::new(std::path::PathBuf::from("/")).into();
                // PhysicalFS is rooted at "/", so `target_dir` is already the
                // host-disk path corresponding to the VFS target. Hand it through
                // so the resolver can reject paths that traverse a symlink.
                CuratedFsConfig::new(target_dir.clone(), physical_root)
                    .with_physical_target_dir(std::path::PathBuf::from(&target_dir))
            };
            ctx.store_userdata(cfg)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to store CuratedFsConfig: {:?}", e),
                    },
                })?;
        }

        global_attachment.attach(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach global modules: {e}"),
            },
        })?;
        install_console_bridge(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach console bridge: {e}"),
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
            let (evaluated, eval_value) = module
                .eval()
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

            maybe_promise(eval_value.into())
                .await
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

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

            let file_path_str = options
                .file_path
                .canonicalize()
                .unwrap_or_else(|_| options.file_path.to_path_buf())
                .to_string_lossy()
                .to_string();
            let target_directory = canonical_target_directory.clone();
            let parsed_content =
                SgRootRjs::try_new_with_semantic(
                    ast_grep,
                    Some(file_path_str.clone()),
                    options.semantic_provider.clone(),
                    Some(file_path_str), // Pass current file path for write() validation
                    Some(target_directory.as_path()),
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
            let target_dir_str = target_directory.to_string_lossy().into_owned();

            let run_options_qjs = build_transform_options(
                &ctx,
                params,
                &language_str,
                options.matrix_values,
                matches,
                options.dry_run,
                &target_dir_str,
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
                map_transform_execution_error(&runtime_hooks_context, e)
            })?;
            let result_obj = maybe_promise(result_obj_promise)
                .await
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

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

fn map_transform_execution_error(
    runtime_hooks_context: &RuntimeHooksContext,
    error: impl std::fmt::Display,
) -> ExecutionError {
    if let Some(source) = runtime_hooks_context.take_pending_failure() {
        ExecutionError::RuntimeHook { source }
    } else {
        ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                message: error.to_string(),
            },
        }
    }
}

/// Options for executing a standalone JavaScript file
pub struct SimpleJsExecutionOptions<'a, R> {
    pub script_path: &'a Path,
    pub resolver: Arc<R>,
    /// Optional metrics context for tracking metrics across execution
    pub metrics_context: Option<MetricsContext>,
    /// Optional shared state context for cross-thread state communication
    pub shared_state_context: Option<SharedStateContext>,
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

    // Add WorkflowGlobalModule (step outputs)
    built_in_resolver = built_in_resolver.add_name("codemod:workflow");
    built_in_loader = built_in_loader.with_module("codemod:workflow", WorkflowGlobalModule);

    built_in_resolver = built_in_resolver.add_name("codemod:metrics");
    built_in_loader = built_in_loader.with_module("codemod:metrics", MetricsModule);

    built_in_resolver = built_in_resolver.add_name("codemod:runtime");
    built_in_loader = built_in_loader.with_module("codemod:runtime", RuntimeModule);

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
    let shared_state_context = options.shared_state_context.clone();
    let runtime_hooks_context = RuntimeHooksContext::default();

    // Execute JavaScript code
    async_with!(context => |ctx| {
        if let Some(ref metrics_ctx) = metrics_context {
            ctx.store_userdata(metrics_ctx.clone()).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to store MetricsContext: {:?}", e),
                },
            })?;
        }

        // Always store a SharedStateContext so codemod:workflow functions work
        ctx.store_userdata(shared_state_context.unwrap_or_default()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store SharedStateContext: {:?}", e),
            },
        })?;

        ctx.store_userdata(runtime_hooks_context.clone()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store RuntimeHooksContext: {:?}", e),
            },
        })?;

        global_attachment.attach(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach global modules: {e}"),
            },
        })?;
        install_console_bridge(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach console bridge: {e}"),
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
            let (_, eval_value) = module
                .eval()
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

            // Await the module evaluation promise to surface errors from top-level await
            maybe_promise(eval_value.into())
                .await
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

            Ok(())
        };
        execution.await
    })
    .await
}

/// Options for executing a shard function in QuickJS
pub struct ShardFunctionOptions<'a, R> {
    pub script_path: &'a Path,
    pub resolver: Arc<R>,
    /// The shard input data passed to the function
    pub input: serde_json::Value,
    /// Optional capabilities to gate module access (fetch, fs, child_process).
    /// When `None`, no extra modules are enabled.
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    /// Directory the curated `fs` module is constrained to. When `Some`
    /// and the caller hasn't opted into the llrt `Fs` capability, the
    /// shard function's `import "fs"` resolves to the curated fs backed
    /// by `vfs::PhysicalFS` at disk root `/`, prefix-checked against this
    /// path.
    pub target_directory: Option<&'a Path>,
}

/// Execute a shard function with QuickJS and return the result as JSON.
///
/// The user's script must export a default function that receives a `ShardInput`
/// object and returns a `ShardResult[]` array. The engine handles all file I/O
/// and serialization — the function only needs to handle grouping logic.
pub async fn execute_shard_function_with_quickjs<'a, R>(
    options: ShardFunctionOptions<'a, R>,
) -> Result<serde_json::Value, ExecutionError>
where
    R: ModuleResolver + 'static,
{
    use crate::ast_grep::serde::JsValue;
    use rquickjs::{FromJs, IntoJs};

    let script_name = options
        .script_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("shard.js");

    let js_code = format!(
        include_str!("scripts/shard_script.js.txt"),
        script_name = script_name
    );

    let runtime = AsyncRuntime::new().map_err(|e| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: format!("Failed to create AsyncRuntime: {e}"),
        },
    })?;

    let mut fs_capability_enabled = false;
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

    let curated_fs_target = if !fs_capability_enabled {
        options
            .target_directory
            .map(|p| p.to_string_lossy().into_owned())
    } else {
        None
    };

    let (mut built_in_resolver, mut built_in_loader, global_attachment) =
        module_builder.builder.build();

    built_in_resolver = built_in_resolver.add_name("codemod:ast-grep");
    built_in_loader = built_in_loader.with_module("codemod:ast-grep", AstGrepModule);

    built_in_resolver = built_in_resolver.add_name("codemod:workflow");
    built_in_loader = built_in_loader.with_module("codemod:workflow", WorkflowGlobalModule);

    built_in_resolver = built_in_resolver.add_name("codemod:metrics");
    built_in_loader = built_in_loader.with_module("codemod:metrics", MetricsModule);

    built_in_resolver = built_in_resolver.add_name("codemod:runtime");
    built_in_loader = built_in_loader.with_module("codemod:runtime", RuntimeModule);

    if curated_fs_target.is_some() {
        built_in_resolver = built_in_resolver.add_name("fs").add_name("fs/promises");
        built_in_loader = built_in_loader
            .with_module("fs", CuratedFsModule)
            .with_module("fs/promises", CuratedFsPromisesModule);
    }

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

    let input_value = options.input;
    let runtime_hooks_context = RuntimeHooksContext::default();

    async_with!(context => |ctx| {
        // Store a default SharedStateContext so codemod:workflow functions work
        ctx.store_userdata(SharedStateContext::default()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store SharedStateContext: {:?}", e),
            },
        })?;

        ctx.store_userdata(runtime_hooks_context.clone()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store RuntimeHooksContext: {:?}", e),
            },
        })?;

        if let Some(target_dir) = curated_fs_target.clone() {
            let physical_root: vfs::VfsPath = vfs::PhysicalFS::new(std::path::PathBuf::from("/")).into();
            // See the selector-config callsite above: PhysicalFS is rooted
            // at "/", so target_dir already names the host path and can be
            // reused for the symlink-safe check.
            let cfg = CuratedFsConfig::new(target_dir.clone(), physical_root)
                .with_physical_target_dir(std::path::PathBuf::from(&target_dir));
            ctx.store_userdata(cfg)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to store CuratedFsConfig: {:?}", e),
                    },
                })?;
        }

        global_attachment.attach(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach global modules: {e}"),
            },
        })?;
        install_console_bridge(&ctx).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to attach console bridge: {e}"),
            },
        })?;

        let execution = async {
            let entry_module_path = options
                .script_path
                .parent()
                .unwrap_or(Path::new("."))
                .join("__codemod_shard_entry.js");
            let entry_module_name = entry_module_path
                .to_str()
                .unwrap_or("__codemod_shard_entry.js");

            let module = Module::declare(ctx.clone(), entry_module_name, js_code)
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to declare module: {e}"),
                    },
                })?;

            let (evaluated, eval_value) = module
                .eval()
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

            // Await the module evaluation promise to surface errors from top-level await
            maybe_promise(eval_value.into())
                .await
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

            let namespace = evaluated
                .namespace()
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            // Convert serde_json::Value input to QuickJS value
            let input_js = JsValue(input_value).into_js(&ctx).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to convert shard input to JS: {e}"),
                },
            })?;

            let func = namespace
                .get::<_, Function>("executeShard")
                .catch(&ctx)
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: e.to_string(),
                    },
                })?;

            let result_promise = func
                .call((input_js,))
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

            let result_value = maybe_promise(result_promise)
                .await
                .catch(&ctx)
                .map_err(|e| map_transform_execution_error(&runtime_hooks_context, e))?;

            // Convert QuickJS result back to serde_json::Value
            let result_json: JsValue = JsValue::from_js(&ctx, result_value).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                    message: format!("Failed to convert shard result from JS: {e}"),
                },
            })?;

            Ok(result_json.0)
        };
        execution.await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
    use crate::sandbox::runtime_module::{RuntimeEvent, RuntimeEventKind, RuntimeFailureKind};
    use ast_grep_language::SupportLang;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
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

    #[test]
    fn dry_run_uses_curated_fs_virtual_path_normalization() {
        assert_eq!(
            normalize_virtual_absolute_path(r"c:\repo\src\foo.cs"),
            "/C:/repo/src/foo.cs"
        );
        assert_eq!(
            normalize_virtual_absolute_path("C:/repo/src/foo.cs"),
            "/C:/repo/src/foo.cs"
        );
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
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
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
    async fn test_dry_run_curated_fs_does_not_mutate_disk() {
        let codemod_content = r#"
import fs from "fs";
import { parseFile } from "codemod:ast-grep";

export default function transform(root, options) {
  const currentExistsBeforeRead = fs.existsSync(root.filename());
  const currentSizeBeforeRead = fs.statSync(root.filename()).size;
  const entriesBeforeRead = fs.readdirSync(options.targetDir).join(",");
  const siblingPath = `${options.targetDir}/Sibling.js`;
  const siblingExistsBeforeRead = fs.existsSync(siblingPath);
  const siblingSizeBeforeRead = fs.statSync(siblingPath).size;
  fs.unlinkSync(siblingPath);
  const siblingExistsAfterUnlink = fs.existsSync(siblingPath);
  const current = fs.readFileSync(root.filename(), "utf-8");
  fs.writeFileSync(`${options.targetDir}/appsettings.json`, '{"dryRun":true}\n', "utf-8");
  const written = fs.readFileSync(`${options.targetDir}/appsettings.json`, "utf-8");
  parseFile("javascript", `${options.targetDir}/Sibling.js`).write("export const sibling = 2;");
  fs.unlinkSync(root.filename());
  const currentExistsAfterUnlink = fs.existsSync(root.filename());
  return [
    `currentExistsBeforeRead=${currentExistsBeforeRead}`,
    `currentSizeBeforeRead=${currentSizeBeforeRead}`,
    `entriesBeforeRead=${entriesBeforeRead}`,
    `siblingExistsBeforeRead=${siblingExistsBeforeRead}`,
    `siblingSizeBeforeRead=${siblingSizeBeforeRead}`,
    `siblingExistsAfterUnlink=${siblingExistsAfterUnlink}`,
    `current=${current}`,
    `written=${written.trim()}`,
    `currentExistsAfterUnlink=${currentExistsAfterUnlink}`,
  ].join(";");
}
        "#
        .trim();
        let (temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let target_dir = temp_dir.path().join("repo");
        fs::create_dir_all(&target_dir).expect("target dir should be created");
        let file_path = target_dir.join("App.js");
        fs::write(&file_path, "const config = true;").expect("fixture should be written");
        let sibling_path = target_dir.join("Sibling.js");
        fs::write(&sibling_path, "export const sibling = 1;").expect("sibling should be written");

        let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: js_lang(),
            file_path: &file_path,
            content: "const config = true;",
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: true,
            target_directory: &target_dir,
        };

        let result = execute_codemod_with_quickjs(options)
            .await
            .expect("dry-run execution should succeed");
        match result.primary {
            ExecutionResult::Modified(modified) => {
                assert_eq!(
                    modified.content,
                    concat!(
                        "currentExistsBeforeRead=true;",
                        "currentSizeBeforeRead=20;",
                        "entriesBeforeRead=App.js,Sibling.js;",
                        "siblingExistsBeforeRead=true;",
                        "siblingSizeBeforeRead=25;",
                        "siblingExistsAfterUnlink=false;",
                        "current=const config = true;;",
                        "written={\"dryRun\":true};",
                        "currentExistsAfterUnlink=false"
                    )
                );
            }
            other => panic!("Expected modified result, got: {:?}", other),
        }

        assert_eq!(
            fs::read_to_string(&file_path).expect("current file should remain on disk"),
            "const config = true;"
        );
        assert!(
            !target_dir.join("appsettings.json").exists(),
            "dry-run fs writes must not create files on disk"
        );
        assert_eq!(
            fs::read_to_string(&sibling_path).expect("root.write target should remain on disk"),
            "export const sibling = 1;"
        );
    }

    #[tokio::test]
    async fn test_transform_options_include_target_dir_and_relative_filename() {
        let codemod_content = r#"
export default function transform(root, options) {
  return JSON.stringify({
    filename: root.filename(),
    relativeFilename: root.relativeFilename(),
    targetDir: options.targetDir ?? null,
  });
}
        "#
        .trim();
        let (temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let target_dir = temp_dir.path().join("repo");
        let file_path = target_dir.join("src/test.js");
        fs::create_dir_all(file_path.parent().unwrap()).expect("Failed to create target directory");

        let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: js_lang(),
            file_path: &file_path,
            content: "const x = 1;",
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: &target_dir,
        };

        let result = execute_codemod_with_quickjs(options)
            .await
            .expect("execution should succeed");
        match result.primary {
            ExecutionResult::Modified(modified) => {
                let payload: serde_json::Value =
                    serde_json::from_str(&modified.content).expect("transform should return JSON");
                let canonical_target_dir = target_dir
                    .canonicalize()
                    .expect("target dir should canonicalize");
                let canonical_file_path = file_path
                    .canonicalize()
                    .unwrap_or_else(|_| file_path.clone());
                assert_eq!(
                    payload["targetDir"],
                    canonical_target_dir.to_string_lossy().as_ref()
                );
                assert_eq!(payload["relativeFilename"], "src/test.js");
                assert_eq!(
                    payload["filename"],
                    canonical_file_path.to_string_lossy().as_ref()
                );
            }
            other => panic!("Expected modified result, got: {:?}", other),
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
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
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
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
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
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
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
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
        };

        let result = execute_codemod_with_quickjs(options).await;

        match result {
            Err(ExecutionError::Runtime { source }) => {
                assert!(source
                    .to_string()
                    .contains("must return either a string or null/undefined"));
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
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
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
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
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

    #[tokio::test]
    async fn test_execute_codemod_emits_runtime_progress_events() {
        let codemod_content = r#"
import runtime from "codemod:runtime";

export default function transform(root) {
  runtime.progress("Preparing transform");
  runtime.warn("Non-fatal warning", { kind: "warning" });
  return null;
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());
        let events = Arc::new(Mutex::new(Vec::<RuntimeEvent>::new()));
        let events_for_callback = Arc::clone(&events);
        let runtime_event_callback: RuntimeEventCallback = Arc::new(move |event| {
            events_for_callback
                .lock()
                .expect("events lock should succeed")
                .push(event);
        });

        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: js_lang(),
            file_path: Path::new("test.js"),
            content: "console.log('hello');",
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            runtime_event_callback: Some(runtime_event_callback),
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
        };

        let result = execute_codemod_with_quickjs(options).await;
        assert!(result.is_ok());

        let events = events.lock().expect("events lock should succeed");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, RuntimeEventKind::Progress);
        assert_eq!(events[0].message, "Preparing transform");
        assert_eq!(events[1].kind, RuntimeEventKind::Warn);
        assert_eq!(events[1].message, "Non-fatal warning");
        assert_eq!(events[1].meta.as_deref(), Some("{\"kind\":\"warning\"}"));
    }

    #[tokio::test]
    async fn test_execute_codemod_fail_step_surfaces_runtime_hook_error() {
        let codemod_content = r#"
import runtime from "codemod:runtime";

export default function transform(root) {
  runtime.failStep("Boom", { code: "step_failed" });
  return null;
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());

        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: js_lang(),
            file_path: Path::new("test.js"),
            content: "console.log('hello');",
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
        };

        let result = execute_codemod_with_quickjs(options).await;
        match result {
            Err(ExecutionError::RuntimeHook { source }) => {
                assert_eq!(source.kind, RuntimeFailureKind::Step);
                assert_eq!(source.message, "Boom");
                assert_eq!(source.meta.as_deref(), Some("{\"code\":\"step_failed\"}"));
            }
            other => panic!("Expected runtime hook error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_codemod_top_level_fail_step_surfaces_runtime_hook_error() {
        let codemod_content = r#"
import runtime from "codemod:runtime";

runtime.failStep("Init failed", { code: "init_failed" });

export default function transform(root) {
  return null;
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());

        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: js_lang(),
            file_path: Path::new("test.js"),
            content: "console.log('hello');",
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
        };

        let result = execute_codemod_with_quickjs(options).await;
        match result {
            Err(ExecutionError::RuntimeHook { source }) => {
                assert_eq!(source.kind, RuntimeFailureKind::Step);
                assert_eq!(source.message, "Init failed");
                assert_eq!(source.meta.as_deref(), Some("{\"code\":\"init_failed\"}"));
            }
            other => panic!("Expected runtime hook error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_codemod_fail_file_surfaces_runtime_hook_error() {
        let codemod_content = r#"
import runtime from "codemod:runtime";

export default async function transform(root) {
  await Promise.resolve();
  runtime.failFile("Bad file", { file: "test.js" });
  return null;
}
        "#
        .trim();

        let (_temp_dir, codemod_path) = setup_test_codemod(codemod_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());

        let options = JssgExecutionOptions {
            script_path: &codemod_path,
            resolver,
            language: js_lang(),
            file_path: Path::new("test.js"),
            content: "console.log('hello');",
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: Path::new("."),
        };

        let result = execute_codemod_with_quickjs(options).await;
        match result {
            Err(ExecutionError::RuntimeHook { source }) => {
                assert_eq!(source.kind, RuntimeFailureKind::File);
                assert_eq!(source.message, "Bad file");
                assert_eq!(source.meta.as_deref(), Some("{\"file\":\"test.js\"}"));
            }
            other => panic!("Expected runtime hook error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_shard_fail_step_surfaces_runtime_hook_error() {
        let shard_content = r#"
import runtime from "codemod:runtime";

export default function shard(input) {
  runtime.failStep("Shard failed", { shard: "team-a" });
  return [];
}
        "#
        .trim();

        let (_temp_dir, shard_path) = setup_test_codemod(shard_content);
        let resolver = Arc::new(OxcResolver::new(_temp_dir.path().to_path_buf(), None).unwrap());

        let options = ShardFunctionOptions {
            script_path: &shard_path,
            resolver,
            input: serde_json::json!({
                "files": ["test.js"],
                "params": {},
                "state": {}
            }),
            capabilities: None,
            target_directory: Some(Path::new(".")),
        };

        let result = execute_shard_function_with_quickjs(options).await;
        match result {
            Err(ExecutionError::RuntimeHook { source }) => {
                assert_eq!(source.kind, RuntimeFailureKind::Step);
                assert_eq!(source.message, "Shard failed");
                assert_eq!(source.meta.as_deref(), Some("{\"shard\":\"team-a\"}"));
            }
            other => panic!("Expected runtime hook error, got: {:?}", other),
        }
    }
}
