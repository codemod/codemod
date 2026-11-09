//! `codemod ai doctor`: checks that a Codemod setup is present, current, and
//! reachable by the agent, and names the command that fixes each problem.

use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// Harnesses in the order setups are reported.
const SETUP_HARNESSES: [Harness; 6] = [
    Harness::Claude,
    Harness::Cursor,
    Harness::Codex,
    Harness::Opencode,
    Harness::Goose,
    Harness::Antigravity,
];

/// Covers a first `npx` download of the pinned CLI on a slow connection.
pub const DEFAULT_MCP_PROBE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub harness: &'static str,
    pub agent: &'static str,
    pub scope: &'static str,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn failed(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == CheckStatus::Fail)
    }
}

pub struct DoctorOptions {
    pub harness: Harness,
    pub scope: Option<InstallScope>,
    /// `None` skips starting the MCP server.
    pub mcp_probe_timeout: Option<Duration>,
}

pub fn doctor(options: &DoctorOptions) -> AdapterResult<Vec<DoctorReport>> {
    doctor_with_runtime(options, &RuntimePaths::current()?)
}

/// Agent setups with Codemod installed, project scope first.
pub(crate) fn installed_setups(runtime_paths: &RuntimePaths) -> Vec<(Harness, InstallScope)> {
    let mut setups = Vec::new();
    for scope in [InstallScope::Project, InstallScope::User] {
        for harness in SETUP_HARNESSES {
            if setup_exists(harness, scope, runtime_paths)
                && !setups.iter().any(|&(other_harness, other_scope)| {
                    other_harness == harness
                        && same_install_record(harness, other_scope, scope, runtime_paths)
                })
            {
                setups.push((harness, scope));
            }
        }
    }
    setups
}

/// Run from the home directory, project and user scope are the same files.
fn same_install_record(
    harness: Harness,
    left: InstallScope,
    right: InstallScope,
    runtime_paths: &RuntimePaths,
) -> bool {
    match (
        managed_state_path_for_harness(harness, left, runtime_paths),
        managed_state_path_for_harness(harness, right, runtime_paths),
    ) {
        (Ok(left), Ok(right)) => same_existing_path(&left, &right),
        _ => false,
    }
}

