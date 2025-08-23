use super::quickjs_adapters::{QuickJSLoader, QuickJSResolver};
use crate::ast_grep::AstGrepModule;
use crate::sandbox::errors::ExecutionError;
use crate::sandbox::filesystem::FileSystem;
use crate::sandbox::resolvers::ModuleResolver;
#[cfg(feature = "native")]
use crate::tree_sitter::SupportedLanguage;
use llrt_modules::module_builder::ModuleBuilder;
use rquickjs::{async_with, AsyncContext, AsyncRuntime};
use rquickjs::{CatchResultExt, Function, Module};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Statistics about the execution results
#[derive(Debug, Default)]
pub struct ExecutionStats {
    pub files_modified: AtomicUsize,
    pub files_unmodified: AtomicUsize,
    pub files_with_errors: AtomicUsize,
}

impl ExecutionStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of files processed
    pub fn total_files(&self) -> usize {
        self.files_modified.load(Ordering::Relaxed)
            + self.files_unmodified.load(Ordering::Relaxed)
            + self.files_with_errors.load(Ordering::Relaxed)
    }

    /// Returns true if any files were processed successfully (modified or unmodified)
    pub fn has_successful_files(&self) -> bool {
        self.files_modified.load(Ordering::Relaxed) > 0
            || self.files_unmodified.load(Ordering::Relaxed) > 0
    }

    /// Returns true if any files had errors during processing
    pub fn has_errors(&self) -> bool {
        self.files_with_errors.load(Ordering::Relaxed) > 0
    }

    /// Returns the success rate as a percentage (0.0 to 1.0)
    pub fn success_rate(&self) -> f64 {
        let total = self.total_files();
        if total == 0 {
            0.0
        } else {
            (self.files_modified.load(Ordering::Relaxed)
                + self.files_unmodified.load(Ordering::Relaxed)) as f64
                / total as f64
        }
    }
}

impl fmt::Display for ExecutionStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Execution Summary: {} files processed ({} modified, {} unmodified, {} errors)",
            self.total_files(),
            self.files_modified.load(Ordering::Relaxed),
            self.files_unmodified.load(Ordering::Relaxed),
            self.files_with_errors.load(Ordering::Relaxed)
        )
    }
}

/// Result of executing a codemod on a single file
#[derive(Debug, Clone)]
pub enum ExecutionResult {
    Modified,
    Unmodified,
    Error(String),
}

/// Output of executing a codemod on content (in-memory)
#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    /// The transformed content, if any
    pub content: Option<String>,
    /// Whether the content was modified from the original
    pub modified: bool,
    /// Error message if execution failed
    pub error: Option<String>,
}

impl ExecutionOutput {
    /// Create a successful output with transformed content
    pub fn success(content: Option<String>, original_content: &str) -> Self {
        let modified = match &content {
            Some(new_content) => new_content != original_content,
            None => false,
        };

        Self {
            content,
            modified,
            error: None,
        }
    }

    /// Create an error output
    pub fn error(message: String) -> Self {
        Self {
            content: None,
            modified: false,
            error: Some(message),
        }
    }

    /// Check if the execution was successful
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }

    /// Check if the execution failed
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

/// Execute a codemod on string content using QuickJS
/// This is the core execution logic that doesn't touch the filesystem
#[cfg(feature = "native")]
pub async fn execute_codemod_with_quickjs<F, R>(
    script_path: &Path,
    _filesystem: Arc<F>,
    resolver: Arc<R>,
    language: SupportedLanguage,
    file_path: &Path,
    content: &str,
) -> Result<ExecutionOutput, ExecutionError>
where
    F: FileSystem,
    R: ModuleResolver + 'static,
{
    use crate::utils::quickjs_utils::maybe_promise;

    let script_name = script_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("main.js");

    let js_code = format!(
        include_str!("scripts/main_script.js.txt"),
        script_name = script_name
    );

    // Initialize QuickJS runtime and context
    let runtime = AsyncRuntime::new().map_err(|e| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: format!("Failed to create AsyncRuntime: {e}"),
        },
    })?;

    // Set up built-in modules
    let module_builder = ModuleBuilder::default();
    let (mut built_in_resolver, mut built_in_loader, global_attachment) = module_builder.build();

    // Add AstGrepModule
    built_in_resolver = built_in_resolver.add_name("codemod:ast-grep");
    built_in_loader = built_in_loader.with_module("codemod:ast-grep", AstGrepModule);

    let fs_resolver = QuickJSResolver::new(Arc::clone(&resolver));
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
    let result: Result<Option<String>, ExecutionError> = async_with!(context => |ctx| {
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

            // Set the current file path for the codemod
            let file_path_str = file_path.to_string_lossy();
            ctx.globals()
                .set("CODEMOD_TARGET_FILE_PATH", file_path_str.as_ref())
                .map_err(|e| ExecutionError::Runtime {
                    source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                        message: format!("Failed to set global variable: {e}"),
                    },
                })?;

            // Set the language for the codemod
            let language_str = language.to_string();
            ctx.globals()
                .set("CODEMOD_LANGUAGE", language_str)
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
                .get::<_, Function>("executeCodemod")
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

            if result_obj.is_string() {
                Ok(Some(result_obj.get::<String>().unwrap()))
            } else if result_obj.is_null() || result_obj.is_undefined() {
                Ok(None)
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
    .await;

    // Convert the result to ExecutionOutput
    match result {
        Ok(new_content) => Ok(ExecutionOutput::success(new_content, content)),
        Err(e) => {
            // Format the error message for better readability
            let error_msg = match &e {
                ExecutionError::Runtime { source } => {
                    match source {
                        crate::sandbox::errors::RuntimeError::InitializationFailed { message } => {
                            // Unescape newlines in JavaScript error messages
                            message.replace("\\n", "\n")
                        }
                        crate::sandbox::errors::RuntimeError::ExecutionFailed { message } => {
                            message.replace("\\n", "\n")
                        }
                        _ => e.to_string(),
                    }
                }
                _ => e.to_string(),
            };
            Ok(ExecutionOutput::error(error_msg))
        }
    }
}
