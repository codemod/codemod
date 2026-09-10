//! Execution bridge for the TypeScript orchestration prototype.
//!
//! One job: turn a JSON `OperationRequest` into a JSON `OperationCompletion`
//! by running an `exec` operation through the existing
//! `butterflow_runners::Runner`. No plans, replay, history, or scheduling live
//! here. See `packages/orchestration/RUST_BRIDGE.md` for the TypeScript
//! equivalent of every item in this file.

use std::collections::HashMap;

use butterflow_models::Error;
use butterflow_runners::Runner;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Must match `PROTOCOL_VERSION` in `packages/orchestration/src/protocol.ts`.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Operation {
    Exec {
        command: String,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        env: HashMap<String, String>,
    },
    /// Decoded for protocol parity only; no executor adapter exists yet.
    Jssg {
        package: String,
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

impl Operation {
    pub fn kind(&self) -> &'static str {
        match self {
            Operation::Exec { .. } => "exec",
            Operation::Jssg { .. } => "jssg",
            Operation::Ai { .. } => "ai",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRequest {
    pub protocol_version: u32,
    pub command_id: String,
    pub operation: Operation,
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
#[serde(rename_all = "camelCase")]
pub struct CompletionError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

    fn not_succeeded(command_id: &str, status: CompletionStatus, error: CompletionError) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            command_id: command_id.to_string(),
            status,
            output: None,
            error: Some(error),
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
    Ok(request)
}

/// Execute one request through the given runner and describe the outcome.
pub async fn execute(runner: &dyn Runner, request: &OperationRequest) -> OperationCompletion {
    match &request.operation {
        Operation::Exec { command, env } => {
            let mut merged: HashMap<String, String> = std::env::vars().collect();
            merged.extend(env.clone());
            let result = runner.run_command(command, &merged, None).await;
            completion_from_result(&request.command_id, result)
        }
        other => OperationCompletion::not_succeeded(
            &request.command_id,
            CompletionStatus::Failed,
            CompletionError {
                message: format!(
                    "operation kind '{}' has no executor adapter in the execution bridge",
                    other.kind()
                ),
                exit_code: None,
                output: None,
            },
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
        Err(Error::ShellCommandFailed { exit_code, output }) => OperationCompletion::not_succeeded(
            command_id,
            CompletionStatus::Failed,
            CompletionError {
                message: format!("Command failed with exit code {exit_code}: {output}"),
                exit_code: Some(exit_code),
                output: Some(output),
            },
        ),
        // Spawn/wait failures: the bridge cannot tell whether side effects happened.
        Err(error) => OperationCompletion::not_succeeded(
            command_id,
            CompletionStatus::Unknown,
            CompletionError {
                message: error.to_string(),
                exit_code: None,
                output: None,
            },
        ),
    }
}
