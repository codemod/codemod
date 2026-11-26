//! Rust semantic analysis for JSSG, on rust-analyzer's library crates.
//!
//! rust-analyzer resolves names (through `use`, modules, macros by
//! example and trait methods), infers types and knows the Cargo workspace.
//! On top of it, [`facts`] reads each file into [`language_flow`]'s value
//! graph so the shared call-graph, dataflow and mutation queries work for
//! Rust as they do for JavaScript and Python.

mod entry;
mod facts;
mod host;
mod modules;
mod provider;
mod symbols;
mod workspace;

pub use host::RsFlow;
pub use provider::RaSemanticProvider;
