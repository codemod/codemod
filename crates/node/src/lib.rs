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

/// Identifies the orchestrator source this addon's engine was built from,
/// so the npm package can check it ships the same one.
#[napi_derive::napi]
pub fn orchestrator_version() -> String {
    codemod_workflow::orchestrator_version()
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

/// The workflow source as Node.js should load it: transforms imported for
/// `jssg` and `shard` replaced by references to their files, and their
/// imports dropped, as the codemod CLI's build does. `None` when the
/// workflow imports no transforms. Throws the build's error for a
/// transform the build cannot read.
#[napi_derive::napi]
pub fn rewrite_workflow(file: String, source: String) -> napi::Result<Option<String>> {
    codemod_workflow::rewrite_transforms(&file, &source)
        .map_err(|error| napi::Error::from_reason(error.to_string()))
}
