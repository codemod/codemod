//! Python semantic analysis provider using Ruff's ty_ide.
//!
//! This crate provides semantic analysis capabilities for Python
//! by leveraging Ruff's ty_ide crate for goto-definition and
//! find-references functionality.
//!
//! # Features
//!
//! - Symbol definition lookup (go-to-definition)
//! - Reference finding (find-all-references)
//! - Two analysis modes: FileScope (single-file) and WorkspaceScope (workspace-wide)
//!
//! # Example
//!
//! ```no_run
//! use language_python::RuffSemanticProvider;
//! use language_core::{SemanticProvider, ByteRange, ProviderMode, DefinitionOptions};
//! use std::path::Path;
//!
//! // Create a file-scope provider for single-file analysis
//! let provider = RuffSemanticProvider::file_scope();
//!
//! // Or create a workspace-scope provider for workspace-wide analysis
//! // let provider = RuffSemanticProvider::workspace_scope(workspace_root);
//!
//! // Notify the provider about processed files to build the symbol cache
//! provider.notify_file_processed(
//!     Path::new("src/utils.py"),
//!     "def add(a, b):\n    return a + b"
//! ).unwrap();
//!
//! // Query for definitions, references, etc.
//! let definition = provider.get_definition(
//!     Path::new("src/main.py"),
//!     ByteRange::new(10, 13), // byte range of "add" reference
//!     DefinitionOptions::default()
//! ).unwrap();
//! ```

mod analyzer;
mod db;
mod error;
mod provider;

pub use error::PySemanticError;
pub use provider::RuffSemanticProvider;

// Re-export core types for convenience
pub use language_core::{ByteRange, ProviderMode, SemanticProvider, SymbolKind, SymbolLocation};