pub(crate) fn setup_exists(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> bool {
    if managed_state_path_for_harness(harness, scope, runtime_paths)
        .is_ok_and(|state_path| state_path.is_file())
    {
        return true;
    }
    // Codex and Antigravity share `.agents/skills` in a project, so only the
    // install record tells them apart.
    let shared_skill_root =
        scope == InstallScope::Project && matches!(harness, Harness::Codex | Harness::Antigravity);
    !shared_skill_root
        && core_skill_path(harness, scope, runtime_paths).is_some_and(|p| p.is_file())
}

fn core_skill_path(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> Option<PathBuf> {
    skills_root_for_harness(harness, scope, runtime_paths)
        .ok()
        .map(|root| root.join(MCS_SKILL_DIR_NAME).join(SKILL_FILE_NAME))
}

pub(crate) fn setup_command(harness: Harness, scope: InstallScope, subcommand: &str) -> String {
    let subcommand = if subcommand.is_empty() {
        String::new()
    } else {
        format!(" {subcommand}")
    };
    let user = if scope == InstallScope::User {
        " --user"
    } else {
        ""
    };
    format!(
        "npx codemod ai{subcommand} --harness {}{user}",
        harness.as_str()
    )
}

fn doctor_with_runtime(
    options: &DoctorOptions,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<DoctorReport>> {
    let scopes = match options.scope {
        Some(scope) => vec![scope],
        None => vec![InstallScope::Project, InstallScope::User],
    };
    let mut targets = installed_setups(runtime_paths)
        .into_iter()
        .filter(|(harness, scope)| {
            (options.harness == Harness::Auto || *harness == options.harness)
                && scopes.contains(scope)
        })
        .collect::<Vec<_>>();
    if targets.is_empty() && options.harness != Harness::Auto {
        targets.push((options.harness, scopes[0]));
    }

    Ok(targets
        .into_iter()
        .map(|(harness, scope)| DoctorReport {
            harness: harness.as_str(),
            agent: harness.display_name(),
            scope: scope.as_str(),
            checks: check_setup(harness, scope, options.mcp_probe_timeout, runtime_paths),
        })
        .collect())
}

fn check_setup(
    harness: Harness,
    scope: InstallScope,
    mcp_probe_timeout: Option<Duration>,
    runtime_paths: &RuntimePaths,
) -> Vec<DoctorCheck> {
    let reinstall = setup_command(harness, scope, "");
    if !setup_exists(harness, scope, runtime_paths) {
        return vec![fail(
            "Setup",
            format!(
                "Codemod isn't set up for {} {}.",
                harness.display_name(),
                scope_phrase(scope)
            ),
            reinstall,
        )];
    }

    let mut checks = vec![check_skill(harness, scope, runtime_paths)];
    if mcs_command_is_available(harness, scope) {
        checks.push(check_command(harness, scope, runtime_paths));
    }
    if harness_supports_mcp(harness) {
        checks.extend(check_mcp(harness, scope, mcp_probe_timeout, runtime_paths));
    }
    checks.extend(check_agent_loads_mcp(harness, scope, runtime_paths));
    checks.push(check_discovery_hints(harness, scope, runtime_paths));
    checks
}

fn scope_phrase(scope: InstallScope) -> &'static str {
    match scope {
        InstallScope::Project => "in this project",
        InstallScope::User => "for all your projects",
    }
}

fn pass(name: &'static str, detail: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        name,
        status: CheckStatus::Pass,
        detail: detail.into(),
        fix: None,
    }
}

fn warn(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        name,
        status: CheckStatus::Warn,
        detail: detail.into(),
        fix: Some(fix.into()),
    }
}

fn fail(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        name,
        status: CheckStatus::Fail,
        detail: detail.into(),
        fix: Some(fix.into()),
    }
}

fn display_path(path: &Path, runtime_paths: &RuntimePaths) -> String {
    if let Ok(relative) = path.strip_prefix(&runtime_paths.cwd) {
        return relative.display().to_string();
    }
    if let Some(relative) = runtime_paths
        .home_dir
        .as_ref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
}

fn check_skill(harness: Harness, scope: InstallScope, runtime_paths: &RuntimePaths) -> DoctorCheck {
    const NAME: &str = "Skill";
    let reinstall = setup_command(harness, scope, "");
    let Some(path) = core_skill_path(harness, scope, runtime_paths).filter(|p| p.is_file()) else {
        return fail(NAME, "The Codemod skill is missing.", reinstall);
    };
    let shown = display_path(&path, runtime_paths);
    let skill = InstalledSkill {
        name: MCS_SKILL_DIR_NAME.to_string(),
        path: path.clone(),
        version: None,
        scope: Some(scope),
    };
    let verification = verify_installed_skill(&skill);
    if verification.status == VerificationStatus::Fail {
        let reason = verification.reason.unwrap_or_default();
        return fail(
            NAME,
            format!("{shown} is broken: {reason}"),
            format!("{reinstall} --force"),
        );
    }
    match read_skill_version_marker(&path) {
        Ok(version) if version == MCS_SKILL_VERSION => {
            pass(NAME, format!("codemod {version} ({shown})"))
        }
        Ok(version) => warn(
            NAME,
            format!("codemod {version} is installed; this CLI has {MCS_SKILL_VERSION} ({shown})."),
            setup_command(harness, scope, "update"),
        ),
        Err(_) => fail(
            NAME,
            format!("{shown} has no version marker."),
            format!("{reinstall} --force"),
        ),
    }
}

