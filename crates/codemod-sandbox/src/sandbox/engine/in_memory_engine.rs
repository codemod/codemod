use super::codemod_lang::CodemodLang;
use super::curated_fs::{CuratedFsConfig, CuratedFsModule, CuratedFsPromisesModule, FileFetcher};
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
use crate::workflow_global::{SharedStateContext, WorkflowGlobalModule};
use ast_grep_config::RuleConfig;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::AstGrep;
use codemod_llrt_capabilities::module_builder::LlrtModuleBuilder;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use language_core::SemanticProvider;
use rquickjs::{async_with, AsyncContext, AsyncRuntime, CatchResultExt, Function, Module};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use vfs::VfsPath;

/// Default execution timeout in milliseconds (180s)
const DEFAULT_TIMEOUT_MS: u64 = 180000;

/// Default memory limit in bytes (512 MB)
const DEFAULT_MEMORY_LIMIT: usize = 512 * 1024 * 1024;

/// Default max stack size in bytes (4 MB)
const DEFAULT_MAX_STACK_SIZE: usize = 4 * 1024 * 1024;

/// SHA256 hash type (32 bytes)
pub type Sha256Hash = [u8; 32];

/// Sandboxed values for the QuickJS `process` global.
#[derive(Clone, Debug, Default)]
pub struct ProcessSandbox {
    /// Values exposed on `process.env`.
    pub env: HashMap<String, String>,
    /// Value returned by `process.cwd()`.
    pub cwd: String,
}

