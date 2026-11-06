//! Native file selection (`crates/orchestration-walker`) as the host exposes
//! it: the `selectFiles` / `readSources` primitives (`native.rs`) and the
//! `orchestration-quickjs-host select` command, which runs the same calls on
//! JSON from stdin so `packages/orchestration/tests/files-native.test.ts` can
//! compare them with the TypeScript walker.

use serde::Deserialize;
use serde_json::{json, Value};

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

/// One `select` command job.
#[derive(Deserialize)]
#[serde(untagged)]
enum Job {
    /// `selectFiles(request)`.
    Select { request: SelectRequest },
    /// `readSources(root, paths)`.
    Read { root: String, paths: Vec<String> },
}

/// `orchestration-quickjs-host select`: a JSON array of jobs in
/// (`{ request }` or `{ root, paths }`), a JSON array out, one per job:
/// `{ files }`, `{ sources }` (`null` for a skipped file), or
/// `{ error: { code, message } }`.
pub fn run_jobs(input: &str) -> Result<String, String> {
    let jobs: Vec<Job> =
        serde_json::from_str(input).map_err(|error| format!("invalid select input: {error}"))?;
    let failure = |error: &orchestration_walker::Error| {
        let (code, message) = describe(error);
        json!({ "error": { "code": code, "message": message } })
    };
    let results: Vec<Value> = jobs
        .into_iter()
        .map(|job| match job {
            Job::Select { request } => match orchestration_walker::select(&request.into()) {
                Ok(files) => json!({ "files": files }),
                Err(error) => failure(&error),
            },
            Job::Read { root, paths } => match orchestration_walker::read_sources(&root, &paths) {
                Ok(sources) => json!({ "sources": sources }),
                Err(error) => failure(&error),
            },
        })
        .collect();
    Ok(Value::Array(results).to_string())
}
