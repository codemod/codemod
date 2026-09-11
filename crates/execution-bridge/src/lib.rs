//! Execution bridge for the TypeScript orchestration prototype.
//!
//! One binary, one exchange: an `OperationRequest` is read from a file, an
//! `OperationCompletion` is written to another. `exec` runs through the
//! existing `butterflow_runners::Runner`; `jssg` runs one batch of
//! host-supplied files through the sandbox (see [`jssg`]). The bridge never
//! walks a repository, interprets globs, orders files, applies edits, or
//! decides repository-level failure policy: TypeScript owns all of that
//! (`packages/orchestration/src/jssg.ts`, `RUST_BRIDGE.md`).

use std::collections::HashMap;

use butterflow_models::Error;
use butterflow_runners::Runner;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod jssg;

/// Must match `PROTOCOL_VERSION` in `packages/orchestration/src/protocol.ts`.
pub const PROTOCOL_VERSION: u32 = 4;

/// Every variant rejects fields it does not declare, so a `target` on `exec`
/// or `ai` is a parse error rather than a silently dropped field. `include`,
/// `exclude`, and `target` are decoded for strictness only: TypeScript has
/// already turned them into the file list in `RequestContext::files`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    Exec {
        command: String,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        env: HashMap<String, String>,
    },
    Jssg {
        /// Safe relative path, resolved beneath `RequestContext::script_root`.
        script: String,
        language: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclude: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        semantic_analysis: Option<SemanticAnalysis>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Target>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
    /// Decoded for protocol parity only; no executor adapter exists yet.
    Ai {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
}

/// `"file"`, `"workspace"`, or `{ mode, root? }` where `root` is a safe
/// relative path beneath the target root and requires workspace mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SemanticAnalysis {
    Mode(SemanticMode),
    Detailed(SemanticAnalysisDetails),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticAnalysisDetails {
    pub mode: SemanticMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SemanticMode {
    File,
    Workspace,
}

/// Mirrors `Target` in `protocol.ts`. Interpreted only by TypeScript.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

/// Executor-side context set by the host that spawns the bridge. It is not
/// command identity and never enters history, so it may hold machine-specific
/// paths and file contents.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestContext {
    /// Absolute directory that a relative JSSG `script` resolves against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_root: Option<String>,
    /// Absolute directory every JSSG file path is relative to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_root: Option<String>,
    /// The selected files, in transform order, already read by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<BatchFile>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchFile {
    /// Safe relative path beneath the target root.
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationRequest {
    pub protocol_version: u32,
    pub command_id: String,
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<RequestContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionStatus {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Structured failure detail; produced by TypeScript for JSSG commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationCompletion {
    pub protocol_version: u32,
    pub command_id: String,
    pub status: CompletionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CompletionError>,
}

impl OperationCompletion {
    fn new(command_id: &str, status: CompletionStatus, output: Option<Value>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            command_id: command_id.to_string(),
            status,
            output,
            error: None,
        }
    }

    pub fn not_succeeded(command_id: &str, status: CompletionStatus, message: String) -> Self {
        Self {
            error: Some(CompletionError {
                message,
                exit_code: None,
                output: None,
                details: None,
            }),
            ..Self::new(command_id, status, None)
        }
    }
}

/// Parse a request and reject protocol versions this bridge does not speak.
/// Path rules are enforced where the paths are used (`jssg`).
pub fn parse_request(text: &str) -> Result<OperationRequest, String> {
    let request: OperationRequest =
        serde_json::from_str(text).map_err(|error| format!("invalid request JSON: {error}"))?;
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported protocolVersion {} (expected {PROTOCOL_VERSION})",
            request.protocol_version
        ));
    }
    Ok(request)
}

/// Execute one request. `exec` runs in the process working directory (the
/// runner owns that). `jssg` transforms `context.files` and returns the edits
/// as data in `output.files`; it never writes to the repository.
pub async fn execute(runner: &dyn Runner, request: &OperationRequest) -> OperationCompletion {
    let id = &request.command_id;
    match &request.operation {
        Operation::Exec { command, env } => {
            let mut merged: HashMap<String, String> = std::env::vars().collect();
            merged.extend(env.clone());
            completion_from_result(id, runner.run_command(command, &merged, None).await)
        }
        Operation::Jssg {
            script,
            language,
            semantic_analysis,
            input,
            ..
        } => {
            let context = request.context.clone().unwrap_or_default();
            let batch = jssg::Batch {
                script,
                script_root: context.script_root.as_deref(),
                language,
                target_root: context.target_root.as_deref(),
                semantic_analysis: semantic_analysis.as_ref(),
                input: input.as_ref(),
                files: context.files.as_deref().unwrap_or_default(),
            };
            match jssg::transform_batch(batch).await {
                Ok(files) => OperationCompletion::new(
                    id,
                    CompletionStatus::Succeeded,
                    Some(json!({ "files": files })),
                ),
                Err(message) => {
                    OperationCompletion::not_succeeded(id, CompletionStatus::Failed, message)
                }
            }
        }
        Operation::Ai { .. } => OperationCompletion::not_succeeded(
            id,
            CompletionStatus::Failed,
            "operation kind 'ai' has no executor adapter in the execution bridge".to_string(),
        ),
    }
}

/// Convert the runner's success or failure into a structured completion.
pub fn completion_from_result(
    command_id: &str,
    result: butterflow_models::Result<String>,
) -> OperationCompletion {
    match result {
        Ok(stdout) => OperationCompletion::new(
            command_id,
            CompletionStatus::Succeeded,
            Some(json!({ "stdout": stdout })),
        ),
        Err(Error::ShellCommandFailed { exit_code, output }) => OperationCompletion {
            error: Some(CompletionError {
                message: format!("Command failed with exit code {exit_code}: {output}"),
                exit_code: Some(exit_code),
                output: Some(output),
                details: None,
            }),
            ..OperationCompletion::new(command_id, CompletionStatus::Failed, None)
        },
        // Spawn/wait failures: the bridge cannot tell whether side effects happened.
        Err(error) => OperationCompletion::not_succeeded(
            command_id,
            CompletionStatus::Unknown,
            error.to_string(),
        ),
    }
}
