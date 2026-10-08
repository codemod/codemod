//! `agent`: one task run in the bridge's working directory by the operation's
//! backend. `builtin` is the Butterflow agent (`codemod-ai`, a Rig loop) with
//! exactly the tools the operation names; `claude-code` and `codex` are the
//! installed CLIs (see [`crate::external`]), which never see `LLM_API_KEY`.
//!
//! Builtin configuration is the workflow engine's AI step convention: `LLM_API_KEY`
//! (required), `LLM_PROVIDER` (default `openai`), `LLM_MODEL` (default
//! `gpt-4o`), and `LLM_BASE_URL` (default per provider), from the environment.
//!
//! The API key has a second channel so it never has to be in the bridge's
//! launch environment (which same-user tools can read through `ps eww` or
//! `/proc/<pid>/environ`): with `CODEMOD_BRIDGE_SECRETS=stdin` the binary
//! reads `{"LLM_API_KEY": "..."}` from stdin. Either way the binary calls
//! [`take_process_settings`] before any thread starts, which removes
//! `LLM_API_KEY` and the marker from the process environment, so tool
//! processes do not inherit them. The library [`crate::execute`] does neither:
//! it reads `LLM_API_KEY` from the environment and leaves it there.
//!
//! The completion output is `{ "text": <final response> }`; parsing it is
//! TypeScript's job.
//!
//! Tools are not sandboxed: file tools take any absolute path, and `bash` or
//! `mcp_tool`, when named, run arbitrary commands as the bridge's user.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use codemod_ai::execute::{execute_ai_step, ExecuteAiStepConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::external::{self, ClaudeCodeTool, CodexSandbox, ProcessError};
use crate::{CompletionError, CompletionStatus, Operation, OperationCompletion};

pub const API_KEY_ENV: &str = "LLM_API_KEY";
/// `stdin` when the launcher sends the API key on stdin instead of the environment.
pub const SECRETS_ENV: &str = "CODEMOD_BRIDGE_SECRETS";

/// The `codemod-ai` tool names an operation may list (`AGENT_TOOLS` in `protocol.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentTool {
    #[serde(rename = "bash")]
    Bash,
    #[serde(rename = "str_replace_based_edit_tool")]
    Edit,
    #[serde(rename = "json_edit_tool")]
    JsonEdit,
    #[serde(rename = "glob")]
    Glob,
    #[serde(rename = "sequentialthinking")]
    SequentialThinking,
    #[serde(rename = "task_done")]
    TaskDone,
    #[serde(rename = "ckg_tool")]
    Ckg,
    #[serde(rename = "mcp_tool")]
    Mcp,
}

impl AgentTool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Edit => "str_replace_based_edit_tool",
            Self::JsonEdit => "json_edit_tool",
            Self::Glob => "glob",
            Self::SequentialThinking => "sequentialthinking",
            Self::TaskDone => "task_done",
            Self::Ckg => "ckg_tool",
            Self::Mcp => "mcp_tool",
        }
    }
}

/// `json` appends [`JSON_RESPONSE_INSTRUCTION`] to the task. The bridge does not
/// check the reply; TypeScript decoding enforces the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseFormat {
    #[serde(rename = "json")]
    Json,
}

/// Which agent loop runs the task (`AgentBackend` in `protocol.ts`). Each
/// variant carries only settings that backend enforces; a setting from another
/// backend is a parse error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentBackend {
    Builtin {
        tools: Vec<AgentTool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_steps: Option<usize>,
    },
    ClaudeCode {
        tools: Vec<ClaudeCodeTool>,
    },
    Codex {
        sandbox: CodexSandbox,
    },
}

impl AgentBackend {
    pub fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin { .. })
    }
}

/// The part of an `agent` operation that drives one run.
#[derive(Debug, Clone, Copy)]
pub struct Task<'a> {
    pub prompt: &'a str,
    pub input: Option<&'a Value>,
    pub backend: &'a AgentBackend,
    pub response_format: Option<ResponseFormat>,
}

impl<'a> Task<'a> {
    pub fn from_operation(operation: &'a Operation) -> Option<Self> {
        match operation {
            Operation::Agent {
                prompt,
                input,
                backend,
                response_format,
            } => Some(Self {
                prompt,
                input: input.as_ref(),
                backend,
                response_format: *response_format,
            }),
            _ => None,
        }
    }
}

