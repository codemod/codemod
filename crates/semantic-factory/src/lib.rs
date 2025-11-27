//! Semantic analysis factory for creating language-specific providers.
//!
//! This crate provides a factory pattern for creating semantic analysis providers
//! based on the programming language and desired analysis scope. It abstracts
//! away the details of individual language implementations.
//!
//! # Features
//!
//! - **Factory pattern**: Create providers based on language without knowing implementation details
//! - **Lazy initialization**: Defer provider creation until first semantic operation
//! - **Configurable scope**: Choose between FileScope (single-file) and WorkspaceScope (cross-file)
//!
//! # Example
//!
//! ```
//! use semantic_factory::{SemanticFactory, SemanticConfig, LazySemanticProvider};
//! use language_core::SemanticProvider;
//! use std::path::PathBuf;
//!
//! // Option 1: Direct factory creation
//! let provider = SemanticFactory::create("typescript", SemanticConfig::file_scope());
//!
//! // Option 2: Lazy provider (recommended for most use cases)
//! let lazy_provider = LazySemanticProvider::file_scope();
//!
//! // Option 3: Workspace-scope for cross-file analysis
//! let workspace_provider = LazySemanticProvider::workspace_scope(PathBuf::from("/path/to/project"));
//! ```

mod config;
mod factory;
mod lazy;

pub use config::{SemanticConfig, SemanticScope};
pub use factory::SemanticFactory;
pub use lazy::LazySemanticProvider;

// Re-export core types for convenience
pub use language_core::{
    ByteRange, DefinitionResult, ProviderMode, ReferencesResult, SemanticProvider,
};
