//! Core traits and types for semantic analysis providers.
//!
//! This crate provides the foundational abstractions for symbol indexing
//! and semantic analysis across different programming languages.

mod dataflow;
mod error;
pub mod filesystem;
mod model_graph;
mod models;
mod modules;
mod noop;
mod provider;
mod types;

pub use dataflow::{
    CallEdgeKind, CallGraphDirection, CallGraphEntry, CallGraphFile, CallGraphOptions,
    CallGraphResult, FileFlowSteps, FlowDirection, FlowEdgeKind, FlowNodeKind, FlowOptions,
    FlowResult, FlowStep, FunctionRef, KindSet, Mutation, MutationConfidence, MutationKind,
    MutationOptions, MutationsResult, UnparsedFile,
};
pub use error::{SemanticError, SemanticResult};
pub use model_graph::{
    model_call_graph, model_data_flow, FactResolver, ModelCall, ModelEntryPoint, ModelFlow,
    ModelFunction, ModelGraph, ModelGraphCache, ModelResolver, ModelStore, ModelValue,
};
pub use models::{
    match_keys, ChannelFact, ChannelKey, ChannelReport, ChannelSide, KeyMatch, KeyPart, ModelFact,
    ModelFileFacts, ModelNode, ModelPort, ModelSpan,
};
pub use modules::{
    ImportKind, Importer, ImportersFile, ImportersResult, SpecifierOptions, SpecifierStyle,
};
pub use noop::NoopSemanticProvider;
pub use provider::{ProviderMode, SemanticProvider};
pub use types::{
    ByteRange, DefinitionKind, DefinitionOptions, DefinitionResult, FileReferences, Position,
    ReferencesResult, SymbolKind, SymbolLocation,
};