/// Remove the builtin agent's key and the secrets marker from the process
/// environment without reading them: what an external backend's bridge does
/// before any thread starts. Must be called while single-threaded.
pub fn scrub_process_env() {
    std::env::remove_var(API_KEY_ENV);
    std::env::remove_var(SECRETS_ENV);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSettings {
    pub api_key: String,
    pub provider: String,
    pub model: String,
    pub endpoint: String,
}

/// Resolve settings the way the engine's AI step does
/// (`crates/core/src/engine.rs`, `execute_ai_step`). Blank values count as
/// unset. A missing API key is an error: unlike the engine, the bridge has no
/// instructions channel to fall back to.
pub fn settings_from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<AgentSettings, String> {
    let get = |name: &str| lookup(name).filter(|value| !value.trim().is_empty());
    let api_key = get(API_KEY_ENV).ok_or_else(|| format!("agent requires {API_KEY_ENV}"))?;
    let provider = get("LLM_PROVIDER").unwrap_or_else(|| "openai".to_string());
    let model = get("LLM_MODEL").unwrap_or_else(|| "gpt-4o".to_string());
    let endpoint = get("LLM_BASE_URL").unwrap_or_else(|| {
        match provider.as_str() {
            "anthropic" => "https://api.anthropic.com",
            "google_ai" => "https://generativelanguage.googleapis.com/v1beta",
            "azure_openai" => "https://api.openai.com",
            _ => "https://api.openai.com/v1",
        }
        .to_string()
    });
    Ok(AgentSettings {
        api_key,
        provider,
        model,
        endpoint,
    })
}

/// Settings for this bridge process, read once before any thread exists: the
/// API key from stdin when `CODEMOD_BRIDGE_SECRETS=stdin`, else from the
/// environment. Always removes `LLM_API_KEY` and `CODEMOD_BRIDGE_SECRETS`
/// from the process environment, also on error, so processes the agent
/// starts do not inherit them.
///
/// Must be called while the process is single-threaded: it mutates the
/// process environment.
pub fn take_process_settings(stdin: impl Read) -> Result<AgentSettings, String> {
    let secrets = match std::env::var(SECRETS_ENV).ok().as_deref() {
        None => Ok(BTreeMap::new()),
        Some("stdin") => read_secrets(stdin),
        Some(other) => Err(format!("unsupported {SECRETS_ENV} value '{other}'")),
    };
    let settings = secrets.and_then(|secrets| {
        settings_from_env(|name| {
            secrets
                .get(name)
                .cloned()
                .or_else(|| std::env::var(name).ok())
        })
    });
    std::env::remove_var(API_KEY_ENV);
    std::env::remove_var(SECRETS_ENV);
    settings
}

fn read_secrets(stdin: impl Read) -> Result<BTreeMap<String, String>, String> {
    let mut text = String::new();
    stdin
        .take(64 * 1024)
        .read_to_string(&mut text)
        .map_err(|error| format!("failed to read agent secrets from stdin: {error}"))?;
    let secrets: BTreeMap<String, String> = serde_json::from_str(&text)
        .map_err(|_| "agent secrets on stdin must be a JSON object of strings".to_string())?;
    if let Some(unknown) = secrets.keys().find(|name| name.as_str() != API_KEY_ENV) {
        return Err(format!("unsupported agent secret '{unknown}'"));
    }
    Ok(secrets)
}

pub const JSON_RESPONSE_INSTRUCTION: &str = "Final response format: when the task is done, reply with a single JSON value and nothing else. Do not add prose. If you use a code fence, use exactly one ```json block.";

/// The task message: the prompt, the validated input as JSON when there is
/// one, and the JSON response requirement when the step declares an output.
pub fn task_prompt(prompt: &str, input: Option<&Value>, format: Option<ResponseFormat>) -> String {
    let mut message = prompt.to_string();
    if let Some(input) = input {
        message.push_str(&format!(
            "\n\nInput (JSON):\n```json\n{}\n```",
            serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string())
        ));
    }
    if format == Some(ResponseFormat::Json) {
        message.push_str("\n\n");
        message.push_str(JSON_RESPONSE_INSTRUCTION);
    }
    message
}

fn failure(
    command_id: &str,
    message: String,
    phase: &str,
    repository_may_be_modified: bool,
) -> OperationCompletion {
    OperationCompletion {
        error: Some(CompletionError {
            message,
            exit_code: None,
            output: None,
            details: Some(json!({
                "phase": phase,
                "repositoryMayBeModified": repository_may_be_modified,
            })),
        }),
        ..OperationCompletion::new(command_id, CompletionStatus::Failed, None)
    }
}

