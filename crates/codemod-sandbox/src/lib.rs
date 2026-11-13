mod ast_grep;
pub mod capabilities;
pub mod llm;
pub mod metrics;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod plugins;
pub mod sandbox;
pub mod utils;
pub mod workflow_global;

#[cfg(feature = "native")]
pub use ast_grep::{
    scan_file_with_combined_scan, with_combined_scan, AstGrepModule, DefaultParseLanguage,
};
/// Matchers for hosts that bind the ast-grep API themselves (the Node.js addon).
#[cfg(feature = "native")]
pub mod matchers {
    pub use crate::ast_grep::utils::{json_matcher, JsMatcherRjs as Matcher};
}
pub use metrics::{MetricsContext, MetricsData};
#[cfg(feature = "native")]
pub use sandbox::engine::codemod_lang::CodemodLang;
#[cfg(feature = "jssg-in-memory")]
pub use sandbox::engine::curated_fs::FileFetcher;
#[cfg(feature = "native")]
pub use sandbox::engine::worker::{Worker, WorkerOptions};
#[cfg(feature = "jssg-in-memory")]
pub use sandbox::engine::{
    execute_codemod_sync, CodemodOutput, ExecutionResult, FsSandbox, InMemoryExecutionOptions,
    ProcessSandbox,
};
#[cfg(feature = "jssg-in-memory")]
pub use sandbox::resolvers::{InMemoryLoader, InMemoryResolver};
pub use workflow_global::SharedStateContext;