fn check_command(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> DoctorCheck {
    const NAME: &str = "/codemod command";
    let path = match harness {
        Harness::Goose => goose_mcs_command_paths(scope, runtime_paths)
            .ok()
            .flatten()
            .map(|(recipe_path, _)| recipe_path),
        _ => mcs_command_entrypoint_path_for_harness(harness, scope, runtime_paths)
            .ok()
            .flatten(),
    };
    match path {
        Some(path) if path.is_file() => pass(NAME, display_path(&path, runtime_paths)),
        _ => warn(
            NAME,
            "The /codemod command is missing.",
            setup_command(harness, scope, ""),
        ),
    }
}

/// The command line an agent runs to start the Codemod MCP server.
#[derive(Debug, Eq, PartialEq)]
struct McpLaunch {
    command: Vec<String>,
    disabled: bool,
}

fn read_mcp_launch(harness: Harness, content: &str) -> Result<Option<McpLaunch>, String> {
    match harness {
        Harness::Codex => {
            let document = content
                .parse::<DocumentMut>()
                .map_err(|error| format!("isn't valid TOML: {error}"))?;
            let server = read_codex_mcp_server(&document).map_err(|error| error.to_string())?;
            let disabled = document
                .get("mcp_servers")
                .and_then(|servers| servers.get(MCP_SERVER_NAME))
                .and_then(|server| server.get("enabled"))
                .and_then(Item::as_bool)
                == Some(false);
            Ok(server.map(|server| McpLaunch {
                command: std::iter::once(server.command).chain(server.args).collect(),
                disabled,
            }))
        }
        Harness::Opencode => {
            let config = parse_jsonc_value(content)
                .map_err(|error| format!("isn't valid JSON: {error}"))?
                .unwrap_or(Value::Null);
            let Some(entry) = config.get("mcp").and_then(|mcp| mcp.get(MCP_SERVER_NAME)) else {
                return Ok(None);
            };
            let command = entry
                .get("command")
                .and_then(json_string_array)
                .filter(|command| !command.is_empty())
                .ok_or("has a `codemod` entry without a command list")?;
            Ok(Some(McpLaunch {
                command,
                disabled: entry.get("enabled").and_then(Value::as_bool) == Some(false),
            }))
        }
        _ => {
            let config: Value = serde_json::from_str(content)
                .map_err(|error| format!("isn't valid JSON: {error}"))?;
            let Some(entry) = config
                .get("mcpServers")
                .and_then(|servers| servers.get(MCP_SERVER_NAME))
            else {
                return Ok(None);
            };
            let command = entry
                .get("command")
                .and_then(Value::as_str)
                .ok_or("has a `codemod` entry without a command")?;
            let args = entry
                .get("args")
                .map(|args| json_string_array(args).ok_or("has non-string `args`"))
                .transpose()?
                .unwrap_or_default();
            Ok(Some(McpLaunch {
                command: std::iter::once(command.to_string()).chain(args).collect(),
                disabled: entry.get("disabled").and_then(Value::as_bool) == Some(true),
            }))
        }
    }
}

fn check_mcp(
    harness: Harness,
    scope: InstallScope,
    mcp_probe_timeout: Option<Duration>,
    runtime_paths: &RuntimePaths,
) -> Vec<DoctorCheck> {
    const CONFIG: &str = "MCP config";
    let reinstall = setup_command(harness, scope, "");
    let Ok(config_path) = mcp_config_path_for_harness(harness, scope, runtime_paths) else {
        return Vec::new();
    };
    let shown = display_path(&config_path, runtime_paths);
    let Ok(content) = fs::read_to_string(&config_path) else {
        return vec![fail(CONFIG, format!("{shown} is missing."), reinstall)];
    };
    let launch = match read_mcp_launch(harness, &content) {
        Ok(Some(launch)) => launch,
        Ok(None) => {
            return vec![fail(
                CONFIG,
                format!("{shown} has no `codemod` server."),
                reinstall,
            )]
        }
        Err(problem) => {
            return vec![fail(
                CONFIG,
                format!("{shown} {problem}"),
                format!("Fix {shown} by hand, then run `{reinstall}`."),
            )]
        }
    };
    let command_line = launch.command.join(" ");
    if launch.disabled {
        return vec![fail(
            CONFIG,
            format!("The `codemod` server in {shown} is turned off."),
            format!("Turn it back on in {shown}, or run `{reinstall} --force`."),
        )];
    }
    let mut checks = vec![pass(CONFIG, format!("{command_line} ({shown})"))];

    let program = &launch.command[0];
    let Some(program_path) = find_program(program, runtime_paths) else {
        let fix = if program == NPX_COMMAND {
            "Install Node.js, which includes npx, then restart your agent.".to_string()
        } else {
            format!("Install the Codemod CLI, or run `{reinstall} --force` to use npx instead.")
        };
        checks.push(fail(
            "MCP launcher",
            format!("`{program}` isn't installed or isn't on PATH."),
            fix,
        ));
        return checks;
    };
    checks.push(pass(
        "MCP launcher",
        format!("{program} ({})", program_path.display()),
    ));

    if let Some(timeout) = mcp_probe_timeout {
        let mut command = launch.command.clone();
        command[0] = program_path.to_string_lossy().to_string();
        let started = Instant::now();
        checks.push(
            match probe_mcp_server(&command, &runtime_paths.cwd, timeout) {
                Ok(answer) => pass(
                    "MCP server",
                    format!("{answer} in {:.1}s", started.elapsed().as_secs_f32()),
                ),
                Err(problem) => fail(
                    "MCP server",
                    format!("`{command_line}` {problem}"),
                    format!(
                        "Run `{command_line}` to see the full error, then `{reinstall} --force`."
                    ),
                ),
            },
        );
    }
    checks
}

fn find_program(program: &str, runtime_paths: &RuntimePaths) -> Option<PathBuf> {
    if is_explicit_command_path(program) {
        let path = PathBuf::from(program);
        return path.is_file().then_some(path);
    }
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(runtime_paths.path_env.as_ref()?)
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |extension| dir.join(format!("{program}{extension}")))
        })
        .find(|candidate| candidate.is_file())
}

