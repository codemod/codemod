//! Native file selection (`crates/orchestration-walker`) as the host exposes
//! it: the request shape and error text the `selectFiles` / `readSources`
//! primitives (`native.rs`) use.

use serde::Deserialize;

use crate::native::io_code;

/// `HostSelectRequest` (`packages/orchestration/src/core/host.ts`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectRequest {
    root: String,
    overrides: Vec<SelectOverride>,
    global_excludes: Option<String>,
}

#[derive(Deserialize)]
struct SelectOverride {
    root: String,
    patterns: Vec<String>,
    whitelists: bool,
}

impl From<SelectRequest> for orchestration_walker::Request {
    fn from(request: SelectRequest) -> Self {
        Self {
            root: request.root,
            overrides: request
                .overrides
                .into_iter()
                .map(|set| orchestration_walker::OverrideSet {
                    root: set.root,
                    patterns: set.patterns,
                    whitelists: set.whitelists,
                })
                .collect(),
            global_excludes: request.global_excludes,
        }
    }
}

/// The error `code` and message the primitives throw, the same text the
/// host's `fs` functions use (`EACCES: ..., scandir '<path>'`).
pub fn describe(error: &orchestration_walker::Error) -> (Option<&'static str>, String) {
    match error {
        orchestration_walker::Error::Io { op, path, error } => {
            let code = io_code(error);
            (Some(code), format!("{code}: {error}, {op} '{path}'"))
        }
        other => (None, other.to_string()),
    }
}
