//! The agent runner of the TypeScript orchestration runtime (`quickjs`):
//! `codemod-agent` reads one `agent` request on stdin and writes
//! its completion on stdout (`src/main.rs`). Every other operation kind
//! runs in the runtime itself; an agent runs here, in its own process,
//! because its tools (`bash`, file tools, MCP servers, the `claude` and
//! `codex` CLIs) run in the process working directory and inherit its
//! environment: the host starts the runner in the target directory with an
//! allowlisted environment and the API key on stdin
//! (`packages/orchestration/src/execution/agent.ts`, `agent-env.ts`).
//!
//! - [`agent`]: the task, the backends, settings and the key channel;
//! - [`external`]: the `claude` and `codex` CLIs, hardened.
//!
//! The wire shapes are the orchestration protocol's (`OperationRequest` /
//! `OperationCompletion` in `packages/orchestration/src/core/protocol.ts`),
//! restricted to `kind: "agent"`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod agent;
pub mod external;

/// Must match `PROTOCOL_VERSION` in `packages/orchestration/src/core/protocol.ts`.
pub const PROTOCOL_VERSION: u32 = 8;

/// The one operation the runner executes. Another `kind` is a parse error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    /// A task for an agent backend, run in the working directory.
    Agent {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
        backend: agent::AgentBackend,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_format: Option<agent::ResponseFormat>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
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

/// Parse a request and reject protocol versions this runner does not speak.
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
