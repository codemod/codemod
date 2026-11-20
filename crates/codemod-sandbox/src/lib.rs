mod ast_grep;
pub mod capabilities;
/// The codemod runtime modules and how code names them.
pub use codemod_llrt_capabilities::codemod_modules;
pub mod llm;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod plugins;
pub mod sandbox;
pub mod utils;

#[cfg(feature = "native")]
pub use ast_grep::{
    scan_file_with_combined_scan, with_combined_scan, with_combined_scan_of, AstGrepModule,
    DefaultParseLanguage,
};
/// What hosts that bind the ast-grep API themselves (the Node.js addon)
/// share with this one.
#[cfg(feature = "native")]
pub mod matchers {
    pub use crate::ast_grep::sg_node::splice_edits;
    pub use crate::ast_grep::utils::{json_matcher, JsMatcherRjs as Matcher};
}
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