/// Curated filesystem sandbox. When attached to
/// [`InMemoryExecutionOptions::fs_sandbox`], the codemod's `fs` import is
/// backed by `root` and constrained to paths beneath `target_dir`.
///
/// When the caller instead opts the codemod into the `Fs` llrt capability,
/// llrt's real-disk fs is used and this option is ignored.
#[derive(Clone)]
pub struct FsSandbox {
    /// Absolute prefix that the codemod is allowed to read/write. Paths that
    /// normalize outside this prefix are rejected with `EACCES`.
    pub target_dir: String,
    /// Backing VFS. For pg_ast_grep this is a pre-populated `MemoryFS`; for
    /// CLI runs it would be a `PhysicalFS` rooted at `/`.
    pub root: VfsPath,
    /// Optional fallback consulted on VFS miss. Typically used by
    /// pg_ast_grep to lazily pull sibling files from Postgres so a read of
    /// a shared config that isn't pre-loaded in the per-file MemoryFS
    /// succeeds instead of returning `ENOENT`.
    pub fetcher: Option<Arc<dyn FileFetcher>>,
}

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
    /// Optional shared state context for cross-thread state communication
    pub shared_state_context: Option<SharedStateContext>,
    /// Execution timeout in milliseconds (default: 200ms)
    pub timeout_ms: Option<u64>,
    /// Memory limit in bytes (default: 64 MB)
    pub memory_limit: Option<usize>,
    /// Optional sandboxed values for the `process` global.
    ///
    /// When `Some`, the llrt `process` module is omitted from the runtime and
    /// a minimal stub exposing only `env` and `cwd()` is installed instead.
    /// When `None`, the host-derived defaults from llrt_modules remain in
    /// place.
    pub process_sandbox: Option<ProcessSandbox>,
    /// Optional curated filesystem sandbox.
    ///
    /// When `Some`, a curated `fs` / `fs/promises` module backed by the
    /// provided `VfsPath` is registered and constrained to `target_dir`.
    /// When `None`, no fs module is provided (codemods that `import "fs"`
    /// will fail to resolve unless the caller separately enables the llrt
    /// `Fs` capability).
    pub fs_sandbox: Option<FsSandbox>,
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

    // When the caller asks for a process sandbox, omit the llrt `process`
    // module so the host env, cwd, argv, exit, setuid, etc. are never attached
    // in the first place; we install a restricted stub below instead.
    let module_builder = if options.process_sandbox.is_some() {
        LlrtModuleBuilder::build_with_exclusions(&[LlrtSupportedModules::Process])
    } else {
        LlrtModuleBuilder::build()
    };
    let (mut built_in_resolver, mut built_in_loader, global_attachment) =
        module_builder.builder.build();

    built_in_resolver = built_in_resolver.add_name("codemod:ast-grep");
    built_in_loader = built_in_loader.with_module("codemod:ast-grep", AstGrepModule);

    built_in_resolver = built_in_resolver.add_name("codemod:metrics");
    built_in_loader = built_in_loader.with_module("codemod:metrics", MetricsModule);

    built_in_resolver = built_in_resolver.add_name("codemod:workflow");
    built_in_loader = built_in_loader.with_module("codemod:workflow", WorkflowGlobalModule);

    // Register the curated fs module as `fs` / `fs/promises` when the caller
    // asked for a curated sandbox. The config is installed into userdata
    // below so the module can find it on first import.
    if options.fs_sandbox.is_some() {
        built_in_resolver = built_in_resolver.add_name("fs").add_name("fs/promises");
        built_in_loader = built_in_loader
            .with_module("fs", CuratedFsModule)
            .with_module("fs/promises", CuratedFsPromisesModule);
    }

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

    // Capture metrics context and shared state context for use inside async block
    let metrics_context = options.metrics_context.clone();
    let shared_state_context = options.shared_state_context.clone();
    let process_sandbox = options.process_sandbox.clone();
    let fs_sandbox = options.fs_sandbox.clone();

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

        // Always store a SharedStateContext so codemod:workflow functions work
        ctx.store_userdata(shared_state_context.unwrap_or_default()).map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: format!("Failed to store SharedStateContext: {:?}", e),
            },
        })?;

        // Install the curated fs config before the codemod module evaluates
        // so its first `import "fs"` resolves against the right root.
        if let Some(sandbox) = fs_sandbox {
            let mut cfg = CuratedFsConfig::new(sandbox.target_dir, sandbox.root);
            if let Some(fetcher) = sandbox.fetcher {
                cfg = cfg.with_fetcher(fetcher);
            }
            ctx.store_userdata(cfg).map_err(|e| ExecutionError::Runtime {
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

        if let Some(sandbox) = process_sandbox {
            let env_json = serde_json::to_string(&sandbox.env).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to serialize process.env sandbox: {e}"),
                },
            })?;
            let cwd_json = serde_json::to_string(&sandbox.cwd).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to serialize process.cwd sandbox: {e}"),
                },
            })?;
            // The llrt `process` module was excluded from the module builder
            // above, so `globalThis.process` is currently undefined. Install a
            // minimal stub that exposes only the sandboxed `env` and `cwd`.
            let script = format!(
                "globalThis.process = {{ env: {env_json}, cwd: function() {{ return {cwd_json}; }} }};"
            );
            ctx.eval::<(), _>(script).catch(&ctx).map_err(|e| ExecutionError::Runtime {
                source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                    message: format!("Failed to install process sandbox stub: {e}"),
                },
            })?;
        }

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
                true,
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
            shared_state_context: None,
            timeout_ms: Some(50), // 50ms timeout for faster test
            memory_limit: None,
            process_sandbox: None,
            fs_sandbox: None,
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
            shared_state_context: None,
            timeout_ms: None,
            memory_limit: None,
            process_sandbox: None,
            fs_sandbox: None,
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

    #[test]
    fn test_process_sandbox_overrides_env_and_cwd() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        // Codemod emits cwd, env keys, the sorted list of `process` keys, and
        // whether a few dangerous llrt-provided properties are absent. When
        // sandboxed, only `env` + `cwd` should survive; `exit`, `argv`,
        // `platform` etc. should be undefined since the llrt process module
        // is no longer attached.
        let codemod_content = r#"
