//! External agent harnesses: the installed, logged-in `claude` (Claude Code)
//! and `codex` (Codex) CLIs, each run once, non-interactively, in the bridge's
//! working directory with the prompt on stdin.
//!
//! These backends own their agent loop, authenticate with the CLI's own local
//! login (subscription quota), and never see `LLM_API_KEY`. The bridge does
//! not read credential files; it asks the CLI for its login state and keeps
//! only a yes/no.
//!
//! Safety flags, checked against `claude --help` (2.1.x) and `codex exec
//! --help` (0.153.x). The bypass flags (`--dangerously-skip-permissions`,
//! `--dangerously-bypass-approvals-and-sandbox`) are never passed.
//!
//! - Claude Code: `-p --output-format json` (one result object),
//!   `--no-session-persistence`, `--restricted` (ignores user, project, and
//!   local settings files, so their hooks and plugins; confines file tools to
//!   the working directory; refuses bypass), `--strict-mcp-config` with no
//!   `--mcp-config` (no MCP servers), `--disable-slash-commands`,
//!   `--tools` and `--allowedTools` set to exactly the operation's tools,
//!   `--permission-mode dontAsk`, and `--permission-prompts none`: anything
//!   that would ask for permission is denied, so nothing can wait for a person.
//! - Codex: `exec --sandbox <read-only|workspace-write>`,
//!   `-c approval_policy="never"` (no approval prompt can block),
//!   `-c shell_environment_policy.inherit="core"` (commands Codex runs see
//!   only core variables such as `PATH`, `HOME`, and `TMPDIR`, with Codex's
//!   default `*KEY*`/`*SECRET*`/`*TOKEN*` excludes still applied),
//!   `-c sandbox_workspace_write.exclude_slash_tmp=true` (`/tmp` is not a
//!   writable root; the bridge gives Codex a private `TMPDIR` instead),
//!   `--ephemeral`, `--ignore-user-config` (no user MCP servers, profiles, or
//!   trust overrides; auth still comes from `CODEX_HOME`), `--ignore-rules`
//!   (no user or repository execpolicy rules that could loosen the sandbox),
//!   `--color never`, `--json`, `-C <working directory>`, prompt on stdin
//!   (`-`). The final text is the last `agent_message` in the JSON event
//!   stream on stdout: no file path is handed to the sandboxed agent. Codex
//!   refuses to run outside a git repository without `--skip-git-repo-check`,
//!   which is not passed; the bridge checks for a repository first.
//!
//! Every CLI process (login checks and the task) starts with a private,
//! empty temporary `TMPDIR` and without `LLM_API_KEY`, the secrets marker, or
//! any variable whose name looks like a credential ([`is_secret_env_name`]).
//! Login checks run from a separate empty private directory, never the
//! target, so repository settings cannot influence them.
//!
//! Both CLIs may still load repository instructions (`CLAUDE.md`,
//! `AGENTS.md`) from the working directory.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// Claude Code built-in tools an operation may grant (`CLAUDE_CODE_TOOLS` in `protocol.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClaudeCodeTool {
    Read,
    Edit,
    Write,
    Glob,
    Grep,
    Bash,
}

impl ClaudeCodeTool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Edit => "Edit",
            Self::Write => "Write",
            Self::Glob => "Glob",
            Self::Grep => "Grep",
            Self::Bash => "Bash",
        }
    }
}

/// Codex sandbox modes an operation may use (`CODEX_SANDBOXES` in `protocol.ts`).
/// `danger-full-access` has no variant on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexSandbox {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "workspace-write")]
    WorkspaceWrite,
}

impl CodexSandbox {
    pub fn name(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}

/// Flag substrings that must never reach an external CLI.
pub const FORBIDDEN_FLAG_FRAGMENTS: &[&str] = &["dangerously", "bypass", "danger-full-access"];

/// How long a login-state check may take before it counts as a config failure.
pub const AUTH_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Kept from a CLI's stdout: enough for a large final response.
const STDOUT_LIMIT: usize = 16 * 1024 * 1024;
/// Longest JSON event line considered from `codex exec --json`; longer lines are skipped.
const EVENT_LINE_LIMIT: usize = 16 * 1024 * 1024;
/// After a CLI exits, how long its pipes may stay open (a descendant holding
/// them) before the bridge stops reading and uses what it has.
pub const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);
/// Kept from stderr (the tail) and quoted in error messages.
const STDERR_TAIL: usize = 8 * 1024;
const MESSAGE_LIMIT: usize = 2_000;

