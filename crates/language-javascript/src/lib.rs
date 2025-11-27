//! JavaScript/TypeScript semantic analysis provider using OXC.
//!
//! This crate provides semantic analysis capabilities for JavaScript and TypeScript
//! using the OXC toolchain (parser, semantic analyzer, and module resolver).
//!
//! # Features
//!
//! - Symbol definition lookup (go-to-definition)
//! - Reference finding (find-all-references)
//! - Type information extraction
//! - Two analysis modes: FileScope (single-file) and WorkspaceScope (workspace-wide)
//!
//! # Example
//!
//! ```no_run
//! use language_javascript::OxcSemanticProvider;
//! use language_core::{SemanticProvider, ByteRange, ProviderMode};
//! use std::path::Path;
//!
//! // Create a file-scope provider for single-file analysis
//! let provider = OxcSemanticProvider::file_scope();
//!
//! // Or create a workspace-scope provider for workspace-wide analysis
//! // let provider = OxcSemanticProvider::workspace_scope(workspace_root);
//!
//! // Notify the provider about processed files to build the symbol cache
//! provider.notify_file_processed(
//!     Path::new("src/utils.ts"),
//!     "export function add(a: number, b: number): number { return a + b; }"
//! ).unwrap();
//!
//! // Query for definitions, references, etc.
//! let definition = provider.get_definition(
//!     Path::new("src/main.ts"),
//!     ByteRange::new(10, 13) // byte range of "add" reference
//! ).unwrap();
//! ```

mod accurate;
mod cache;
mod error;
mod lightweight;
mod oxc_adapter;
mod provider;

pub use error::JsSemanticError;
pub use provider::OxcSemanticProvider;

// Re-export core types for convenience
pub use language_core::{ByteRange, ProviderMode, SemanticProvider, SymbolKind, SymbolLocation};