/// Run one task. Failures before the agent starts (`phase: "config"`) left the
/// repository untouched; a failure while it ran (`phase: "execute"`) may not have.
/// `settings` is only consulted by the builtin backend.
pub async fn run(
    command_id: &str,
    task: Task<'_>,
    settings: impl FnOnce() -> Result<AgentSettings, String>,
    working_dir: std::io::Result<PathBuf>,
) -> OperationCompletion {
    let config_failure = |message: String| failure(command_id, message, "config", false);
    let working_dir = match working_dir {
        Ok(dir) => dir,
        Err(error) => return config_failure(format!("agent has no working directory: {error}")),
    };
    let prompt = task_prompt(task.prompt, task.input, task.response_format);
    let (tools, max_steps) = match task.backend {
        AgentBackend::Builtin { tools, max_steps } => (tools, *max_steps),
        AgentBackend::ClaudeCode { tools } => {
            if tools.iter().collect::<HashSet<_>>().len() != tools.len() {
                return config_failure("claude-code tools must not repeat".to_string());
            }
            return run_claude_code(command_id, tools, &prompt, &working_dir, path_var()).await;
        }
        AgentBackend::Codex { sandbox } => {
            return run_codex(command_id, *sandbox, &prompt, &working_dir, path_var()).await;
        }
    };
    let settings = match settings() {
        Ok(settings) => settings,
        Err(message) => return config_failure(message),
    };
    if tools.iter().collect::<HashSet<_>>().len() != tools.len() {
        return config_failure("agent tools must not repeat".to_string());
    }
    if max_steps == Some(0) {
        return config_failure("agent maxSteps must be at least 1".to_string());
    }
    let config = ExecuteAiStepConfig {
        endpoint: settings.endpoint,
        api_key: settings.api_key,
        model: settings.model,
        system_prompt: None,
        max_steps,
        // Always explicit: `None` would mean codemod-ai's defaults, which include `bash`.
        tools: Some(tools.iter().map(|tool| tool.name().to_string()).collect()),
        prompt,
        working_dir,
        llm_protocol: settings.provider,
    };
    match execute_ai_step(config).await {
        Ok(result) => {
            let text = match result.data {
                None => String::new(),
                Some(Value::String(text)) => text,
                Some(other) => other.to_string(),
            };
            OperationCompletion::new(
                command_id,
                CompletionStatus::Succeeded,
                Some(json!({ "text": text })),
            )
        }
        Err(error) => failure(command_id, error.to_string(), "execute", true),
    }
}

fn path_var() -> Option<OsString> {
    std::env::var_os("PATH")
}

fn succeeded(command_id: &str, text: String) -> OperationCompletion {
    OperationCompletion::new(
        command_id,
        CompletionStatus::Succeeded,
        Some(json!({ "text": text })),
    )
}

struct Cli {
    label: &'static str,
    executable: &'static str,
    auth_args: Vec<OsString>,
    /// Reads only the login yes/no from the status command's output.
    logged_in: fn(&external::ProcessOutput) -> bool,
    login_hint: &'static str,
}

/// A located, logged-in CLI and how to start it for the task.
struct Prepared {
    bin: PathBuf,
    launch: external::Launch,
    /// Holds the private temporary directory until the task ends.
    _private: tempfile::TempDir,
}

/// Find the CLI, create its private directories, and confirm it is logged in.
/// Every failure here is `config`: the task has not started.
///
/// The login check runs from an empty private directory (never the target,
/// so repository settings, instructions, and hooks cannot affect it) with
/// exactly the environment the task will get.
async fn prepare_external(
    command_id: &str,
    cli: Cli,
    working_dir: &Path,
    path: Option<OsString>,
) -> Result<Prepared, OperationCompletion> {
    let Cli {
        label,
        executable,
        auth_args,
        logged_in,
        login_hint,
    } = cli;
    let config = |message: String| failure(command_id, message, "config", false);
    let Some(bin) = external::find_executable(executable, path.as_deref()) else {
        return Err(config(format!(
            "{label} backend requires the `{executable}` executable on an absolute PATH entry"
        )));
    };
    let private = tempfile::Builder::new()
        .prefix("codemod-agent-")
        .tempdir()
        .map_err(|error| config(format!("{label} private directory: {error}")))?;
    let check_dir = private.path().join("login-check");
    let tmp_dir = private.path().join("tmp");
    for dir in [&check_dir, &tmp_dir] {
        std::fs::create_dir(dir)
            .map_err(|error| config(format!("{label} private directory: {error}")))?;
    }
    let launch = external::Launch::external(
        working_dir,
        &tmp_dir,
        std::env::vars_os().map(|(name, _)| name),
    );
    match external::run_process(
        &bin,
        &auth_args,
        &launch.with_cwd(&check_dir),
        None,
        external::Capture::Head,
        Some(external::AUTH_CHECK_TIMEOUT),
    )
    .await
    {
        Ok(output) if logged_in(&output) => Ok(Prepared {
            bin,
            launch,
            _private: private,
        }),
        Ok(_) => Err(config(format!(
            "{label} is not logged in; run `{login_hint}`"
        ))),
        Err(ProcessError::TimedOut) => Err(config(format!("{label} login status check timed out"))),
        Err(ProcessError::Spawn(error) | ProcessError::Wait(error)) => Err(config(format!(
            "{label} login status check failed: {error}"
        ))),
    }
}