/// Starts the server the way an agent does (stdio JSON-RPC), then asks for
/// its tools. Returns what it answered, or why it didn't.
pub(crate) fn probe_mcp_server(
    command: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("couldn't start: {error}"))?;

    let (responses, received) = mpsc::channel::<Value>();
    let stdout = child.stdout.take().expect("piped stdout");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(message) = serde_json::from_str::<Value>(&line) {
                if responses.send(message).is_err() {
                    break;
                }
            }
        }
    });
    let (stderr_lines, stderr_received) = mpsc::channel::<String>();
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if stderr_lines.send(line).is_err() {
                break;
            }
        }
    });

    let mut stdin = child.stdin.take().expect("piped stdin");
    let deadline = Instant::now() + timeout;
    let mut exchange = || -> Result<String, String> {
        send_json_rpc(
            &mut stdin,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "codemod-ai-doctor", "version": CODEMOD_CLI_VERSION }
                }
            }),
        )?;
        await_response(&received, 1, deadline)?;
        send_json_rpc(
            &mut stdin,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )?;
        send_json_rpc(
            &mut stdin,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        )?;
        let tools = await_response(&received, 2, deadline)?
            .get("tools")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        Ok(format!("started and answered with {tools} tools"))
    };
    let outcome = exchange();
    drop(stdin);
    let _ = child.kill();
    let status = child.wait().ok();

    outcome.map_err(|problem| {
        // The reader thread may still be draining stderr after the exit.
        let drain_until = Instant::now() + Duration::from_millis(500);
        let mut lines = Vec::new();
        while let Ok(line) =
            stderr_received.recv_timeout(drain_until.saturating_duration_since(Instant::now()))
        {
            lines.push(line);
        }
        match (
            status.and_then(|status| status.code()),
            telling_error_line(&lines),
        ) {
            (_, Some(line)) => format!("{problem}: {line}"),
            (Some(code), None) if problem.starts_with("exited") => {
                format!("{problem} with code {code}")
            }
            _ => problem,
        }
    })
}