/// An executable on `path` (the `PATH` value), trying `PATHEXT` suffixes on
/// Windows. Empty, `.`, and other relative entries are ignored: they would
/// resolve against the working directory, which is the target repository.
pub fn find_executable(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let path = path?;
    let suffixes: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| {
            suffixes.iter().find_map(|suffix| {
                let candidate = dir.join(format!("{name}{suffix}"));
                candidate.is_file().then_some(candidate)
            })
        })
}

/// Name tokens that mark a variable as a credential.
const SECRET_TOKENS: &[&str] = &[
    "KEY",
    "KEYS",
    "APIKEY",
    "TOKEN",
    "TOKENS",
    "SECRET",
    "SECRETS",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "CREDENTIALS",
    "AUTH",
    "OAUTH",
    "COOKIE",
    "SESSION",
    "PRIVATE",
];

/// Provider and agent prefixes whose variables route or authenticate model
/// access; external CLIs use their own login instead.
const PROVIDER_PREFIXES: &[&str] = &[
    "ANTHROPIC_",
    "OPENAI_",
    "AZURE_",
    "AWS_",
    "GOOGLE_",
    "GEMINI_",
    "GCP_",
    "VERTEX_",
    "BEDROCK_",
    "CLAUDE_CODE_",
    "CODEX_",
    "LLM_",
];

/// Non-secret locations of a CLI's own login and settings, allowed through.
pub const EXTERNAL_ENV_ALLOWED: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME"];

/// Whether a variable name looks like a credential or provider setting that
/// must not reach an external CLI or the tools it runs. Mirrors
/// `isSecretEnvName` in `agent-env.ts`.
pub fn is_secret_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if EXTERNAL_ENV_ALLOWED.contains(&upper.as_str()) {
        return false;
    }
    PROVIDER_PREFIXES
        .iter()
        .any(|prefix| upper.starts_with(prefix))
        || upper
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| SECRET_TOKENS.contains(&token))
}

/// How one CLI process is started: its working directory and environment
/// changes on top of the bridge's own (already allowlisted) environment.
#[derive(Debug, Clone)]
pub struct Launch {
    pub cwd: PathBuf,
    pub remove: Vec<OsString>,
    pub set: Vec<(OsString, OsString)>,
}

impl Launch {
    /// Remove credential-looking variables present in `vars` and point the
    /// temporary directory variables at `tmp`.
    pub fn external(cwd: &Path, tmp: &Path, vars: impl IntoIterator<Item = OsString>) -> Self {
        let remove = vars
            .into_iter()
            .filter(|name| is_secret_env_name(&name.to_string_lossy()))
            .collect();
        let set = ["TMPDIR", "TMP", "TEMP"]
            .into_iter()
            .map(|name| (OsString::from(name), tmp.as_os_str().to_owned()))
            .collect();
        Self {
            cwd: cwd.to_path_buf(),
            remove,
            set,
        }
    }

    pub fn with_cwd(&self, cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            ..self.clone()
        }
    }
}

pub fn claude_auth_args() -> Vec<OsString> {
    ["auth", "status", "--json"].map(OsString::from).to_vec()
}

pub fn claude_args(tools: &[ClaudeCodeTool]) -> Vec<OsString> {
    let list = tools
        .iter()
        .map(|tool| tool.name())
        .collect::<Vec<_>>()
        .join(",");
    let mut args: Vec<OsString> = [
        "-p",
        "--output-format",
        "json",
        "--no-session-persistence",
        "--restricted",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--permission-mode",
        "dontAsk",
        "--permission-prompts",
        "none",
        "--tools",
    ]
    .map(OsString::from)
    .to_vec();
    // `--tools ""` disables every built-in tool.
    args.push(OsString::from(&list));
    if !tools.is_empty() {
        args.push(OsString::from("--allowedTools"));
        args.push(OsString::from(list));
    }
    args
}

pub fn codex_auth_args() -> Vec<OsString> {
    ["login", "status"].map(OsString::from).to_vec()
}

pub fn codex_args(sandbox: CodexSandbox, working_dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "exec",
        "--sandbox",
        sandbox.name(),
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "shell_environment_policy.inherit=\"core\"",
        "-c",
        "sandbox_workspace_write.exclude_slash_tmp=true",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--color",
        "never",
        "--json",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(OsString::from("-C"));
    args.push(working_dir.as_os_str().to_owned());
    args.push(OsString::from("-"));
    args
}

