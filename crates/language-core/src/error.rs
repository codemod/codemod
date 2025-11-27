//! Error types for semantic analysis.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during semantic analysis.
#[derive(Error, Debug)]
pub enum SemanticError {
    /// File could not be read
    #[error("Failed to read file '{path}': {message}")]
    FileRead { path: PathBuf, message: String },

    /// File could not be parsed
    #[error("Failed to parse file '{path}': {message}")]
    ParseError { path: PathBuf, message: String },

    /// Symbol not found at the given position
    #[error("No symbol found at position {line}:{column} in '{path}'")]
    SymbolNotFound {
        path: PathBuf,
        line: u32,
        column: u32,
    },

    /// Module resolution failed
    #[error("Failed to resolve module '{specifier}' from '{from_path}': {message}")]
    ModuleResolution {
        specifier: String,
        from_path: PathBuf,
        message: String,
    },

    /// Language not supported
    #[error("Language '{language}' is not supported by this provider")]
    UnsupportedLanguage { language: String },

    /// Provider not configured
    #[error("Semantic provider is not configured")]
    ProviderNotConfigured,

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Generic IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for semantic operations.
pub type SemanticResult<T> = Result<T, SemanticError>;
