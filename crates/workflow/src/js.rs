//! The engine's own JavaScript.
//!
//! It is written in TypeScript (`js/*.ts`) that only needs its types
//! removed, which happens here the first time each file is used, so building
//! the engine needs nothing but cargo. The types are checked by
//! `pnpm --filter codemod typecheck`; the npm package builds its copy of the
//! orchestrator from the same source with `tsc`.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

const ORCHESTRATOR_TS: &str = include_str!("js/orchestrator.ts");

/// Appended by the build to every extracted callback, which then strips the
/// types of the whole module.
pub(crate) const CALLBACK_PRELUDE_TS: &str = include_str!("js/callback_prelude.ts");

/// These files are part of the engine: one that cannot be stripped is a bug
/// in the engine, which every test that runs a workflow would show.
fn stripped(file: &str, source: &str) -> String {
    crate::build::strip_types(file, source, true)
        .unwrap_or_else(|error| panic!("the engine's {file} is not valid: {error}"))
}

macro_rules! javascript {
    ($(#[$doc:meta])* $visibility:vis fn $name:ident = $source:expr, $file:literal) => {
        $(#[$doc])*
        $visibility fn $name() -> &'static str {
            static JS: OnceLock<String> = OnceLock::new();
            JS.get_or_init(|| stripped($file, $source))
        }
    };
}

javascript! {
    /// The orchestrator shared with the Node.js host: `step()`, replay and
    /// the task builders.
    pub fn orchestrator = ORCHESTRATOR_TS, "orchestrator.ts"
}
javascript! {
    /// `codemod/workflow` in the workflow body: the orchestrator bound to
    /// the host the engine installs.
    pub(crate) fn workflow_module = include_str!("js/workflow.ts"), "workflow.ts"
}
javascript! {
    /// `codemod/deps` in the workflow body: `dependencyGraph`, and `registry`
    /// and `semver` outside steps.
    pub(crate) fn deps_module = include_str!("js/deps.ts"), "deps.ts"
}
javascript! {
    /// `codemod-internal:semver`: `semver` for the body and isolates.
    pub(crate) fn semver_module = include_str!("js/semver.ts"), "semver.ts"
}
javascript! {
    /// `codemod/deps` in isolates, also as `codemod-internal:dependencies`:
    /// `registry`, `semver` and the graph.
    pub(crate) fn dependencies_module = include_str!("js/dependencies.ts"), "dependencies.ts"
}
javascript! {
    /// `codemod/workflow` in isolates, for files steps and callbacks import:
    /// the body-only functions as errors.
    pub(crate) fn isolate_workflow_module = include_str!("js/isolate_workflow.ts"), "isolate_workflow.ts"
}
javascript! {
    /// `codemod-internal:jssg-compat` in isolates: a jssg transform module
    /// run from a `forEachFile` callback, for the YAML translation.
    pub(crate) fn jssg_compat_module = include_str!("js/jssg_compat.ts"), "jssg_compat.ts"
}
javascript! {
    /// Replay-safe globals of the workflow body; a script, not a module.
    pub(crate) fn body_globals = include_str!("js/body_globals.ts"), "body_globals.ts"
}

/// Identifies the orchestrator's source. The npm package records the same
/// value when it is built, so it can tell an addon built from another
/// version of the orchestrator. Line endings do not count: the two may be
/// built from checkouts that differ in them.
pub fn orchestrator_version() -> String {
    let digest = Sha256::digest(ORCHESTRATOR_TS.replace('\r', "").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_file_strips_to_javascript_without_types() {
        for js in [
            super::orchestrator(),
            super::workflow_module(),
            super::deps_module(),
            super::body_globals(),
            super::semver_module(),
            super::dependencies_module(),
            super::isolate_workflow_module(),
            super::jssg_compat_module(),
        ] {
            assert!(!js.is_empty());
            assert!(!js.contains("interface "), "types are left in:\n{js}");
            assert!(!js.contains(": string"), "types are left in:\n{js}");
        }
        assert_eq!(super::orchestrator_version().len(), 64);
    }
}