/// Whether `claude auth status --json` reported a login. Only the boolean is
/// read; account details in the output are never kept.
pub fn claude_logged_in(stdout: &str) -> bool {
    serde_json::from_str::<Value>(stdout.trim())
        .ok()
        .and_then(|value| value.get("loggedIn")?.as_bool())
        .unwrap_or(false)
}

/// The final response from `claude -p --output-format json`: the last
/// `{"type":"result"}` object on stdout. Anything else on stdout is ignored.
pub fn parse_claude_result(stdout: &str) -> Result<String, String> {
    let result = stdout
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find(|value| value.get("type").and_then(Value::as_str) == Some("result"))
        .or_else(|| {
            serde_json::from_str::<Value>(stdout.trim())
                .ok()
                .filter(|value| value.get("type").and_then(Value::as_str) == Some("result"))
        })
        .ok_or_else(|| "claude-code produced no result object".to_string())?;
    let subtype = result
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let is_error = result
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let text = result.get("result").and_then(Value::as_str);
    match (is_error, subtype, text) {
        (false, "success", Some(text)) => Ok(text.to_string()),
        _ => Err(format!(
            "claude-code reported {subtype}: {}",
            limit(text.unwrap_or("no result text"))
        )),
    }
}

/// What the bridge keeps from a `codex exec --json` event stream: the last
/// agent message, the last error message, and counts of lines it could not
/// use. Nothing else from the stream is retained.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodexEvents {
    pub last_message: Option<String>,
    pub last_error: Option<String>,
    /// Lines that were not JSON objects.
    pub malformed: usize,
    /// Lines longer than the event line limit, skipped unread.
    pub oversized: usize,
}

impl CodexEvents {
    pub fn from_jsonl(jsonl: &str) -> Self {
        let mut events = Self::default();
        for line in jsonl.lines() {
            events.scan_line(line);
        }
        events
    }

    pub fn scan_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Some(event) = serde_json::from_str::<Value>(line)
            .ok()
            .filter(Value::is_object)
        else {
            self.malformed += 1;
            return;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("item.completed") => {
                let item = event.get("item");
                let is_message = item
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                    == Some("agent_message");
                if let Some(text) = item
                    .filter(|_| is_message)
                    .and_then(|item| item.get("text"))
                    .and_then(Value::as_str)
                {
                    self.last_message = Some(text.to_string());
                }
            }
            Some("error") => {
                if let Some(message) = event.get("message").and_then(Value::as_str) {
                    self.last_error = Some(message.to_string());
                }
            }
            Some("turn.failed") => {
                if let Some(message) = event
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                {
                    self.last_error = Some(message.to_string());
                }
            }
            _ => {}
        }
    }
}

/// The last error message in a `codex exec --json` event stream, if any.
pub fn codex_error_message(jsonl: &str) -> Option<String> {
    CodexEvents::from_jsonl(jsonl).last_error
}

/// The directory or one of its ancestors holds a `.git` entry.
pub fn inside_git_repository(dir: &Path) -> bool {
    dir.ancestors()
        .any(|ancestor| ancestor.join(".git").exists())
}

pub fn limit(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MESSAGE_LIMIT {
        return text.to_string();
    }
    format!("{}…", text.chars().take(MESSAGE_LIMIT).collect::<String>())
}

/// What to keep from a CLI's stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// The first `STDOUT_LIMIT` bytes, as text.
    Head,
    /// Only [`CodexEvents`], scanned line by line as the stream arrives.
    CodexEvents,
}

/// One finished CLI process.
#[derive(Debug)]
pub struct ProcessOutput {
    pub success: bool,
    pub code: Option<i32>,
    /// Empty for [`Capture::CodexEvents`].
    pub stdout: String,
    pub stderr_tail: String,
    /// Empty unless [`Capture::CodexEvents`].
    pub codex: CodexEvents,
    /// Stdout or stderr was still open [`PIPE_DRAIN_GRACE`] after the process
    /// exited (a descendant kept it); reading stopped there.
    pub pipes_held_open: bool,
}

#[derive(Debug)]
pub enum ProcessError {
    /// The process never started: nothing it could change was touched.
    Spawn(std::io::Error),
    /// The process started, then waiting failed.
    Wait(std::io::Error),
    /// The process did not exit in time and was killed.
    TimedOut,
}

#[derive(Default)]
struct StdoutState {
    head: Vec<u8>,
    events: CodexEvents,
    partial: Vec<u8>,
    skipping: bool,
}

