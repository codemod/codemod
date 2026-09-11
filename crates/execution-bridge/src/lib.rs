//! Execution bridge for the TypeScript orchestration prototype.
//!
//! Two entry points share one binary:
//!
//! - the one-shot file protocol (`OperationRequest` in, `OperationCompletion`
//!   out) executes `exec` through the existing `butterflow_runners::Runner`;
//! - the JSONL worker (`--jssg-worker`, see [`worker`]) holds one stateful
//!   JSSG [`session::JssgSession`] and answers `open` / `index` / `transform`
//!   / `close` messages with plain JSON.
//!
//! Neither path walks a repository, interprets globs, orders files, applies
//! edits, or decides repository-level failure policy: TypeScript owns all of
//! that (`packages/orchestration/src/jssg.ts`). See
//! `packages/orchestration/RUST_BRIDGE.md`.

use std::{
    collections::HashMap,
    path::{Component, Path},
};

use butterflow_models::Error;
use butterflow_runners::Runner;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub mod paths;
pub mod session;
pub mod worker;

/// Must match `PROTOCOL_VERSION` in `packages/orchestration/src/protocol.ts`.
pub const PROTOCOL_VERSION: u32 = 3;

/// Every variant rejects fields it does not declare, so a `target` on `exec`
/// or `ai` is a parse error rather than a silently dropped field. Only `jssg`
/// carries a target. The `jssg` variant is decoded for wire parity only: JSSG
/// runs through the worker protocol, never through the one-shot bridge.
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
        /// Safe relative path, resolved by the host against its script root.
        /// Never absolute, so command identity is stable across checkouts.
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

impl SemanticAnalysis {
    pub fn mode(&self) -> SemanticMode {
        match self {
            SemanticAnalysis::Mode(mode) => *mode,
            SemanticAnalysis::Detailed(details) => details.mode,
        }
    }

    pub fn root(&self) -> Option<&str> {
        match self {
            SemanticAnalysis::Mode(_) => None,
            SemanticAnalysis::Detailed(details) => details.root.as_deref(),
        }
    }

    /// Shared validation: `root` requires workspace mode and must be a safe
    /// relative path. Used by the one-shot request parser and the worker.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(root) = self.root() {
            if self.mode() == SemanticMode::File {
                return Err("semanticAnalysis.root requires workspace mode".to_string());
            }
            validate_relative_path(root, "semanticAnalysis.root")?;
        }
        Ok(())
    }
}

/// Repository area one JSSG invocation applies to. Mirrors `Target` in
/// `protocol.ts`: `root` is relative to the working directory, `include` and
/// `exclude` are globs relative to `root`. Interpreted only by TypeScript.
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

impl Operation {
    pub fn kind(&self) -> &'static str {
        match self {
            Operation::Exec { .. } => "exec",
            Operation::Jssg { .. } => "jssg",
            Operation::Ai { .. } => "ai",
        }
    }
}

/// Executor-side context that is not part of command identity. It is set by
/// the host that spawns the bridge and is never recorded in history, so it
/// may hold machine-specific paths.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestContext {
    /// Directory that relative JSSG `script` paths are resolved against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_root: Option<String>,
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
    /// Structured failure detail (phase, path, applied/remaining files).
    /// Produced by TypeScript for JSSG commands; the bridge never sets it.
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
    fn succeeded(command_id: &str, output: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            command_id: command_id.to_string(),
            status: CompletionStatus::Succeeded,
            output: Some(output),
            error: None,
        }
    }

    pub fn not_succeeded(command_id: &str, status: CompletionStatus, message: String) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            command_id: command_id.to_string(),
            status,
            output: None,
            error: Some(CompletionError {
                message,
                exit_code: None,
                output: None,
                details: None,
            }),
        }
    }
}

