//! Core traits and types for semantic analysis providers.
//!
//! This crate provides the foundational abstractions for symbol indexing
//! and semantic analysis across different programming languages.

mod dataflow;
mod entry_points;
mod error;
pub mod filesystem;
mod modules;
mod noop;
mod provider;
mod snapshot;
mod types;

pub use dataflow::{
    CallEdgeKind, CallGraphDirection, CallGraphEntry, CallGraphFile, CallGraphOptions,
    CallGraphResult, Crossing, FileFlowSteps, FlowDirection, FlowEdgeKind, FlowNodeKind,
    FlowOptions, FlowResult, FlowStep, FunctionRef, KindSet, Mutation, MutationConfidence,
    MutationKind, MutationOptions, MutationsResult, UnparsedFile,
};
pub use entry_points::{
    declared_entry_points, glob_matches, EntryPoint, EntryPointRule, FunctionEntry,
    FunctionEntryPoints,
};
pub use error::{SemanticError, SemanticResult};
pub use modules::{
    Import, ImportKind, Importer, ImportersFile, ImportersResult, SpecifierOptions, SpecifierStyle,
};
pub use noop::NoopSemanticProvider;
pub use provider::{ProviderMode, SemanticProvider};
pub use snapshot::FileSnapshot;
pub use types::{
    ByteRange, DefinitionKind, DefinitionOptions, DefinitionResult, FileReferences, Position,
    ReferencesResult, SymbolKind, SymbolLocation,
};