impl StdoutState {
    fn push(&mut self, capture: Capture, chunk: &[u8]) {
        match capture {
            Capture::Head => {
                // Keep draining past the limit so the child never blocks on a full pipe.
                let room = STDOUT_LIMIT.saturating_sub(self.head.len());
                self.head.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Capture::CodexEvents => {
                let mut rest = chunk;
                while let Some(newline) = rest.iter().position(|byte| *byte == b'\n') {
                    self.line_bytes(&rest[..newline]);
                    self.finish_line();
                    rest = &rest[newline + 1..];
                }
                self.line_bytes(rest);
            }
        }
    }

    fn line_bytes(&mut self, bytes: &[u8]) {
        if self.skipping {
            return;
        }
        if self.partial.len() + bytes.len() > EVENT_LINE_LIMIT {
            self.partial.clear();
            self.skipping = true;
            self.events.oversized += 1;
            return;
        }
        self.partial.extend_from_slice(bytes);
    }

    fn finish_line(&mut self) {
        if !self.skipping {
            let line = String::from_utf8_lossy(&self.partial).into_owned();
            self.events.scan_line(&line);
        }
        self.partial.clear();
        self.skipping = false;
    }
}

/// Run `executable` to completion: `stdin` written then closed, stdout kept
/// per `capture`, the stderr tail kept, the environment inherited with
/// `launch`'s changes. Pipes are read by background tasks, so a descendant
/// that keeps them open delays the result by at most [`PIPE_DRAIN_GRACE`]
/// after the process exits. The child is killed on timeout or if this future
/// is dropped.
pub async fn run_process(
    executable: &Path,
    args: &[OsString],
    launch: &Launch,
    stdin: Option<&str>,
    capture: Capture,
    timeout: Option<Duration>,
) -> Result<ProcessOutput, ProcessError> {
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .current_dir(&launch.cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in &launch.remove {
        command.env_remove(name);
    }
    for (name, value) in &launch.set {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(ProcessError::Spawn)?;

    if let (Some(mut pipe), Some(input)) = (child.stdin.take(), stdin.map(str::to_string)) {
        tokio::spawn(async move {
            // A CLI that exits before reading its prompt closes the pipe; its
            // exit status reports that.
            let _ = pipe.write_all(input.as_bytes()).await;
        });
    }
    let stdout_state = Arc::new(Mutex::new(StdoutState::default()));
    let stderr_state = Arc::new(Mutex::new(Vec::new()));
    let mut stdout_task = {
        let state = Arc::clone(&stdout_state);
        let reader = child.stdout.take().expect("piped stdout");
        tokio::spawn(pump(reader, move |chunk| {
            state.lock().expect("stdout state").push(capture, chunk);
        }))
    };
    let mut stderr_task = {
        let state = Arc::clone(&stderr_state);
        let reader = child.stderr.take().expect("piped stderr");
        tokio::spawn(pump(reader, move |chunk| {
            let mut tail = state.lock().expect("stderr state");
            tail.extend_from_slice(chunk);
            if tail.len() > STDERR_TAIL {
                let excess = tail.len() - STDERR_TAIL;
                tail.drain(..excess);
            }
        }))
    };

    let status = match timeout {
        None => child.wait().await,
        Some(duration) => match tokio::time::timeout(duration, child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                let _ = child.start_kill();
                stdout_task.abort();
                stderr_task.abort();
                return Err(ProcessError::TimedOut);
            }
        },
    }
    .map_err(ProcessError::Wait)?;

    let drained = tokio::time::timeout(PIPE_DRAIN_GRACE, async {
        let _ = (&mut stdout_task).await;
        let _ = (&mut stderr_task).await;
    })
    .await
    .is_ok();
    if !drained {
        stdout_task.abort();
        stderr_task.abort();
    }

    let mut stdout = std::mem::take(&mut *stdout_state.lock().expect("stdout state"));
    if capture == Capture::CodexEvents && !stdout.partial.is_empty() {
        stdout.finish_line();
    }
    let stderr = std::mem::take(&mut *stderr_state.lock().expect("stderr state"));
    Ok(ProcessOutput {
        success: status.success(),
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout.head).into_owned(),
        stderr_tail: String::from_utf8_lossy(&stderr).into_owned(),
        codex: stdout.events,
        pipes_held_open: !drained,
    })
}

async fn pump(mut reader: impl AsyncRead + Unpin, mut sink: impl FnMut(&[u8])) {
    let mut buffer = [0u8; 8192];
    while let Ok(read) = reader.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        sink(&buffer[..read]);
    }
}
