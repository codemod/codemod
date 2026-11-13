//! The native addon behind the `codemod` npm package's Node.js and Bun host.
//!
//! Node runs the workflow body, function steps and `forEachFile` callbacks
//! itself, with the shared orchestrator; this addon gives it what the engine
//! owns ([`run::Run`]: the run store, host tasks, file selection and commits)
//! and the `codemod/ast-grep` API ([`ast`]).
//!
//! There is no capability sandbox here: code in Node has whatever Node gives
//! it. Host tasks (`jssg`, `codemod`, ...) still run in the engine, sandboxed
//! as usual.

pub mod ast;
pub mod run;

/// The orchestrator source the engine runs in QuickJS, so the npm package
/// can check it ships the same one.
#[napi_derive::napi]
pub fn orchestrator_source() -> String {
    codemod_workflow::ORCHESTRATOR_JS.to_string()
}

/// The workflow file a source names: the file itself, or for a package
/// directory the workflow `codemod.yaml` declares (by `name`, or its
/// default), falling back to `workflow.ts`.
#[napi_derive::napi]
pub fn resolve_workflow(source: String, name: Option<String>) -> napi::Result<String> {
    codemod_workflow::locate_workflow(std::path::Path::new(&source), name.as_deref())
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(napi::Error::from_reason)
}
