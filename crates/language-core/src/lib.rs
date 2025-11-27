//! Core traits and types for semantic analysis providers.
//!
//! This crate provides the foundational abstractions for symbol indexing
//! and semantic analysis across different programming languages.

mod error;
mod noop;
mod provider;
mod types;

pub use error::{SemanticError, SemanticResult};
pub use noop::NoopSemanticProvider;
pub use provider::{ProviderMode, SemanticProvider};
pub use types::{
    ByteRange, DefinitionResult, FileReferences, Position, ReferencesResult, SymbolKind,
    SymbolLocation,
};
