mod ast_grep;
pub mod capabilities;
#[cfg(feature = "wasm")]
mod plugins;
pub mod sandbox;
pub mod utils;

#[cfg(feature = "native")]
pub use ast_grep::{scan_file_with_combined_scan, with_combined_scan};

#[cfg(feature = "jssg-in-memory")]
pub use sandbox::engine::{execute_codemod_sync, ExecutionResult, InMemoryExecutionOptions};
#[cfg(feature = "jssg-in-memory")]
pub use sandbox::resolvers::{InMemoryLoader, InMemoryResolver};
