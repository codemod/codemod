#[cfg(feature = "native")]
pub mod curated_fs;
#[cfg(feature = "native")]
pub mod embedded;
pub mod execution_engine;
#[cfg(feature = "native")]
pub mod fetching_vfs;
pub mod in_memory_engine;
pub mod quickjs_adapters;
pub mod selector_engine;
pub(crate) mod transform_helpers;

#[cfg(feature = "native")]
pub mod codemod_lang;

pub use execution_engine::*;
pub use in_memory_engine::*;
pub use selector_engine::*;
/// The binding-agnostic result rules, shared with embedders that bind
/// transforms themselves (`crates/orchestration/src/jssg_batch.rs`).
pub use transform_helpers::{
    classify_transform_result, missing_output_error, output_json_error, ModificationCheck,
    ReturnedContent, MISSING_OUTPUT_MESSAGE, SECONDARY_STRUCTURED_RESULT,
};
pub mod language_data;