export default function transform(root) {
  const envKeys = Object.keys(process.env).sort().join(",");
  const processKeys = Object.keys(process).sort().join(",");
  const stripped = [
    typeof process.exit,
    typeof process.argv,
    typeof process.platform,
    typeof process.versions,
  ].join(",");
  return `cwd=${process.cwd()};env=${envKeys};keys=${processKeys};stripped=${stripped}`;
}
        "#
        .trim();

        fs::write(temp_dir.path().join("sandbox_codemod.js"), codemod_content)
            .expect("Failed to write codemod file");

        // Seed a host env var that must not leak into process.env.
        std::env::set_var("PG_SG_SANDBOX_LEAK_CHECK", "should-not-appear");

        let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
        let content = "const x = 1;";
        let ast = AstGrep::new(content, js_lang());

        let sandbox = ProcessSandbox {
            env: HashMap::new(),
            cwd: "/app/".to_string(),
        };

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
            shared_state_context: None,
            timeout_ms: None,
            memory_limit: None,
            process_sandbox: Some(sandbox),
            fs_sandbox: None,
        });

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Modified(modified) => {
                    assert_eq!(
                        modified.content,
                        "cwd=/app/;env=;keys=cwd,env;stripped=undefined,undefined,undefined,undefined",
                    );
                }
                other => panic!("Expected modified result, got: {:?}", other),
            },
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }

    /// Build a MemoryFS-backed sandbox that contains the target file plus a
    /// sibling the codemod can try to read/write. Returns the VfsPath root.
    fn build_memory_fs_with_files(files: &[(&str, &str)]) -> vfs::VfsPath {
        let root: vfs::VfsPath = vfs::MemoryFS::new().into();
        for (path, content) in files {
            if let Some(parent) = std::path::Path::new(path).parent() {
                let p = parent.to_string_lossy();
                if !p.is_empty() {
                    let parent_vfs = root.join(p.trim_start_matches('/')).unwrap();
                    let _ = parent_vfs.create_dir_all();
                }
            }
            let file = root.join(path.trim_start_matches('/')).unwrap();
            let mut w = file.create_file().unwrap();
            use std::io::Write;
            w.write_all(content.as_bytes()).unwrap();
        }
        root
    }

    /// Curated fs must expose allowed files to the sandbox while rejecting
    /// paths that escape `target_dir`.
    #[test]
    fn test_fs_sandbox_allows_read_and_rejects_escape() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        // Codemod emits a summary of fs behaviour:
        //   (1) successful read of a sibling inside target_dir
        //   (2) error `.code` when reading outside target_dir
        //   (3) error `.code` when reading a missing file inside target_dir
        let codemod_content = r#"
import fs from "fs";
export default function transform(root) {
  const ok = fs.readFileSync("/app/sibling.ts", "utf-8");
  let escape = "none";
  try {
    fs.readFileSync("/etc/passwd", "utf-8");
  } catch (e) {
    escape = e.code;
  }
  let missing = "none";
  try {
    fs.readFileSync("/app/missing.ts", "utf-8");
  } catch (e) {
    missing = e.code;
  }
  return `ok=${ok};escape=${escape};missing=${missing}`;
}
        "#
        .trim();

        fs::write(temp_dir.path().join("fs_codemod.js"), codemod_content)
            .expect("Failed to write codemod file");

        let root = build_memory_fs_with_files(&[
            ("/app/main.ts", "const x = 1;"),
            ("/app/sibling.ts", "export const y = 2;"),
        ]);
        let fs_sandbox = FsSandbox {
            target_dir: "/app".to_string(),
            root: root.clone(),
            fetcher: None,
        };

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
            file_path: Some("/app/main.ts"),
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            timeout_ms: None,
            memory_limit: None,
            process_sandbox: None,
            fs_sandbox: Some(fs_sandbox),
        });

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Modified(modified) => {
                    assert_eq!(
                        modified.content,
                        "ok=export const y = 2;;escape=EACCES;missing=ENOENT",
                    );
                }
                other => panic!("Expected modified result, got: {:?}", other),
            },
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }

    /// Mirror pg_ast_grep's batch flow: run the same fs-using codemod across
    /// many files sequentially (one tokio runtime + one QuickJS runtime per
    /// file, just like execute_codemod_sync on each Rayon worker). If the fs
    /// sandbox path leaves behind poisoned global state (panic handler,
    /// tokio internals, rquickjs userdata bookkeeping), later files fail
    /// even though the first-file tests above pass.
    #[test]
    fn test_fs_sandbox_batch_like_pg_ast_grep() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let codemod_content = r#"
