mod ast_grep;
pub mod capabilities;
#[cfg(feature = "wasm")]
mod plugins;
pub mod sandbox;
pub mod utils;
pub mod workflow_global;

#[cfg(feature = "native")]
pub use ast_grep::{scan_file_with_combined_scan, with_combined_scan};
