//! Python-specific semantic analysis errors.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during Python semantic analysis.
#[derive(Error, Debug)]
pub enum PySemanticError {
    /// Failed to parse Python file.
    #[error("Failed to parse Python file {path}: {message}")]
    ParseError { path: PathBuf, message: String },

    /// File not found in cache.
    #[error("File not found in cache: {path}")]
    FileNotCached { path: PathBuf },

    /// Failed to read file.
    #[error("Failed to read file {path}: {message}")]
    FileRead { path: PathBuf, message: String },

    /// Symbol not found at the given position.
    #[error("No symbol found at position {start}..{end} in {path}")]
    SymbolNotFound { path: PathBuf, start: u32, end: u32 },

    /// Module resolution failed.
    #[error("Failed to resolve module '{module}' from {path}")]
    ModuleResolution { module: String, path: PathBuf },

    /// Other error.
    #[error("{0}")]
    Other(String),
}

impl From<PySemanticError> for language_core::SemanticError {
    fn from(err: PySemanticError) -> Self {
        match err {
            PySemanticError::ParseError { path, message } => {
                language_core::SemanticError::ParseError { path, message }
            }
            PySemanticError::FileNotCached { path } => language_core::SemanticError::Internal(
                format!("File not cached: {}", path.display()),
            ),
            PySemanticError::FileRead { path, message } => {
                language_core::SemanticError::FileRead { path, message }
            }
            PySemanticError::SymbolNotFound {
                path,
                start,
                end: _,
            } => {
                // Convert byte positions to approximate line/column
                // For now, use start as line and 0 as column
                language_core::SemanticError::SymbolNotFound {
                    path,
                    line: start,
                    column: 0,
                }
            }
            PySemanticError::ModuleResolution { module, path } => {
                language_core::SemanticError::ModuleResolution {
                    specifier: module,
                    from_path: path,
                    message: "Module not found".to_string(),
                }
            }
            PySemanticError::Other(msg) => language_core::SemanticError::Internal(msg),
        }
    }
}

pub type PySemanticResult<T> = Result<T, PySemanticError>;