import fs from "fs";
export default function transform(root) {
  const content = fs.readFileSync(root.filename(), "utf-8");
  return content + "\n// touched";
}
        "#
        .trim();

        fs::write(temp_dir.path().join("batch_codemod.js"), codemod_content)
            .expect("Failed to write codemod file");

        let files: Vec<(String, String)> = (0..16)
            .map(|i| {
                (
                    format!("/app/src/file_{i}.ts"),
                    format!("export const v{i} = {i};"),
                )
            })
            .collect();

        let mut modified = 0usize;
        for (idx, (path, content)) in files.iter().enumerate() {
            let root = build_memory_fs_with_files(&[(path.as_str(), content.as_str())]);
            let fs_sandbox = FsSandbox {
                target_dir: "/app".to_string(),
                root,
                fetcher: None,
            };
            let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());
            let ast = AstGrep::new(content.as_str(), js_lang());
            let out = execute_codemod_sync(InMemoryExecutionOptions {
                codemod_source: codemod_content,
                language: js_lang(),
                ast,
                original_sha256: Some(compute_sha256(content)),
                resolver: Some(resolver),
                selector_config: None,
                params: None,
                matrix_values: None,
                file_path: Some(path.as_str()),
                semantic_provider: None,
                metrics_context: None,
                shared_state_context: None,
                timeout_ms: None,
                memory_limit: None,
                process_sandbox: None,
                fs_sandbox: Some(fs_sandbox),
            })
            .unwrap_or_else(|e| panic!("iteration {idx} failed: {e:?}"));
            match out.primary {
                ExecutionResult::Modified(m) => {
                    assert!(
                        m.content.ends_with("// touched"),
                        "iteration {idx} produced unexpected content: {:?}",
                        m.content
                    );
                    modified += 1;
                }
                other => panic!("iteration {idx} expected modified, got {other:?}"),
            }
        }
        assert_eq!(modified, files.len());
    }

    /// `writeFileSync` inside target_dir must land in the backing VFS and be
    /// visible to subsequent reads; writes outside target_dir must fail.
    #[test]
    fn test_fs_sandbox_write_round_trip() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let codemod_content = r#"
