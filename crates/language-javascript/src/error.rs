//! Error types specific to JavaScript/TypeScript semantic analysis.

use language_core::SemanticError;
use std::path::PathBuf;
use thiserror::Error;

/// Errors specific to JavaScript/TypeScript semantic analysis.
#[derive(Error, Debug)]
pub enum JsSemanticError {
    /// OXC parser error
    #[error("Parse error in '{path}': {message}")]
    ParseError { path: PathBuf, message: String },

    /// Symbol not found at the given position
    #[error("No symbol found at byte offset {offset} in '{path}'")]
    SymbolNotFound { path: PathBuf, offset: u32 },

    /// Module resolution failed
    #[error("Failed to resolve module '{specifier}' from '{from_path}'")]
    ModuleResolution { specifier: String, from_path: PathBuf },

    /// File not in cache
    #[error("File '{path}' not in symbol cache")]
    FileNotCached { path: PathBuf },

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<JsSemanticError> for SemanticError {
    fn from(err: JsSemanticError) -> Self {
        match err {
            JsSemanticError::ParseError { path, message } => {
                SemanticError::ParseError { path, message }
            }
            JsSemanticError::SymbolNotFound { path, offset } => SemanticError::SymbolNotFound {
                path,
                line: 0,
                column: offset,
            },
            JsSemanticError::ModuleResolution {
                specifier,
                from_path,
            } => SemanticError::ModuleResolution {
                specifier,
                from_path,
                message: "Module not found".to_string(),
            },
            JsSemanticError::FileNotCached { path } => SemanticError::FileRead {
                path,
                message: "File not in symbol cache".to_string(),
            },
            JsSemanticError::Internal(msg) => SemanticError::Internal(msg),
        }
    }
}

