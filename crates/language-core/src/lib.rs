//! Core traits and types for semantic analysis providers.
//!
//! This crate provides the foundational abstractions for symbol indexing
//! and semantic analysis across different programming languages.

mod dataflow;
mod error;
pub mod filesystem;
mod noop;
mod provider;
mod types;

pub use dataflow::{
    CallEdgeKind, CallGraphDirection, CallGraphEntry, CallGraphFile, CallGraphOptions,
    CallGraphResult, FileFlowSteps, FlowDirection, FlowEdgeKind, FlowNodeKind, FlowOptions,
    FlowResult, FlowStep, Mutation, MutationConfidence, MutationKind, MutationOptions,
    MutationsResult,
};
pub use error::{SemanticError, SemanticResult};
pub use noop::NoopSemanticProvider;
pub use provider::{ProviderMode, SemanticProvider};
pub use types::{
    ByteRange, DefinitionKind, DefinitionOptions, DefinitionResult, FileReferences, Position,
    ReferencesResult, SymbolKind, SymbolLocation,
};