import fs from "fs";
export default function transform(root) {
  fs.writeFileSync("/app/out.ts", "export const z = 3;", "utf-8");
  const back = fs.readFileSync("/app/out.ts", "utf-8");
  let denied = "none";
  try {
    fs.writeFileSync("/tmp/escape.ts", "leaked", "utf-8");
  } catch (e) {
    denied = e.code;
  }
  return `back=${back};denied=${denied}`;
}
        "#
        .trim();

        fs::write(temp_dir.path().join("fs_write_codemod.js"), codemod_content)
            .expect("Failed to write codemod file");

        let root = build_memory_fs_with_files(&[("/app/main.ts", "const x = 1;")]);
        let fs_sandbox = FsSandbox {
            target_dir: "/app".to_string(),
            root: root.clone(),
            fetcher: None,
        };

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
            file_path: Some("/app/main.ts"),
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            timeout_ms: None,
            memory_limit: None,
            process_sandbox: None,
            fs_sandbox: Some(fs_sandbox),
        });

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Modified(modified) => {
                    assert_eq!(modified.content, "back=export const z = 3;;denied=EACCES",);
                    // Verify the write actually landed in the MemoryFS (not
                    // just a runtime illusion) by reading through the VFS.
                    let written = root.join("app/out.ts").unwrap();
                    let mut buf = String::new();
                    use std::io::Read;
                    written
                        .open_file()
                        .unwrap()
                        .read_to_string(&mut buf)
                        .unwrap();
                    assert_eq!(buf, "export const z = 3;");
                }
                other => panic!("Expected modified result, got: {:?}", other),
            },
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }

    /// Hand-crafted fetcher that records every call for test assertions.
    /// Returns Some(bytes) for keys in the preloaded map, None otherwise.
    struct RecordingFetcher {
        files: std::collections::HashMap<String, Vec<u8>>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingFetcher {
        fn new(entries: &[(&str, &str)]) -> Self {
            let files = entries
                .iter()
                .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
                .collect();
            Self {
                files,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl crate::sandbox::engine::curated_fs::FileFetcher for RecordingFetcher {
        fn fetch(&self, path: &str) -> std::result::Result<Option<Vec<u8>>, String> {
            self.calls.lock().unwrap().push(path.to_string());
            Ok(self.files.get(path).cloned())
        }
    }

    /// When a readFileSync misses the VFS, the configured fetcher should
    /// fill it in, and subsequent reads of the same path should hit the
    /// VFS cache (the fetcher is called once even across repeated reads).
    #[test]
    fn test_fs_sandbox_fetcher_fills_missing_and_caches() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let codemod_content = r#"
import fs from "fs";
export default function transform(root) {
  const first = fs.readFileSync("/app/shared/env.ts", "utf-8");
  const second = fs.readFileSync("/app/shared/env.ts", "utf-8");
  return `first=${first};second=${second}`;
}
        "#
        .trim();

        fs::write(temp_dir.path().join("fetcher_codemod.js"), codemod_content)
            .expect("Failed to write codemod file");

        // Only the target file is pre-seeded; env.ts comes from the fetcher.
        let root = build_memory_fs_with_files(&[("/app/main.ts", "const x = 1;")]);
        let fetcher = Arc::new(RecordingFetcher::new(&[(
            "/app/shared/env.ts",
            "export const ENV = 'prod';",
        )]));
        let fs_sandbox = FsSandbox {
            target_dir: "/app".to_string(),
            root: root.clone(),
            fetcher: Some(fetcher.clone()),
        };

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
            file_path: Some("/app/main.ts"),
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            timeout_ms: None,
            memory_limit: None,
            process_sandbox: None,
            fs_sandbox: Some(fs_sandbox),
        });

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Modified(modified) => {
                    assert_eq!(
                        modified.content,
                        "first=export const ENV = 'prod';;second=export const ENV = 'prod';",
                    );
                }
                other => panic!("Expected modified result, got: {:?}", other),
            },
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
        // Fetcher called exactly once — second read served from the VFS
        // after the first read wrote the bytes back.
        assert_eq!(fetcher.call_count(), 1, "fetcher should be called once");
    }

    /// A fetcher returning `Ok(None)` must surface as `ENOENT`; returning
    /// `Err` must surface as `EIO`.
    #[test]
    fn test_fs_sandbox_fetcher_none_is_enoent_err_is_eio() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let codemod_content = r#"
import fs from "fs";
export default function transform(root) {
  let missing = "none";
  try {
    fs.readFileSync("/app/shared/missing.ts", "utf-8");
  } catch (e) {
    missing = e.code;
  }
  let failing = "none";
  try {
    fs.readFileSync("/app/shared/boom.ts", "utf-8");
  } catch (e) {
    failing = e.code;
  }
  return `missing=${missing};failing=${failing}`;
}
        "#
        .trim();

        fs::write(
            temp_dir.path().join("fetcher_err_codemod.js"),
            codemod_content,
        )
        .expect("Failed to write codemod file");

        struct FailingFetcher;
        impl crate::sandbox::engine::curated_fs::FileFetcher for FailingFetcher {
            fn fetch(&self, path: &str) -> std::result::Result<Option<Vec<u8>>, String> {
                if path.ends_with("/boom.ts") {
                    Err("storage unavailable".to_string())
                } else {
                    Ok(None)
                }
            }
        }

        let root = build_memory_fs_with_files(&[("/app/main.ts", "const x = 1;")]);
        let fs_sandbox = FsSandbox {
            target_dir: "/app".to_string(),
            root,
            fetcher: Some(Arc::new(FailingFetcher)),
        };

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
            file_path: Some("/app/main.ts"),
            semantic_provider: None,
            metrics_context: None,
            shared_state_context: None,
            timeout_ms: None,
            memory_limit: None,
            process_sandbox: None,
            fs_sandbox: Some(fs_sandbox),
        });

        match result {
            Ok(output) => match output.primary {
                ExecutionResult::Modified(modified) => {
                    assert_eq!(modified.content, "missing=ENOENT;failing=EIO");
                }
                other => panic!("Expected modified result, got: {:?}", other),
            },
            Err(e) => panic!("Expected success, got error: {:?}", e),
        }
    }
}
