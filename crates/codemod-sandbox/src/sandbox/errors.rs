use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FsError {
    #[error("File not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Permission denied: {path}")]
    PermissionDenied { path: PathBuf },

    #[error("IO error: {message}")]
    Io { message: String },

    #[error("Path canonicalization failed: {path}")]
    CanonicalizationFailed { path: PathBuf },
}

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("Module resolution failed:\nCannot resolve {name} from {base}")]
    ResolutionFailed { base: String, name: String },

    #[error("Invalid module path: {path}")]
    InvalidPath { path: String },

    #[error("Module not found: {name}")]
    ModuleNotFound { name: String },
}

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("Module loading failed: {name}")]
    LoadingFailed { name: String },

    #[error("Module compilation failed: {name} - {message}")]
    CompilationFailed { name: String, message: String },

    #[error("File system error while loading module: {name}")]
    FileSystemError {
        name: String,
        #[source]
        source: FsError,
    },
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Runtime initialization failed: {message}")]
    InitializationFailed { message: String },

    #[error("JavaScript execution failed: {message}")]
    ExecutionFailed { message: String },

    #[error("Module resolution error")]
    ModuleResolution {
        #[source]
        source: ResolverError,
    },

    #[error("Module loading error")]
    ModuleLoading {
        #[source]
        source: LoaderError,
    },

    #[error("Context creation failed: {message}")]
    ContextCreationFailed { message: String },

    #[error("QuickJS error")]
    QuickJS(#[from] rquickjs::Error),

    #[error("Execution timeout: exceeded {timeout_ms}ms limit")]
    ExecutionTimeout { timeout_ms: u64 },

    #[error("Memory limit exceeded")]
    MemoryLimitExceeded,
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("File system error")]
    FileSystem {
        #[source]
        source: FsError,
    },

    #[error("Runtime error {source}")]
    Runtime {
        #[source]
        source: RuntimeError,
    },

    #[error("Configuration error: {message}")]
    Configuration { message: String },

    #[error("Thread execution failed: {message}")]
    ThreadExecution { message: String },

    #[error("No files found to process")]
    NoFilesFound,
}

// Convenience conversion implementations
impl From<FsError> for ExecutionError {
    fn from(err: FsError) -> Self {
        ExecutionError::FileSystem { source: err }
    }
}

impl From<RuntimeError> for ExecutionError {
    fn from(err: RuntimeError) -> Self {
        ExecutionError::Runtime { source: err }
    }
}

impl From<ResolverError> for RuntimeError {
    fn from(err: ResolverError) -> Self {
        RuntimeError::ModuleResolution { source: err }
    }
}

impl From<LoaderError> for RuntimeError {
    fn from(err: LoaderError) -> Self {
        RuntimeError::ModuleLoading { source: err }
    }
}

impl From<FsError> for LoaderError {
    fn from(err: FsError) -> Self {
        LoaderError::FileSystemError {
            name: "unknown".to_string(),
            source: err,
        }
    }
}