/// Parse a request and reject protocol versions this bridge does not speak.
pub fn parse_request(text: &str) -> Result<OperationRequest, String> {
    let request: OperationRequest =
        serde_json::from_str(text).map_err(|error| format!("invalid request JSON: {error}"))?;
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported protocolVersion {} (expected {PROTOCOL_VERSION})",
            request.protocol_version
        ));
    }
    if let Operation::Jssg {
        script,
        language,
        include,
        exclude,
        semantic_analysis,
        target,
        ..
    } = &request.operation
    {
        validate_relative_path(script, "JSSG script")?;
        if language.trim().is_empty() {
            return Err("JSSG language must not be empty".to_string());
        }
        for (name, patterns) in [("include", include), ("exclude", exclude)] {
            if patterns.as_ref().is_some_and(|values| {
                values.is_empty() || values.iter().any(|value| value.trim().is_empty())
            }) {
                return Err(format!("JSSG {name} must contain non-empty glob patterns"));
            }
        }
        if let Some(root) = target.as_ref().and_then(|target| target.root.as_deref()) {
            validate_relative_path(root, "JSSG target root")?;
        }
        if let Some(semantic) = semantic_analysis {
            semantic.validate()?;
        }
    }
    if let Some(root) = request
        .context
        .as_ref()
        .and_then(|context| context.script_root.as_deref())
    {
        if root.trim().is_empty() {
            return Err("context.scriptRoot must not be empty".to_string());
        }
    }
    Ok(request)
}

/// Same rules as `isSafeRelativePath` in `packages/orchestration/src/paths.ts`:
/// non-empty, not absolute on any platform (`/x`, `\x`, `C:\x`), and no `..`
/// segment. A `..` inside a name such as `foo..bar` is allowed.
pub fn validate_relative_path(value: &str, name: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if is_absolute_path(trimmed) || escapes_root(trimmed) {
        return Err(format!("{name} must be a safe relative path"));
    }
    Ok(())
}

fn is_absolute_path(value: &str) -> bool {
    let path = Path::new(value);
    let bytes = value.as_bytes();
    path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
        || value.starts_with('/')
        || value.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn escapes_root(value: &str) -> bool {
    value.split(['/', '\\']).any(|segment| segment == "..")
}

/// Execute one one-shot request through the given runner. `exec` runs in the
/// process working directory (the runner owns that). `jssg` is refused: it
/// runs through the worker protocol so TypeScript can stage its edits.
pub async fn execute(runner: &dyn Runner, request: &OperationRequest) -> OperationCompletion {
    match &request.operation {
        Operation::Exec { command, env } => {
            let mut merged: HashMap<String, String> = std::env::vars().collect();
            merged.extend(env.clone());
            let result = runner.run_command(command, &merged, None).await;
            completion_from_result(&request.command_id, result)
        }
        Operation::Jssg { .. } => OperationCompletion::not_succeeded(
            &request.command_id,
            CompletionStatus::Failed,
            "jssg operations run through the JSSG worker protocol (--jssg-worker), not the one-shot bridge"
                .to_string(),
        ),
        Operation::Ai { .. } => OperationCompletion::not_succeeded(
            &request.command_id,
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
        Ok(stdout) => {
            let mut output = Map::new();
            output.insert("stdout".to_string(), Value::String(stdout));
            OperationCompletion::succeeded(command_id, Value::Object(output))
        }
        Err(Error::ShellCommandFailed { exit_code, output }) => OperationCompletion {
            protocol_version: PROTOCOL_VERSION,
            command_id: command_id.to_string(),
            status: CompletionStatus::Failed,
            output: None,
            error: Some(CompletionError {
                message: format!("Command failed with exit code {exit_code}: {output}"),
                exit_code: Some(exit_code),
                output: Some(output),
                details: None,
            }),
        },
        // Spawn/wait failures: the bridge cannot tell whether side effects happened.
        Err(error) => OperationCompletion::not_succeeded(
            command_id,
            CompletionStatus::Unknown,
            error.to_string(),
        ),
    }
}