/// The stderr line that says what went wrong: the first one mentioning an
/// error, skipping npm's error code and log-file lines, else the last line.
fn telling_error_line(lines: &[String]) -> Option<&str> {
    let lines = lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| {
            !line.is_empty()
                && !line.contains("complete log of this run")
                && !line.starts_with("npm error code ")
        })
        .collect::<Vec<_>>();
    lines
        .iter()
        .find(|line| line.to_lowercase().contains("error"))
        .or(lines.last())
        .copied()
}

fn send_json_rpc(stdin: &mut impl Write, message: Value) -> Result<(), String> {
    writeln!(stdin, "{message}")
        .and_then(|_| stdin.flush())
        .map_err(|_| "exited before answering".to_string())
}

fn await_response(
    received: &mpsc::Receiver<Value>,
    id: u64,
    deadline: Instant,
) -> Result<Value, String> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let message = received
            .recv_timeout(remaining)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => "didn't answer in time".to_string(),
                mpsc::RecvTimeoutError::Disconnected => "exited before answering".to_string(),
            })?;
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(format!("answered with an error: {text}"));
        }
        return Ok(message.get("result").cloned().unwrap_or(Value::Null));
    }
}

/// Some agents ignore a project's MCP config until the user approves it.
fn check_agent_loads_mcp(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> Option<DoctorCheck> {
    match (harness, scope) {
        (Harness::Claude, InstallScope::Project) => {
            Some(check_claude_project_approval(runtime_paths))
        }
        (Harness::Codex, InstallScope::Project) => Some(check_codex_folder_trust(runtime_paths)),
        _ => None,
    }
}

fn read_json_file(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or(Value::Null)
}

fn lists_codemod(settings: &Value, key: &str) -> bool {
    settings
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(|names| {
            names
                .iter()
                .any(|name| name.as_str() == Some(MCP_SERVER_NAME))
        })
}

fn approves_codemod(settings: &Value) -> bool {
    settings
        .get("enableAllProjectMcpServers")
        .and_then(Value::as_bool)
        == Some(true)
        || lists_codemod(settings, "enabledMcpjsonServers")
}

/// Mirrors how Claude Code decides whether a project `.mcp.json` server runs:
/// a rejection anywhere wins; approvals in user settings always count, while
/// project and local approvals count only once the folder is trusted.
fn check_claude_project_approval(runtime_paths: &RuntimePaths) -> DoctorCheck {
    const NAME: &str = "Claude Code approval";
    let user_settings = runtime_paths
        .home_dir
        .as_ref()
        .map(|home| read_json_file(&home.join(".claude/settings.json")))
        .unwrap_or(Value::Null);
    let project_settings = read_json_file(&runtime_paths.cwd.join(".claude/settings.json"));
    let local_settings = read_json_file(&runtime_paths.cwd.join(".claude/settings.local.json"));
    let claude_state = runtime_paths
        .home_dir
        .as_ref()
        .map(|home| read_json_file(&home.join(".claude.json")))
        .unwrap_or(Value::Null);
    let folder_states = runtime_paths
        .cwd
        .ancestors()
        .filter_map(|folder| {
            claude_state
                .get("projects")?
                .get(folder.to_string_lossy().as_ref())
                .cloned()
        })
        .collect::<Vec<_>>();

    let rejected = [&user_settings, &project_settings, &local_settings]
        .into_iter()
        .chain(&folder_states)
        .any(|settings| lists_codemod(settings, "disabledMcpjsonServers"));
    if rejected {
        return fail(
            NAME,
            "The codemod MCP server is turned off for this project in Claude Code.",
            "Run `claude` here, open `/mcp`, and enable codemod.",
        );
    }
    let trusted = folder_states
        .iter()
        .any(|state| state.get("hasTrustDialogAccepted").and_then(Value::as_bool) == Some(true));
    let approved = approves_codemod(&user_settings)
        || (trusted
            && [&project_settings, &local_settings]
                .into_iter()
                .chain(&folder_states)
                .any(approves_codemod));
    if approved {
        pass(NAME, "codemod MCP server approved")
    } else {
        warn(
            NAME,
            "Claude Code hasn't approved the codemod MCP server for this project yet.",
            "Run `claude` here and approve `codemod` when it asks. Check it with `/mcp`.",
        )
    }
}

fn check_codex_folder_trust(runtime_paths: &RuntimePaths) -> DoctorCheck {
    const NAME: &str = "Codex folder trust";
    let document = runtime_paths
        .codex_home
        .as_ref()
        .and_then(|codex_home| {
            fs::read_to_string(codex_home.join(CODEMOD_PERIODIC_TRIGGER_CODEX_CONFIG_FILE_NAME))
                .ok()
        })
        .and_then(|content| content.parse::<DocumentMut>().ok());
    let trust_level = runtime_paths.cwd.ancestors().find_map(|folder| {
        document
            .as_ref()?
            .get("projects")?
            .get(folder.to_string_lossy().as_ref())?
            .get("trust_level")?
            .as_str()
            .map(str::to_string)
    });
    match trust_level.as_deref() {
        Some("trusted") => pass(NAME, "this folder is trusted"),
        Some(_) => fail(
            NAME,
            "Codex marks this folder untrusted, so it ignores .codex/config.toml.",
            "Trust this folder in Codex, or run `npx codemod ai --harness codex --user`.",
        ),
        None => warn(
            NAME,
            "Codex hasn't trusted this folder yet and ignores .codex/config.toml until it does.",
            "Start `codex` here and trust the folder when it asks.",
        ),
    }
}

fn check_discovery_hints(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> DoctorCheck {
    const NAME: &str = "Discovery hints";
    let guides =
        discovery_guide_paths_with_runtime(harness, scope, runtime_paths).unwrap_or_default();
    let missing = guides
        .iter()
        .filter(|path| {
            !fs::read_to_string(path)
                .is_ok_and(|content| content.contains(SKILL_DISCOVERY_SECTION_BEGIN))
        })
        .map(|path| display_path(path, runtime_paths))
        .collect::<Vec<_>>();
    let shown = guides
        .iter()
        .map(|path| display_path(path, runtime_paths))
        .collect::<Vec<_>>()
        .join(", ");
    if missing.is_empty() {
        pass(NAME, shown)
    } else {
        warn(
            NAME,
            format!("No Codemod section in {}.", missing.join(", ")),
            setup_command(harness, scope, ""),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{full_install, runtime_paths_with_temp_roots, write_fake_executable};
    use super::*;

    const ANSWERING_SERVER: &str = r#"#!/bin/sh
read line
echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"codemod","version":"1.18.6"}}}'
read line
read line
echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"search"},{"name":"run"}]}}'
"#;

    fn check(harness: Harness, runtime_paths: &RuntimePaths, name: &str) -> DoctorCheck {
        check_setup(harness, InstallScope::Project, None, runtime_paths)
            .into_iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("no {name} check"))
    }

    fn with_npx(runtime_paths: &mut RuntimePaths, script: &str) {
        let bin = runtime_paths.cwd.parent().unwrap().join("path-bin");
        write_fake_executable(&bin.join(NPX_COMMAND), script);
        runtime_paths.path_env = Some(bin.into_os_string());
    }

    fn write_json(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
    }

    fn trust_claude_folder(runtime_paths: &RuntimePaths) {
        let folder = runtime_paths.cwd.to_string_lossy().to_string();
        write_json(
            &runtime_paths
                .home_dir
                .as_ref()
                .unwrap()
                .join(".claude.json"),
            json!({ "projects": { folder: { "hasTrustDialogAccepted": true } } }),
        );
    }

    fn write_codex_trust(runtime_paths: &RuntimePaths, folder: &Path, level: &str) {
        let config = runtime_paths
            .codex_home
            .as_ref()
            .unwrap()
            .join("config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            format!(
                "[projects.\"{}\"]\ntrust_level = \"{level}\"\n",
                folder.display()
            ),
        )
        .unwrap();
    }

    #[test]
    fn finds_no_setup_in_a_project_codemod_was_never_installed_in() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        assert!(installed_setups(&runtime_paths).is_empty());
    }

    #[test]
    fn finds_each_agent_codemod_was_set_up_for() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        full_install(Harness::Codex, InstallScope::User, &runtime_paths);
        assert_eq!(
            installed_setups(&runtime_paths),
            vec![
                (Harness::Cursor, InstallScope::Project),
                (Harness::Codex, InstallScope::User)
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fresh_setup_passes_every_check_when_the_mcp_server_answers() {
        let (mut runtime_paths, _temp) = runtime_paths_with_temp_roots();
        with_npx(&mut runtime_paths, ANSWERING_SERVER);
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let checks = check_setup(
            Harness::Cursor,
            InstallScope::Project,
            Some(Duration::from_secs(10)),
            &runtime_paths,
        );
        let not_passing = checks
            .iter()
            .filter(|check| check.status != CheckStatus::Pass)
            .map(|check| format!("{}: {}", check.name, check.detail))
            .collect::<Vec<_>>();
        assert_eq!(not_passing, Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn the_mcp_server_check_shows_npms_error_when_the_server_exits_early() {
        let (mut runtime_paths, _temp) = runtime_paths_with_temp_roots();
        with_npx(
            &mut runtime_paths,
            "#!/bin/sh\necho 'npm error code E404' >&2\necho 'npm error 404 Not Found - codemod' >&2\necho 'npm error A complete log of this run can be found in: /tmp/x.log' >&2\nexit 1\n",
        );
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let server = check_setup(
            Harness::Cursor,
            InstallScope::Project,
            Some(Duration::from_secs(10)),
            &runtime_paths,
        )
        .into_iter()
        .find(|check| check.name == "MCP server")
        .unwrap();
        assert!(
            server.status == CheckStatus::Fail
                && server.detail.ends_with("npm error 404 Not Found - codemod"),
            "{server:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_mcp_server_check_fails_when_the_server_never_answers() {
        let (mut runtime_paths, _temp) = runtime_paths_with_temp_roots();
        with_npx(&mut runtime_paths, "#!/bin/sh\nexec sleep 5\n");
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let server = check_setup(
            Harness::Cursor,
            InstallScope::Project,
            Some(Duration::from_millis(100)),
            &runtime_paths,
        )
        .into_iter()
        .find(|check| check.name == "MCP server")
        .unwrap();
        assert_eq!(server.status, CheckStatus::Fail);
    }

    #[test]
    fn the_launcher_check_says_to_install_node_when_npx_is_missing() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        assert_eq!(
            check(Harness::Cursor, &runtime_paths, "MCP launcher")
                .fix
                .as_deref(),
            Some("Install Node.js, which includes npx, then restart your agent.")
        );
    }

    #[test]
    fn the_mcp_config_check_fails_when_the_codemod_server_was_removed() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        write_json(
            &runtime_paths.cwd.join(".cursor/mcp.json"),
            json!({ "mcpServers": {} }),
        );
        assert_eq!(
            check(Harness::Cursor, &runtime_paths, "MCP config").status,
            CheckStatus::Fail
        );
    }

    #[test]
    fn the_mcp_config_check_fails_when_the_codemod_server_is_turned_off() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        let config = runtime_paths.cwd.join(".mcp.json");
        let mut value: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        value["mcpServers"]["codemod"]["disabled"] = json!(true);
        write_json(&config, value);
        assert_eq!(
            check(Harness::Claude, &runtime_paths, "MCP config").status,
            CheckStatus::Fail
        );
    }

    #[test]
    fn an_older_skill_warns_and_suggests_the_update_command() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let skill =
            core_skill_path(Harness::Cursor, InstallScope::Project, &runtime_paths).unwrap();
        let content = fs::read_to_string(&skill).unwrap().replace(
            &format!("codemod-skill-version: {MCS_SKILL_VERSION}"),
            "codemod-skill-version: 0.9.0",
        );
        fs::write(&skill, content).unwrap();
        assert_eq!(
            check(Harness::Cursor, &runtime_paths, "Skill")
                .fix
                .as_deref(),
            Some("npx codemod ai update --harness cursor")
        );
    }

    #[test]
    fn claude_project_server_waits_for_approval_after_a_fresh_setup() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        assert_eq!(
            check(Harness::Claude, &runtime_paths, "Claude Code approval").status,
            CheckStatus::Warn
        );
    }

    #[test]
    fn claude_project_server_is_approved_by_enabling_all_project_servers_in_user_settings() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        write_json(
            &runtime_paths
                .home_dir
                .as_ref()
                .unwrap()
                .join(".claude/settings.json"),
            json!({ "enableAllProjectMcpServers": true }),
        );
        assert_eq!(
            check(Harness::Claude, &runtime_paths, "Claude Code approval").status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn claude_local_approval_counts_once_the_folder_is_trusted() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        write_json(
            &runtime_paths.cwd.join(".claude/settings.local.json"),
            json!({ "enabledMcpjsonServers": ["codemod"] }),
        );
        trust_claude_folder(&runtime_paths);
        assert_eq!(
            check(Harness::Claude, &runtime_paths, "Claude Code approval").status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn claude_local_approval_does_not_count_in_an_untrusted_folder() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        write_json(
            &runtime_paths.cwd.join(".claude/settings.local.json"),
            json!({ "enabledMcpjsonServers": ["codemod"] }),
        );
        assert_eq!(
            check(Harness::Claude, &runtime_paths, "Claude Code approval").status,
            CheckStatus::Warn
        );
    }

    #[test]
    fn claude_project_server_fails_when_it_was_rejected() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        write_json(
            &runtime_paths.cwd.join(".claude/settings.local.json"),
            json!({ "disabledMcpjsonServers": ["codemod"] }),
        );
        assert_eq!(
            check(Harness::Claude, &runtime_paths, "Claude Code approval").status,
            CheckStatus::Fail
        );
    }

    #[test]
    fn codex_waits_until_the_project_folder_is_trusted() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Codex, InstallScope::Project, &runtime_paths);
        assert_eq!(
            check(Harness::Codex, &runtime_paths, "Codex folder trust").status,
            CheckStatus::Warn
        );
    }

    #[test]
    fn codex_trust_of_a_parent_folder_covers_the_project() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Codex, InstallScope::Project, &runtime_paths);
        write_codex_trust(
            &runtime_paths,
            runtime_paths.cwd.parent().unwrap(),
            "trusted",
        );
        assert_eq!(
            check(Harness::Codex, &runtime_paths, "Codex folder trust").status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn codex_fails_when_the_folder_is_marked_untrusted() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Codex, InstallScope::Project, &runtime_paths);
        write_codex_trust(&runtime_paths, &runtime_paths.cwd, "untrusted");
        assert_eq!(
            check(Harness::Codex, &runtime_paths, "Codex folder trust").status,
            CheckStatus::Fail
        );
    }
}