fn started_failure(command_id: &str, label: &str, error: ProcessError) -> OperationCompletion {
    match error {
        // Never ran: nothing changed.
        ProcessError::Spawn(error) => failure(
            command_id,
            format!("{label} could not start: {error}"),
            "config",
            false,
        ),
        ProcessError::Wait(error) => failure(
            command_id,
            format!("{label} failed while running: {error}"),
            "execute",
            true,
        ),
        ProcessError::TimedOut => {
            failure(command_id, format!("{label} timed out"), "execute", true)
        }
    }
}

pub async fn run_claude_code(
    command_id: &str,
    tools: &[ClaudeCodeTool],
    prompt: &str,
    working_dir: &Path,
    path: Option<OsString>,
) -> OperationCompletion {
    let label = "claude-code";
    let cli = Cli {
        label,
        executable: "claude",
        auth_args: external::claude_auth_args(),
        logged_in: |output| output.success && external::claude_logged_in(&output.stdout),
        login_hint: "claude auth login",
    };
    let prepared = match prepare_external(command_id, cli, working_dir, path).await {
        Ok(prepared) => prepared,
        Err(completion) => return completion,
    };
    let output = match external::run_process(
        &prepared.bin,
        &external::claude_args(tools),
        &prepared.launch,
        Some(prompt),
        external::Capture::Head,
        None,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return started_failure(command_id, label, error),
    };
    match external::parse_claude_result(&output.stdout) {
        Ok(text) if output.success => succeeded(command_id, text),
        Ok(_) => failure(
            command_id,
            format!(
                "{label} exited with {}: {}",
                exit_text(output.code),
                external::limit(&output.stderr_tail)
            ),
            "execute",
            true,
        ),
        Err(message) => failure(command_id, with_stderr(message, &output), "execute", true),
    }
}

pub async fn run_codex(
    command_id: &str,
    sandbox: CodexSandbox,
    prompt: &str,
    working_dir: &Path,
    path: Option<OsString>,
) -> OperationCompletion {
    let label = "codex";
    if !external::inside_git_repository(working_dir) {
        return failure(
            command_id,
            format!(
                "{label} backend requires the target to be inside a git repository; `codex exec` refuses other directories and the bridge does not skip that check"
            ),
            "config",
            false,
        );
    }
    let cli = Cli {
        label,
        executable: "codex",
        auth_args: external::codex_auth_args(),
        logged_in: |output| output.success,
        login_hint: "codex login",
    };
    let prepared = match prepare_external(command_id, cli, working_dir, path).await {
        Ok(prepared) => prepared,
        Err(completion) => return completion,
    };
    let output = match external::run_process(
        &prepared.bin,
        &external::codex_args(sandbox, working_dir),
        &prepared.launch,
        Some(prompt),
        external::Capture::CodexEvents,
        None,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return started_failure(command_id, label, error),
    };
    match (output.success, &output.codex.last_message) {
        (true, Some(text)) => succeeded(command_id, text.trim_end().to_string()),
        (true, None) => failure(
            command_id,
            format!(
                "{label} finished without a final agent message ({} malformed and {} oversized event lines ignored)",
                output.codex.malformed, output.codex.oversized
            ),
            "execute",
            true,
        ),
        (false, _) => {
            let reason = output
                .codex
                .last_error
                .as_deref()
                .map(external::limit)
                .unwrap_or_else(|| external::limit(&output.stderr_tail));
            failure(
                command_id,
                format!("{label} exited with {}: {reason}", exit_text(output.code)),
                "execute",
                true,
            )
        }
    }
}

fn exit_text(code: Option<i32>) -> String {
    code.map_or_else(|| "a signal".to_string(), |code| format!("code {code}"))
}

fn with_stderr(message: String, output: &external::ProcessOutput) -> String {
    let tail = external::limit(&output.stderr_tail);
    if tail.is_empty() {
        message
    } else {
        format!("{message} ({}; stderr: {tail})", exit_text(output.code))
    }
}
