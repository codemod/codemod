use crate::utils::env_paths::home_dir_from_env;
use crate::utils::skill_layout::SKILL_FILE_NAME;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use toml_edit::{value, Array, DocumentMut, Item, Table};

const MCS_SKILL_COMPONENT_ID: &str = "codemod";
const MCS_SKILL_DIR_NAME: &str = "codemod";
const MCS_SKILL_VERSION: &str = "1.0.0";
const SKILL_PACKAGE_COMPATIBILITY_MARKER: &str = "codemod-compatibility: skill-package-v1";
const CODEMOD_COMPATIBILITY_MARKER_PREFIX: &str = "codemod-compatibility:";
const MCS_COMPATIBILITY_MARKER: &str = "codemod-compatibility: mcs-v1";
const MCS_VERSION_MARKER: &str = "codemod-skill-version: 1.0.0";
const CODEMOD_VERSION_MARKER_PREFIX: &str = "codemod-skill-version:";
const MCP_SERVER_NAME: &str = "codemod";
const CODEMOD_CLI_COMMAND: &str = "codemod";
const NPX_COMMAND: &str = "npx";
const MCP_SERVER_ARG_PACKAGE: &str = "codemod@latest";
const MCP_SERVER_ARG_COMMAND: &str = "mcp";
const MCS_REFERENCE_INDEX_RELATIVE_PATH: &str = "references/index.md";
const MCS_AI_NATIVE_RECIPES_RELATIVE_PATH: &str = "references/ai-native/recipes.md";
const MCS_CREATE_CODEMODS_RELATIVE_PATH: &str = "references/core/create-codemods.md";
const MCS_MAINTAINER_MONOREPO_RELATIVE_PATH: &str = "references/core/maintainer-monorepo.md";
const MCS_SEARCH_DISCOVERY_RELATIVE_PATH: &str = "references/core/search-and-discovery.md";
const MCS_SCAFFOLD_RUN_RELATIVE_PATH: &str = "references/core/scaffold-and-run.md";
const MCS_DRY_RUN_VERIFY_RELATIVE_PATH: &str = "references/core/dry-run-and-verify.md";
const MCS_TROUBLESHOOTING_RELATIVE_PATH: &str = "references/core/troubleshooting.md";
const REQUIRED_FRONTMATTER_KEYS: [&str; 3] = ["name:", "description:", "allowed-tools:"];
const MCS_SKILL_MD: &str = include_str!("../templates/ai-native-cli/codemod-cli/SKILL.md");
const MCS_REFERENCE_INDEX_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/index.md");
const MCS_AI_NATIVE_RECIPES_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/ai-native/recipes.md");
const MCS_CREATE_CODEMODS_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/core/create-codemods.md");
const MCS_MAINTAINER_MONOREPO_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/core/maintainer-monorepo.md");
const MCS_SEARCH_DISCOVERY_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/core/search-and-discovery.md");
const MCS_SCAFFOLD_RUN_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/core/scaffold-and-run.md");
const MCS_DRY_RUN_VERIFY_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/core/dry-run-and-verify.md");
const MCS_TROUBLESHOOTING_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/core/troubleshooting.md");
const MCS_REFERENCE_FILES: [(&str, &str); 8] = [
    (MCS_REFERENCE_INDEX_RELATIVE_PATH, MCS_REFERENCE_INDEX_MD),
    (
        MCS_AI_NATIVE_RECIPES_RELATIVE_PATH,
        MCS_AI_NATIVE_RECIPES_MD,
    ),
    (MCS_CREATE_CODEMODS_RELATIVE_PATH, MCS_CREATE_CODEMODS_MD),
    (
        MCS_MAINTAINER_MONOREPO_RELATIVE_PATH,
        MCS_MAINTAINER_MONOREPO_MD,
    ),
    (MCS_SEARCH_DISCOVERY_RELATIVE_PATH, MCS_SEARCH_DISCOVERY_MD),
    (MCS_SCAFFOLD_RUN_RELATIVE_PATH, MCS_SCAFFOLD_RUN_MD),
    (MCS_DRY_RUN_VERIFY_RELATIVE_PATH, MCS_DRY_RUN_VERIFY_MD),
    (MCS_TROUBLESHOOTING_RELATIVE_PATH, MCS_TROUBLESHOOTING_MD),
];
const MCS_INDEX_LINKED_REFERENCE_PATHS: [&str; 7] = [
    MCS_AI_NATIVE_RECIPES_RELATIVE_PATH,
    MCS_CREATE_CODEMODS_RELATIVE_PATH,
    MCS_MAINTAINER_MONOREPO_RELATIVE_PATH,
    MCS_SEARCH_DISCOVERY_RELATIVE_PATH,
    MCS_SCAFFOLD_RUN_RELATIVE_PATH,
    MCS_DRY_RUN_VERIFY_RELATIVE_PATH,
    MCS_TROUBLESHOOTING_RELATIVE_PATH,
];
const SKILL_DISCOVERY_SECTION_BEGIN: &str = "<!-- codemod-skill-discovery:begin -->";
const SKILL_DISCOVERY_SECTION_END: &str = "<!-- codemod-skill-discovery:end -->";
const AGENTS_GUIDE_FILE_NAME: &str = "AGENTS.md";
const CLAUDE_GUIDE_FILE_NAME: &str = "CLAUDE.md";
const CODEMOD_MANAGED_STATE_SCHEMA_VERSION: &str = "1";
const CODEMOD_MANAGED_STATE_RELATIVE_PATH: &str = "codemod/managed-install-state.json";
const CODEMOD_MANAGED_STATE_LOCK_TIMEOUT_MILLIS: u64 = 3_000;
const CODEMOD_MANAGED_STATE_LOCK_RETRY_MILLIS: u64 = 200;
const CODEMOD_MANAGED_STATE_LOCK_STALE_SECS: u64 = 600;
const CODEMOD_PERIODIC_UPDATE_RELATIVE_DIR: &str = "codemod/periodic-update";
const CODEMOD_PERIODIC_UPDATE_RUNNER_FILE_NAME: &str = "check-updates.sh";
const CODEMOD_PERIODIC_UPDATE_STATE_FILE_NAME: &str = "last-check-epoch-secs";
const CODEMOD_PERIODIC_UPDATE_DEFAULT_INTERVAL_SECS: u64 = 21_600;
const CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_FILE_NAME: &str = ".goosehints";
const CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_BEGIN: &str = "<!-- codemod-periodic-update:begin -->";
const CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_END: &str = "<!-- codemod-periodic-update:end -->";
const CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOKS_FILE_NAME: &str = "hooks.json";
const CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOK_EVENT_NAME: &str = "afterAgentResponse";
const CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_DIR_NAME: &str = "plugins";
const CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_FILE_NAME: &str = "codemod-periodic-update.js";
const CODEMOD_PERIODIC_TRIGGER_OPENCODE_USER_CONFIG_RELATIVE_DIR: &str = ".config/opencode";
const CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_EVENT_NAME: &str = "session.idle";
const CODEMOD_PERIODIC_TRIGGER_CLAUDE_SETTINGS_FILE_NAME: &str = "settings.json";
const CODEMOD_PERIODIC_TRIGGER_CLAUDE_SESSION_START_EVENT: &str = "SessionStart";
const CODEMOD_PERIODIC_TRIGGER_CODEX_CONFIG_FILE_NAME: &str = "config.toml";
const CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_FILE_NAME: &str = "CODEMOD_PERIODIC_UPDATE.md";
const CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_BEGIN: &str =
    "<!-- codemod-periodic-update:begin -->";
const CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_END: &str = "<!-- codemod-periodic-update:end -->";
const CODEX_CONFIG_DIR_NAME: &str = ".codex";
const CODEX_WORKSPACE_SKILLS_RELATIVE_PATH: &str = ".agents/skills";
const ANTIGRAVITY_WORKSPACE_ROOT: &str = ".agents";
const ANTIGRAVITY_USER_ROOT: &str = ".gemini/antigravity";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillPackageInstallSpec {
    pub id: String,
    pub version: String,
    pub description: String,
    pub source_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Harness {
    #[default]
    Auto,
    Claude,
    Goose,
    Opencode,
    Cursor,
    Codex,
    Antigravity,
}

impl Harness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Claude => "claude",
            Self::Goose => "goose",
            Self::Opencode => "opencode",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Logs,
    Table,
    Json,
    Yaml,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::Table => "table",
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum PeriodicUpdatePolicy {
    Manual,
    Notify,
    AutoSafe,
}

impl Default for PeriodicUpdatePolicy {
    fn default() -> Self {
        Self::AutoSafe
    }
}

impl PeriodicUpdatePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Notify => "notify",
            Self::AutoSafe => "auto-safe",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallScope {
    Project,
    User,
}

impl InstallScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallRequest {
    pub scope: InstallScope,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedComponentKind {
    Skill,
    McpConfig,
    DiscoveryGuide,
}

impl ManagedComponentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::McpConfig => "mcp_config",
            Self::DiscoveryGuide => "discovery_guide",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedComponentSnapshot {
    pub id: String,
    pub kind: ManagedComponentKind,
    pub path: PathBuf,
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedStateWriteStatus {
    Created,
    Updated,
    Unchanged,
}

impl ManagedStateWriteStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedStateWriteResult {
    pub path: PathBuf,
    pub status: ManagedStateWriteStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedStateReadResult {
    pub path: PathBuf,
    pub components: Vec<ManagedComponentSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeriodicUpdateTriggerUpsertResult {
    pub tracked_paths: Vec<PathBuf>,
    pub updated_paths: Vec<PathBuf>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PeriodicUpdateIntegrationKind {
    Hook,
    Plugin,
    Guidance,
    Notify,
}

impl PeriodicUpdateIntegrationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Plugin => "plugin",
            Self::Guidance => "guidance",
            Self::Notify => "notify",
        }
    }
}

#[derive(Clone, Debug)]
struct PeriodicUpdateTriggerStrategy {
    integration_path: PathBuf,
    integration_kind: PeriodicUpdateIntegrationKind,
    upsert: fn(&Path, &Path) -> AdapterResult<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledSkill {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub scope: Option<InstallScope>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum VerificationStatus {
    Pass,
    Fail,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationCheck {
    pub skill: String,
    pub scope: Option<InstallScope>,
    pub status: VerificationStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ManagedInstallState {
    schema_version: String,
    harness: String,
    scope: String,
    components: Vec<ManagedInstallStateComponent>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ManagedInstallStateComponent {
    id: String,
    kind: String,
    path: String,
    version: Option<String>,
    fingerprint: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct ManagedStateLockPolicy {
    timeout: Duration,
    retry_interval: Duration,
    stale_after: Duration,
}

impl ManagedStateLockPolicy {
    fn default_policy() -> Self {
        Self {
            timeout: Duration::from_millis(CODEMOD_MANAGED_STATE_LOCK_TIMEOUT_MILLIS),
            retry_interval: Duration::from_millis(CODEMOD_MANAGED_STATE_LOCK_RETRY_MILLIS),
            stale_after: Duration::from_secs(CODEMOD_MANAGED_STATE_LOCK_STALE_SECS),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ManagedStateLockMetadata {
    pid: u32,
    acquired_at_epoch_secs: u64,
}

#[derive(Debug)]
struct ManagedStateLockGuard {
    path: PathBuf,
    released: bool,
}

impl ManagedStateLockGuard {
    fn release(mut self) {
        self.released = true;
        let _ = release_managed_state_lock(&self.path);
    }
}

impl Drop for ManagedStateLockGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let _ = release_managed_state_lock(&self.path);
    }
}

#[derive(Debug, Error)]
pub enum HarnessAdapterError {
    #[error("Unsupported harness: {0}")]
    UnsupportedHarness(String),
    #[error("Invalid skill package: {0}")]
    InvalidSkillPackage(String),
    #[error("Skill install failed: {0}")]
    InstallFailed(String),
    #[error("Unknown skill package id: {0}")]
    SkillPackageNotFound(String),
    #[error("Skill package install failed: {0}")]
    SkillPackageInstallFailed(String),
}

impl HarnessAdapterError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedHarness(_) => "E_UNSUPPORTED_HARNESS",
            Self::InvalidSkillPackage(_) => "E_SKILL_INVALID",
            Self::InstallFailed(_) => "E_SKILL_INSTALL_FAILED",
            Self::SkillPackageNotFound(_) => "E_SKILL_PACKAGE_NOT_FOUND",
            Self::SkillPackageInstallFailed(_) => "E_SKILL_PACKAGE_INSTALL_FAILED",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::UnsupportedHarness(_) => 20,
            Self::InvalidSkillPackage(_) => 21,
            Self::InstallFailed(_) => 22,
            Self::SkillPackageNotFound(_) => 27,
            Self::SkillPackageInstallFailed(_) => 28,
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Self::UnsupportedHarness(_) => {
                "Use --harness claude, --harness goose, --harness opencode, --harness cursor, --harness codex, or --harness antigravity."
            }
            Self::InvalidSkillPackage(_) => {
                "Retry with `codemod agent install --force` and inspect installed entries via `codemod agent list --format json`."
            }
            Self::InstallFailed(_) => "Retry with --force or check filesystem permissions.",
            Self::SkillPackageNotFound(_) => {
                "Run `codemod search <migration> --format json` to locate a valid package id."
            }
            Self::SkillPackageInstallFailed(_) => "Retry with --force or check filesystem permissions.",
        }
    }
}

pub type AdapterResult<T> = std::result::Result<T, HarnessAdapterError>;

pub trait HarnessAdapter: Send + Sync {
    fn install_skills(&self, request: &InstallRequest) -> AdapterResult<Vec<InstalledSkill>>;
    fn list_skills(&self) -> AdapterResult<Vec<InstalledSkill>>;
    fn verify_skills(&self) -> AdapterResult<Vec<VerificationCheck>>;
}

pub struct ResolvedAdapter {
    pub adapter: Box<dyn HarnessAdapter>,
    pub harness: Harness,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimePaths {
    cwd: PathBuf,
    home_dir: Option<PathBuf>,
    current_executable: Option<PathBuf>,
}

impl RuntimePaths {
    fn current() -> AdapterResult<Self> {
        Self::for_context(None, None)
    }

    fn for_context(
        working_directory: Option<&Path>,
        environment: Option<&HashMap<String, String>>,
    ) -> AdapterResult<Self> {
        let cwd = match working_directory {
            Some(path) => path.to_path_buf(),
            None => std::env::current_dir().map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "Unable to read current working directory: {error}"
                ))
            })?,
        };

        Ok(Self {
            cwd,
            home_dir: home_dir_from_env(environment),
            current_executable: std::env::current_exe().ok(),
        })
    }
}

pub(crate) fn runtime_paths_for_execution(
    working_directory: Option<&Path>,
    environment: Option<&HashMap<String, String>>,
) -> AdapterResult<RuntimePaths> {
    RuntimePaths::for_context(working_directory, environment)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodemodCliInvocation {
    command: String,
    args_prefix: Vec<String>,
}

impl CodemodCliInvocation {
    fn with_args(&self, additional_args: &[String]) -> Vec<String> {
        let mut args = self.args_prefix.clone();
        args.extend_from_slice(additional_args);
        args
    }
}

#[derive(Debug, Default)]
pub struct ClaudeHarnessAdapter;

impl HarnessAdapter for ClaudeHarnessAdapter {
    fn install_skills(&self, request: &InstallRequest) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        install_mcs_skill_bundle_with_runtime(Harness::Claude, request, &runtime_paths)
    }

    fn list_skills(&self) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        list_skills_with_runtime(Harness::Claude, &runtime_paths)
    }

    fn verify_skills(&self) -> AdapterResult<Vec<VerificationCheck>> {
        let runtime_paths = RuntimePaths::current()?;
        verify_skills_with_runtime(Harness::Claude, &runtime_paths)
    }
}

#[derive(Debug, Default)]
pub struct GooseHarnessAdapter;

impl HarnessAdapter for GooseHarnessAdapter {
    fn install_skills(&self, request: &InstallRequest) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        install_mcs_skill_bundle_with_runtime(Harness::Goose, request, &runtime_paths)
    }

    fn list_skills(&self) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        list_skills_with_runtime(Harness::Goose, &runtime_paths)
    }

    fn verify_skills(&self) -> AdapterResult<Vec<VerificationCheck>> {
        let runtime_paths = RuntimePaths::current()?;
        verify_skills_with_runtime(Harness::Goose, &runtime_paths)
    }
}

#[derive(Debug, Default)]
pub struct OpencodeHarnessAdapter;

impl HarnessAdapter for OpencodeHarnessAdapter {
    fn install_skills(&self, request: &InstallRequest) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        install_mcs_skill_bundle_with_runtime(Harness::Opencode, request, &runtime_paths)
    }

    fn list_skills(&self) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        list_skills_with_runtime(Harness::Opencode, &runtime_paths)
    }

    fn verify_skills(&self) -> AdapterResult<Vec<VerificationCheck>> {
        let runtime_paths = RuntimePaths::current()?;
        verify_skills_with_runtime(Harness::Opencode, &runtime_paths)
    }
}

#[derive(Debug, Default)]
pub struct CursorHarnessAdapter;

impl HarnessAdapter for CursorHarnessAdapter {
    fn install_skills(&self, request: &InstallRequest) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        install_mcs_skill_bundle_with_runtime(Harness::Cursor, request, &runtime_paths)
    }

    fn list_skills(&self) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        list_skills_with_runtime(Harness::Cursor, &runtime_paths)
    }

    fn verify_skills(&self) -> AdapterResult<Vec<VerificationCheck>> {
        let runtime_paths = RuntimePaths::current()?;
        verify_skills_with_runtime(Harness::Cursor, &runtime_paths)
    }
}

#[derive(Debug, Default)]
pub struct CodexHarnessAdapter;

impl HarnessAdapter for CodexHarnessAdapter {
    fn install_skills(&self, request: &InstallRequest) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        install_mcs_skill_bundle_with_runtime(Harness::Codex, request, &runtime_paths)
    }

    fn list_skills(&self) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        list_skills_with_runtime(Harness::Codex, &runtime_paths)
    }

    fn verify_skills(&self) -> AdapterResult<Vec<VerificationCheck>> {
        let runtime_paths = RuntimePaths::current()?;
        verify_skills_with_runtime(Harness::Codex, &runtime_paths)
    }
}

#[derive(Debug, Default)]
pub struct AntigravityHarnessAdapter;

impl HarnessAdapter for AntigravityHarnessAdapter {
    fn install_skills(&self, request: &InstallRequest) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        install_mcs_skill_bundle_with_runtime(Harness::Antigravity, request, &runtime_paths)
    }

    fn list_skills(&self) -> AdapterResult<Vec<InstalledSkill>> {
        let runtime_paths = RuntimePaths::current()?;
        list_skills_with_runtime(Harness::Antigravity, &runtime_paths)
    }

    fn verify_skills(&self) -> AdapterResult<Vec<VerificationCheck>> {
        let runtime_paths = RuntimePaths::current()?;
        verify_skills_with_runtime(Harness::Antigravity, &runtime_paths)
    }
}

pub fn resolve_adapter(harness: Harness) -> AdapterResult<ResolvedAdapter> {
    let runtime_paths = RuntimePaths::current()?;
    resolve_adapter_with_runtime(harness, &runtime_paths)
}

pub(crate) fn resolve_adapter_with_runtime(
    harness: Harness,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<ResolvedAdapter> {
    let (resolved_harness, warnings) = match harness {
        Harness::Auto => detect_auto_harness(&runtime_paths.cwd),
        Harness::Claude => (Harness::Claude, Vec::new()),
        Harness::Goose => (Harness::Goose, Vec::new()),
        Harness::Opencode => (Harness::Opencode, Vec::new()),
        Harness::Cursor => (Harness::Cursor, Vec::new()),
        Harness::Codex => (Harness::Codex, Vec::new()),
        Harness::Antigravity => (Harness::Antigravity, Vec::new()),
    };

    let adapter: Box<dyn HarnessAdapter> = match resolved_harness {
        Harness::Claude => Box::new(ClaudeHarnessAdapter),
        Harness::Goose => Box::new(GooseHarnessAdapter),
        Harness::Opencode => Box::new(OpencodeHarnessAdapter),
        Harness::Cursor => Box::new(CursorHarnessAdapter),
        Harness::Codex => Box::new(CodexHarnessAdapter),
        Harness::Antigravity => Box::new(AntigravityHarnessAdapter),
        Harness::Auto => return Err(HarnessAdapterError::UnsupportedHarness("auto".to_string())),
    };

    Ok(ResolvedAdapter {
        adapter,
        harness: resolved_harness,
        warnings,
    })
}

fn detect_auto_harness(cwd: &Path) -> (Harness, Vec<String>) {
    for (harness, root_dir) in [
        (Harness::Claude, ".claude"),
        (Harness::Goose, ".goose"),
        (Harness::Opencode, ".opencode"),
        (Harness::Cursor, ".cursor"),
        (Harness::Codex, ".codex"),
        (Harness::Antigravity, ".agents"),
    ] {
        if cwd.join(root_dir).exists() {
            return (harness, Vec::new());
        }
    }

    (
        Harness::Claude,
        vec![
            "No .claude, .goose, .opencode, .cursor, .codex, or .agents directory found; defaulting to Claude harness.".to_string(),
        ],
    )
}

pub fn resolve_install_scope(project: bool, user: bool) -> AdapterResult<InstallScope> {
    match (project, user) {
        (true, true) => Err(HarnessAdapterError::InstallFailed(
            "Conflicting scope flags: use either --project or --user".to_string(),
        )),
        (true, false) => Ok(InstallScope::Project),
        (false, true) => Ok(InstallScope::User),
        (false, false) => Ok(InstallScope::Project),
    }
}

pub fn install_restart_hint(harness: Harness) -> String {
    format!(
        "Restart or reload your {} session so newly installed skills are picked up.",
        harness.as_str()
    )
}

pub fn upsert_skill_discovery_guides(
    harness: Harness,
    scope: InstallScope,
) -> AdapterResult<Vec<PathBuf>> {
    let runtime_paths = RuntimePaths::current()?;
    upsert_skill_discovery_guides_with_runtime(harness, scope, &runtime_paths)
}

pub fn skill_discovery_guide_paths(
    harness: Harness,
    scope: InstallScope,
) -> AdapterResult<Vec<PathBuf>> {
    let runtime_paths = RuntimePaths::current()?;
    discovery_guide_paths_with_runtime(harness, scope, &runtime_paths)
}

pub fn upsert_periodic_update_trigger(
    harness: Harness,
    scope: InstallScope,
    periodic_policy: PeriodicUpdatePolicy,
) -> AdapterResult<PeriodicUpdateTriggerUpsertResult> {
    let runtime_paths = RuntimePaths::current()?;
    upsert_periodic_update_trigger_with_runtime(harness, scope, periodic_policy, &runtime_paths)
}

pub fn mcs_install_requires_force(harness: Harness, scope: InstallScope) -> AdapterResult<bool> {
    let runtime_paths = RuntimePaths::current()?;
    mcs_install_requires_force_with_runtime(harness, scope, &runtime_paths)
}

fn upsert_periodic_update_trigger_with_runtime(
    harness: Harness,
    scope: InstallScope,
    periodic_policy: PeriodicUpdatePolicy,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<PeriodicUpdateTriggerUpsertResult> {
    let harness_root = harness_root_for_scope(harness, scope, runtime_paths)?;
    let trigger_strategy =
        periodic_update_trigger_strategy(harness, scope, runtime_paths, &harness_root)?;
    let periodic_root = harness_root.join(CODEMOD_PERIODIC_UPDATE_RELATIVE_DIR);
    let runner_path = periodic_root.join(CODEMOD_PERIODIC_UPDATE_RUNNER_FILE_NAME);
    let state_path = periodic_root.join(CODEMOD_PERIODIC_UPDATE_STATE_FILE_NAME);

    let tracked_paths = vec![
        runner_path.clone(),
        trigger_strategy.integration_path.clone(),
    ];
    let mut updated_paths = Vec::new();
    let mut notes = Vec::new();

    let runner_updated = write_periodic_update_runner_script(
        harness,
        scope,
        periodic_policy,
        &runner_path,
        &state_path,
        runtime_paths,
    )?;
    if runner_updated {
        updated_paths.push(runner_path.clone());
    }

    if (trigger_strategy.upsert)(&trigger_strategy.integration_path, &runner_path)? {
        updated_paths.push(trigger_strategy.integration_path.clone());
    }

    if runner_updated {
        notes.push(format!(
            "Installed periodic update runner: {}",
            runner_path.display()
        ));
        notes.push(
            "Periodic update manifest signature verification is enforced (`--require-signed-manifest` is baked into periodic runner command).".to_string(),
        );
    }
    if updated_paths
        .iter()
        .any(|path| path == &trigger_strategy.integration_path)
    {
        notes.push(format!(
            "Updated periodic update {}: {}",
            trigger_strategy.integration_kind.as_str(),
            trigger_strategy.integration_path.display()
        ));
    }

    Ok(PeriodicUpdateTriggerUpsertResult {
        tracked_paths,
        updated_paths,
        notes,
    })
}

fn harness_root_for_scope(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<PathBuf> {
    match scope {
        InstallScope::Project => Ok(match harness {
            Harness::Claude => runtime_paths.cwd.join(".claude"),
            Harness::Goose => runtime_paths.cwd.join(".goose"),
            Harness::Opencode => runtime_paths.cwd.join(".opencode"),
            Harness::Cursor => runtime_paths.cwd.join(".cursor"),
            Harness::Codex => runtime_paths.cwd.join(CODEX_CONFIG_DIR_NAME),
            Harness::Antigravity => runtime_paths.cwd.join(ANTIGRAVITY_WORKSPACE_ROOT),
            Harness::Auto => {
                return Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()));
            }
        }),
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| match harness {
                Harness::Claude => home.join(".claude"),
                Harness::Goose => home.join(".goose"),
                Harness::Opencode => home.join(".opencode"),
                Harness::Cursor => home.join(".cursor"),
                Harness::Codex => home.join(CODEX_CONFIG_DIR_NAME),
                Harness::Antigravity => home.join(ANTIGRAVITY_USER_ROOT),
                Harness::Auto => PathBuf::new(),
            })
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            })
            .and_then(|root| {
                if root.as_os_str().is_empty() {
                    Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()))
                } else {
                    Ok(root)
                }
            }),
    }
}

fn periodic_update_trigger_strategy(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
    harness_root: &Path,
) -> AdapterResult<PeriodicUpdateTriggerStrategy> {
    match harness {
        Harness::Claude => Ok(PeriodicUpdateTriggerStrategy {
            integration_path: harness_root.join(CODEMOD_PERIODIC_TRIGGER_CLAUDE_SETTINGS_FILE_NAME),
            integration_kind: PeriodicUpdateIntegrationKind::Hook,
            upsert: upsert_claude_session_start_periodic_hook,
        }),
        Harness::Goose => Ok(PeriodicUpdateTriggerStrategy {
            integration_path: goose_hints_path_for_scope(scope, runtime_paths)?,
            integration_kind: PeriodicUpdateIntegrationKind::Guidance,
            upsert: upsert_goose_periodic_update_hints,
        }),
        Harness::Cursor => Ok(PeriodicUpdateTriggerStrategy {
            integration_path: harness_root.join(CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOKS_FILE_NAME),
            integration_kind: PeriodicUpdateIntegrationKind::Hook,
            upsert: upsert_cursor_periodic_update_hook,
        }),
        Harness::Opencode => Ok(PeriodicUpdateTriggerStrategy {
            integration_path: opencode_periodic_plugin_path_for_scope(
                scope,
                runtime_paths,
                harness_root,
            )?,
            integration_kind: PeriodicUpdateIntegrationKind::Plugin,
            upsert: upsert_opencode_periodic_update_plugin,
        }),
        Harness::Codex => Ok(PeriodicUpdateTriggerStrategy {
            integration_path: codex_config_path_for_scope(scope, runtime_paths, harness_root)?,
            integration_kind: PeriodicUpdateIntegrationKind::Notify,
            upsert: upsert_codex_periodic_update_notify,
        }),
        Harness::Antigravity => Ok(PeriodicUpdateTriggerStrategy {
            integration_path: antigravity_periodic_hints_path_for_scope(
                scope,
                runtime_paths,
                harness_root,
            )?,
            integration_kind: PeriodicUpdateIntegrationKind::Guidance,
            upsert: upsert_antigravity_periodic_update_hints,
        }),
        Harness::Auto => Err(HarnessAdapterError::UnsupportedHarness(
            "auto is not valid for periodic trigger upsert".to_string(),
        )),
    }
}

fn codex_config_path_for_scope(
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
    harness_root: &Path,
) -> AdapterResult<PathBuf> {
    match scope {
        InstallScope::Project => {
            Ok(harness_root.join(CODEMOD_PERIODIC_TRIGGER_CODEX_CONFIG_FILE_NAME))
        }
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| {
                home.join(CODEX_CONFIG_DIR_NAME)
                    .join(CODEMOD_PERIODIC_TRIGGER_CODEX_CONFIG_FILE_NAME)
            })
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
    }
}

fn goose_hints_path_for_scope(
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<PathBuf> {
    match scope {
        InstallScope::Project => Ok(runtime_paths
            .cwd
            .join(CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_FILE_NAME)),
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| home.join(CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_FILE_NAME))
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
    }
}

fn opencode_periodic_plugin_path_for_scope(
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
    harness_root: &Path,
) -> AdapterResult<PathBuf> {
    match scope {
        InstallScope::Project => Ok(harness_root
            .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_DIR_NAME)
            .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_FILE_NAME)),
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| {
                home.join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_USER_CONFIG_RELATIVE_DIR)
                    .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_DIR_NAME)
                    .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_FILE_NAME)
            })
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
    }
}

fn antigravity_periodic_hints_path_for_scope(
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
    harness_root: &Path,
) -> AdapterResult<PathBuf> {
    match scope {
        InstallScope::Project => {
            Ok(harness_root.join(CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_FILE_NAME))
        }
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| {
                home.join(ANTIGRAVITY_USER_ROOT)
                    .join(CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_FILE_NAME)
            })
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
    }
}

fn write_periodic_update_runner_script(
    harness: Harness,
    scope: InstallScope,
    periodic_policy: PeriodicUpdatePolicy,
    runner_path: &Path,
    state_path: &Path,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<bool> {
    let scope_flag = match scope {
        InstallScope::Project => "--project",
        InstallScope::User => "--user",
    };
    let script = build_periodic_update_runner_script(
        harness,
        scope_flag,
        periodic_policy,
        state_path,
        runtime_paths,
    );
    let updated = write_file_if_changed(runner_path, script.as_bytes())?;
    #[cfg(unix)]
    {
        ensure_executable_permissions(runner_path)?;
    }
    Ok(updated)
}

fn build_periodic_update_runner_script(
    harness: Harness,
    scope_flag: &str,
    periodic_policy: PeriodicUpdatePolicy,
    state_path: &Path,
    runtime_paths: &RuntimePaths,
) -> String {
    let quoted_state_path = shell_single_quote(&state_path.to_string_lossy());
    let install_args = vec![
        "agent".to_string(),
        "install".to_string(),
        "--harness".to_string(),
        harness.as_str().to_string(),
        scope_flag.to_string(),
        "--update-policy".to_string(),
        periodic_policy.as_str().to_string(),
        "--require-signed-manifest".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ];
    let invocation_attempts = codemod_cli_invocation_candidates(runtime_paths)
        .iter()
        .map(|invocation| render_shell_invocation_attempt(invocation, &install_args))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        r#"#!/bin/sh
set -eu
STATE_FILE={quoted_state_path}
INTERVAL={default_interval}

NOW="$(date +%s 2>/dev/null || printf '0')"
if [ "$NOW" = "0" ]; then
  exit 0
fi

LAST=0
if [ -f "$STATE_FILE" ]; then
  LAST="$(cat "$STATE_FILE" 2>/dev/null || printf '0')"
  case "$LAST" in
    ''|*[!0-9]*) LAST=0 ;;
  esac
fi

if [ "$LAST" -gt 0 ] && [ $((NOW - LAST)) -lt "$INTERVAL" ]; then
  exit 0
fi

mkdir -p "$(dirname "$STATE_FILE")"
printf '%s\n' "$NOW" > "$STATE_FILE"

OUTPUT="$(
{{
{invocation_attempts}
  exit 127
}} 2>&1 || true
)"
if printf '%s' "$OUTPUT" | grep -Eq '"status"[[:space:]]*:[[:space:]]*"update_available"|"rolled_back"[[:space:]]*:[[:space:]]*true|"failed"[[:space:]]*:[[:space:]]*[1-9]'; then
  printf '%s\n' "$OUTPUT"
fi
"#,
        default_interval = CODEMOD_PERIODIC_UPDATE_DEFAULT_INTERVAL_SECS,
        invocation_attempts = invocation_attempts,
    )
}

fn upsert_claude_session_start_periodic_hook(
    settings_path: &Path,
    runner_path: &Path,
) -> AdapterResult<bool> {
    let existing = if settings_path.exists() {
        fs::read_to_string(settings_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read Claude settings {}: {error}",
                settings_path.display()
            ))
        })?
    } else {
        "{}".to_string()
    };

    let mut settings: Value = serde_json::from_str(&existing).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Claude settings {} are not valid JSON: {error}",
            settings_path.display()
        ))
    })?;

    let Some(root_object) = settings.as_object_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Claude settings {} must contain a top-level JSON object",
            settings_path.display()
        )));
    };

    let hooks_value = root_object
        .entry("hooks".to_string())
        .or_insert_with(|| json!({}));
    let Some(hooks_object) = hooks_value.as_object_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Claude settings {} have non-object `hooks` entry",
            settings_path.display()
        )));
    };

    let session_start_value = hooks_object
        .entry(CODEMOD_PERIODIC_TRIGGER_CLAUDE_SESSION_START_EVENT.to_string())
        .or_insert_with(|| json!([]));
    let Some(session_start_hooks) = session_start_value.as_array_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Claude settings {} have non-array `hooks.{}` entry",
            settings_path.display(),
            CODEMOD_PERIODIC_TRIGGER_CLAUDE_SESSION_START_EVENT
        )));
    };

    let command = runner_path.to_string_lossy().to_string();
    let already_present = session_start_hooks.iter().any(|entry| {
        entry
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("type").and_then(Value::as_str) == Some("command")
                        && hook.get("command").and_then(Value::as_str) == Some(command.as_str())
                })
            })
    });

    if !already_present {
        session_start_hooks.push(json!({
            "hooks": [
                {
                    "type": "command",
                    "command": command
                }
            ]
        }));
    }

    let serialized = serde_json::to_string_pretty(&settings).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to serialize Claude settings {}: {error}",
            settings_path.display()
        ))
    })?;
    write_file_if_changed(settings_path, format!("{serialized}\n").as_bytes())
}

fn upsert_goose_periodic_update_hints(
    hints_path: &Path,
    runner_path: &Path,
) -> AdapterResult<bool> {
    let existing = if hints_path.exists() {
        fs::read_to_string(hints_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read Goose hints file {}: {error}",
                hints_path.display()
            ))
        })?
    } else {
        String::new()
    };

    let block = render_goose_periodic_update_block(runner_path);
    let updated = upsert_managed_block(
        &existing,
        CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_BEGIN,
        CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_END,
        &block,
    );
    if updated == existing {
        return Ok(false);
    }

    write_file_if_changed(hints_path, updated.as_bytes())
}

fn render_goose_periodic_update_block(runner_path: &Path) -> String {
    format!(
        r#"{begin}
Run this command once when a session starts to keep codemod-managed assets fresh (the command has cooldown + no-update quiet behavior):
`{runner_path}`
{end}"#,
        begin = CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_BEGIN,
        runner_path = runner_path.display(),
        end = CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_END,
    )
}

fn upsert_cursor_periodic_update_hook(
    hooks_path: &Path,
    runner_path: &Path,
) -> AdapterResult<bool> {
    let mut hooks = if hooks_path.exists() {
        let content = fs::read_to_string(hooks_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read Cursor hooks {}: {error}",
                hooks_path.display()
            ))
        })?;
        serde_json::from_str::<Value>(&content).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Cursor hooks {} are not valid JSON: {error}",
                hooks_path.display()
            ))
        })?
    } else {
        json!({})
    };

    let Some(root_object) = hooks.as_object_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Cursor hooks {} must contain a top-level JSON object",
            hooks_path.display()
        )));
    };

    root_object
        .entry("version".to_string())
        .or_insert_with(|| json!(1));
    let hooks_value = root_object
        .entry("hooks".to_string())
        .or_insert_with(|| json!({}));
    let Some(hooks_object) = hooks_value.as_object_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Cursor hooks {} have non-object `hooks` entry",
            hooks_path.display()
        )));
    };

    let event_value = hooks_object
        .entry(CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOK_EVENT_NAME.to_string())
        .or_insert_with(|| json!([]));
    let Some(event_hooks) = event_value.as_array_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Cursor hooks {} have non-array `hooks.{}` entry",
            hooks_path.display(),
            CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOK_EVENT_NAME
        )));
    };

    let command = runner_path.to_string_lossy().to_string();
    let already_present = event_hooks.iter().any(|entry| {
        entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|existing| existing == command)
    });

    if !already_present {
        event_hooks.push(json!({ "command": command }));
    }

    let serialized = serde_json::to_string_pretty(&hooks).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to serialize Cursor hooks {}: {error}",
            hooks_path.display()
        ))
    })?;
    write_file_if_changed(hooks_path, format!("{serialized}\n").as_bytes())
}

fn upsert_opencode_periodic_update_plugin(
    plugin_path: &Path,
    runner_path: &Path,
) -> AdapterResult<bool> {
    let runner_literal =
        serde_json::to_string(&runner_path.to_string_lossy()).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to encode OpenCode runner path {}: {error}",
                runner_path.display()
            ))
        })?;
    let content = format!(
        r#"export async function CodemodPeriodicUpdate() {{
  const runnerPath = {runner_literal};
  return {{
    event: async ({{ event, $ }}) => {{
      if (event?.type !== "{event_name}") {{
        return;
      }}

      try {{
        await $`sh ${{runnerPath}}`;
      }} catch {{
        // Best-effort only: keep startup non-blocking.
      }}
    }},
  }};
}}
"#,
        event_name = CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_EVENT_NAME,
        runner_literal = runner_literal,
    );
    write_file_if_changed(plugin_path, content.as_bytes())
}

fn upsert_codex_periodic_update_notify(
    config_path: &Path,
    runner_path: &Path,
) -> AdapterResult<bool> {
    if let Some(parent_dir) = config_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create Codex config directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    let existing = if config_path.exists() {
        fs::read_to_string(config_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read Codex config {}: {error}",
                config_path.display()
            ))
        })?
    } else {
        String::new()
    };

    let mut document = if existing.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing.parse::<DocumentMut>().map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Codex config {} is not valid TOML: {error}",
                config_path.display()
            ))
        })?
    };

    let expected = vec!["sh".to_string(), runner_path.to_string_lossy().to_string()];
    if let Some(existing_notify) = read_notify_command(&document)? {
        if existing_notify != expected {
            // Preserve existing user notify integration if configured.
            return Ok(false);
        }
    }

    let mut notify = Array::new();
    for arg in expected {
        notify.push(arg);
    }
    document["notify"] = value(notify);

    write_file_if_changed(config_path, format!("{}\n", document).as_bytes())
}

fn read_notify_command(document: &DocumentMut) -> AdapterResult<Option<Vec<String>>> {
    let Some(item) = document.get("notify") else {
        return Ok(None);
    };
    let Some(array) = item.as_array() else {
        return Err(HarnessAdapterError::InstallFailed(
            "Codex config has non-array `notify` entry".to_string(),
        ));
    };

    let mut values = Vec::with_capacity(array.len());
    for value in array.iter() {
        let Some(raw) = value.as_str() else {
            return Err(HarnessAdapterError::InstallFailed(
                "Codex config `notify` array must contain only strings".to_string(),
            ));
        };
        values.push(raw.to_string());
    }

    Ok(Some(values))
}

fn upsert_antigravity_periodic_update_hints(
    hints_path: &Path,
    runner_path: &Path,
) -> AdapterResult<bool> {
    let existing = if hints_path.exists() {
        fs::read_to_string(hints_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read Antigravity periodic guidance {}: {error}",
                hints_path.display()
            ))
        })?
    } else {
        String::new()
    };

    let block = format!(
        "{begin}\nRun this command when a session starts to keep codemod-managed assets fresh (cooldown + quiet no-update behavior):\n`{runner}`\n{end}\n",
        begin = CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_BEGIN,
        runner = runner_path.display(),
        end = CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_END,
    );
    let updated = upsert_managed_block(
        &existing,
        CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_BEGIN,
        CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_END,
        &block,
    );
    if updated == existing {
        return Ok(false);
    }

    write_file_if_changed(hints_path, updated.as_bytes())
}

fn codemod_cli_invocation_candidates(runtime_paths: &RuntimePaths) -> Vec<CodemodCliInvocation> {
    let mut invocations = Vec::new();
    if let Some(current_executable) = runtime_paths.current_executable.as_ref() {
        invocations.push(CodemodCliInvocation {
            command: current_executable.to_string_lossy().to_string(),
            args_prefix: Vec::new(),
        });
    }
    invocations.push(CodemodCliInvocation {
        command: CODEMOD_CLI_COMMAND.to_string(),
        args_prefix: Vec::new(),
    });
    invocations.push(codemod_cli_npx_invocation());
    invocations
}

fn codemod_cli_npx_invocation() -> CodemodCliInvocation {
    CodemodCliInvocation {
        command: NPX_COMMAND.to_string(),
        args_prefix: vec![MCP_SERVER_ARG_PACKAGE.to_string()],
    }
}

fn codemod_cli_invocation_for_mcp(runtime_paths: &RuntimePaths) -> CodemodCliInvocation {
    codemod_cli_invocation_candidates(runtime_paths)
        .into_iter()
        .find(codemod_cli_invocation_available)
        .unwrap_or_else(codemod_cli_npx_invocation)
}

fn codemod_cli_invocation_available(invocation: &CodemodCliInvocation) -> bool {
    if is_explicit_command_path(&invocation.command) {
        return Path::new(&invocation.command).exists();
    }

    command_exists_in_path(&invocation.command)
}

fn command_exists_in_path(command: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(command);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn is_explicit_command_path(command: &str) -> bool {
    command.contains('/') || command.contains('\\')
}

fn render_shell_invocation_attempt(invocation: &CodemodCliInvocation, args: &[String]) -> String {
    let command_line = shell_command_line(&invocation.command, &invocation.with_args(args));
    let quoted_command = shell_single_quote(&invocation.command);
    if is_explicit_command_path(&invocation.command) {
        format!("if [ -x {quoted_command} ]; then\n  {command_line}\n  exit $?\nfi")
    } else {
        format!(
            "if command -v {quoted_command} >/dev/null 2>&1; then\n  {command_line}\n  exit $?\nfi"
        )
    }
}

fn shell_command_line(command: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(shell_single_quote(command));
    parts.extend(args.iter().map(|arg| shell_single_quote(arg)));
    parts.join(" ")
}

fn shell_single_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn write_file_if_changed(path: &Path, bytes: &[u8]) -> AdapterResult<bool> {
    if let Some(parent_dir) = path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    if path.exists() {
        let existing = fs::read(path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read {}: {error}",
                path.display()
            ))
        })?;
        if existing == bytes {
            return Ok(false);
        }
    }

    fs::write(path, bytes).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!("Failed to write {}: {error}", path.display()))
    })?;
    Ok(true)
}

#[cfg(unix)]
fn ensure_executable_permissions(path: &Path) -> AdapterResult<()> {
    let metadata = fs::metadata(path).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to read file metadata {}: {error}",
            path.display()
        ))
    })?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to set executable permissions on {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

pub(crate) fn upsert_skill_discovery_guides_with_runtime(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<PathBuf>> {
    let skill_root_hint = skill_root_hint_for_scope(harness, scope)?;
    let discovery_block = render_skill_discovery_block(harness, &skill_root_hint);
    let discovery_paths = discovery_guide_paths_with_runtime(harness, scope, runtime_paths)?;
    let mut updated_files = Vec::new();

    for file_path in discovery_paths {
        if upsert_discovery_block_in_file(&file_path, &discovery_block)? {
            updated_files.push(file_path);
        }
    }

    Ok(updated_files)
}

fn discovery_guide_paths_with_runtime(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<PathBuf>> {
    let docs_root = match scope {
        InstallScope::Project => runtime_paths.cwd.clone(),
        InstallScope::User => runtime_paths.home_dir.clone().ok_or_else(|| {
            HarnessAdapterError::InstallFailed(
                "Could not determine home directory for --user install".to_string(),
            )
        })?,
    };

    let file_names = match harness {
        Harness::Claude => vec![CLAUDE_GUIDE_FILE_NAME],
        Harness::Goose | Harness::Opencode | Harness::Cursor => {
            vec![AGENTS_GUIDE_FILE_NAME, CLAUDE_GUIDE_FILE_NAME]
        }
        Harness::Codex | Harness::Antigravity => vec![AGENTS_GUIDE_FILE_NAME],
        Harness::Auto => {
            return Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()));
        }
    };

    Ok(file_names
        .into_iter()
        .map(|file_name| docs_root.join(file_name))
        .collect())
}

fn skill_root_hint_for_scope(harness: Harness, scope: InstallScope) -> AdapterResult<String> {
    Ok(match scope {
        InstallScope::Project => match harness {
            Harness::Claude => ".claude/skills".to_string(),
            Harness::Goose => ".goose/skills".to_string(),
            Harness::Opencode => ".opencode/skills".to_string(),
            Harness::Cursor => ".cursor/skills".to_string(),
            Harness::Codex => ".agents/skills".to_string(),
            Harness::Antigravity => ".agents/skills".to_string(),
            Harness::Auto => {
                return Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()));
            }
        },
        InstallScope::User => match harness {
            Harness::Claude => "~/.claude/skills".to_string(),
            Harness::Goose => "~/.goose/skills".to_string(),
            Harness::Opencode => "~/.opencode/skills".to_string(),
            Harness::Cursor => "~/.cursor/skills".to_string(),
            Harness::Codex => "~/.agents/skills".to_string(),
            Harness::Antigravity => "~/.gemini/antigravity/skills".to_string(),
            Harness::Auto => {
                return Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()));
            }
        },
    })
}

fn render_skill_discovery_block(harness: Harness, skill_root_hint: &str) -> String {
    format!(
        "{SKILL_DISCOVERY_SECTION_BEGIN}
## Codemod Skill Discovery
This section is managed by `codemod` CLI.

- Core skill: `{skill_root_hint}/{MCS_SKILL_DIR_NAME}/SKILL.md`
- Package skills: `{skill_root_hint}/<package-skill>/SKILL.md`
- List installed Codemod skills: `npx codemod agent list --harness {} --format json`

{SKILL_DISCOVERY_SECTION_END}",
        harness.as_str()
    )
}

fn upsert_discovery_block_in_file(file_path: &Path, block: &str) -> AdapterResult<bool> {
    let existing = if file_path.exists() {
        fs::read_to_string(file_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read {}: {error}",
                file_path.display()
            ))
        })?
    } else {
        String::new()
    };

    let updated = upsert_managed_discovery_block(&existing, block);
    if updated == existing {
        return Ok(false);
    }

    if let Some(parent_dir) = file_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    fs::write(file_path, updated).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to write {}: {error}",
            file_path.display()
        ))
    })?;

    Ok(true)
}

fn upsert_managed_discovery_block(existing: &str, block: &str) -> String {
    upsert_managed_block(
        existing,
        SKILL_DISCOVERY_SECTION_BEGIN,
        SKILL_DISCOVERY_SECTION_END,
        block,
    )
}

fn upsert_managed_block(
    existing: &str,
    begin_marker: &str,
    end_marker: &str,
    block: &str,
) -> String {
    if let (Some(begin_index), Some(end_start)) =
        (existing.find(begin_marker), existing.find(end_marker))
    {
        if end_start >= begin_index {
            let end_index = end_start + end_marker.len();
            let mut updated = String::new();
            updated.push_str(&existing[..begin_index]);
            updated.push_str(block);
            updated.push_str(&existing[end_index..]);
            return updated;
        }
    }

    if existing.trim().is_empty() {
        return format!("{block}\n");
    }

    let mut updated = existing.trim_end_matches('\n').to_string();
    updated.push_str("\n\n");
    updated.push_str(block);
    updated.push('\n');
    updated
}

pub fn persist_managed_install_state(
    harness: Harness,
    scope: InstallScope,
    components: &[ManagedComponentSnapshot],
) -> AdapterResult<ManagedStateWriteResult> {
    let runtime_paths = RuntimePaths::current()?;
    persist_managed_install_state_with_runtime(harness, scope, components, &runtime_paths)
}

pub fn read_managed_install_state(
    harness: Harness,
    scope: InstallScope,
) -> AdapterResult<Option<ManagedStateReadResult>> {
    let runtime_paths = RuntimePaths::current()?;
    read_managed_install_state_with_runtime(harness, scope, &runtime_paths)
}

fn persist_managed_install_state_with_runtime(
    harness: Harness,
    scope: InstallScope,
    components: &[ManagedComponentSnapshot],
    runtime_paths: &RuntimePaths,
) -> AdapterResult<ManagedStateWriteResult> {
    let state_path = managed_state_path_for_harness(harness, scope, runtime_paths)?;
    let lock_guard = acquire_managed_state_lock(&state_path)?;
    let expected_state = build_managed_install_state(harness, scope, components);
    let result = persist_managed_install_state_locked(&state_path, &expected_state);
    lock_guard.release();
    result
}

fn read_managed_install_state_with_runtime(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Option<ManagedStateReadResult>> {
    let state_path = managed_state_path_for_harness(harness, scope, runtime_paths)?;
    let Some(state) = read_managed_install_state_if_present(&state_path)? else {
        return Ok(None);
    };
    let components = managed_components_from_state(&state)?;
    Ok(Some(ManagedStateReadResult {
        path: state_path,
        components,
    }))
}

fn persist_managed_install_state_locked(
    state_path: &Path,
    expected_state: &ManagedInstallState,
) -> AdapterResult<ManagedStateWriteResult> {
    let existing_state = read_managed_install_state_if_present(state_path)?;

    if existing_state.as_ref() == Some(expected_state) {
        return Ok(ManagedStateWriteResult {
            path: state_path.to_path_buf(),
            status: ManagedStateWriteStatus::Unchanged,
        });
    }

    if let Some(parent_dir) = state_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create managed state directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    let serialized = serde_json::to_string_pretty(expected_state).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to serialize managed install state {}: {error}",
            state_path.display()
        ))
    })?;

    write_atomic(state_path, format!("{serialized}\n").as_bytes())?;

    let status = if existing_state.is_some() {
        ManagedStateWriteStatus::Updated
    } else {
        ManagedStateWriteStatus::Created
    };

    Ok(ManagedStateWriteResult {
        path: state_path.to_path_buf(),
        status,
    })
}

fn managed_state_path_for_harness(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<PathBuf> {
    match scope {
        InstallScope::Project => Ok(match harness {
            Harness::Claude => runtime_paths.cwd.join(".claude"),
            Harness::Goose => runtime_paths.cwd.join(".goose"),
            Harness::Opencode => runtime_paths.cwd.join(".opencode"),
            Harness::Cursor => runtime_paths.cwd.join(".cursor"),
            Harness::Codex => runtime_paths.cwd.join(CODEX_CONFIG_DIR_NAME),
            Harness::Antigravity => runtime_paths.cwd.join(ANTIGRAVITY_WORKSPACE_ROOT),
            Harness::Auto => {
                return Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()));
            }
        }
        .join(CODEMOD_MANAGED_STATE_RELATIVE_PATH)),
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| match harness {
                Harness::Claude => home.join(".claude"),
                Harness::Goose => home.join(".goose"),
                Harness::Opencode => home.join(".opencode"),
                Harness::Cursor => home.join(".cursor"),
                Harness::Codex => home.join(CODEX_CONFIG_DIR_NAME),
                Harness::Antigravity => home.join(ANTIGRAVITY_USER_ROOT),
                Harness::Auto => PathBuf::new(),
            })
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            })
            .and_then(|root| {
                if root.as_os_str().is_empty() {
                    Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()))
                } else {
                    Ok(root.join(CODEMOD_MANAGED_STATE_RELATIVE_PATH))
                }
            }),
    }
}

fn acquire_managed_state_lock(state_path: &Path) -> AdapterResult<ManagedStateLockGuard> {
    acquire_managed_state_lock_with_policy(state_path, ManagedStateLockPolicy::default_policy())
}

fn acquire_managed_state_lock_with_policy(
    state_path: &Path,
    policy: ManagedStateLockPolicy,
) -> AdapterResult<ManagedStateLockGuard> {
    let lock_path = managed_state_lock_path(state_path)?;
    if let Some(parent_dir) = lock_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create managed state lock directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    let started_at = Instant::now();
    loop {
        match try_create_managed_state_lock(&lock_path) {
            Ok(()) => {
                return Ok(ManagedStateLockGuard {
                    path: lock_path,
                    released: false,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if maybe_recover_stale_managed_state_lock(&lock_path, policy.stale_after)? {
                    continue;
                }
                if started_at.elapsed() >= policy.timeout {
                    return Err(HarnessAdapterError::InstallFailed(format!(
                        "Managed state lock acquisition timed out after {}ms (retry {}ms) for {}",
                        policy.timeout.as_millis(),
                        policy.retry_interval.as_millis(),
                        state_path.display()
                    )));
                }
                sleep(policy.retry_interval);
            }
            Err(error) => {
                return Err(HarnessAdapterError::InstallFailed(format!(
                    "Failed to acquire managed state lock {}: {error}",
                    lock_path.display()
                )));
            }
        }
    }
}

fn managed_state_lock_path(state_path: &Path) -> AdapterResult<PathBuf> {
    let parent_dir = state_path.parent().ok_or_else(|| {
        HarnessAdapterError::InstallFailed(format!(
            "Managed state path {} is missing a parent directory",
            state_path.display()
        ))
    })?;
    let file_name = state_path.file_name().ok_or_else(|| {
        HarnessAdapterError::InstallFailed(format!(
            "Managed state path {} is missing a file name",
            state_path.display()
        ))
    })?;
    let mut lock_name: OsString = file_name.to_os_string();
    lock_name.push(".lock");
    Ok(parent_dir.join(lock_name))
}

fn try_create_managed_state_lock(lock_path: &Path) -> std::io::Result<()> {
    let mut lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)?;
    let metadata = ManagedStateLockMetadata {
        pid: std::process::id(),
        acquired_at_epoch_secs: now_epoch_secs(),
    };
    let serialized = serde_json::to_vec(&metadata).map_err(std::io::Error::other)?;
    lock_file.write_all(&serialized)?;
    lock_file.flush()?;
    Ok(())
}

fn maybe_recover_stale_managed_state_lock(
    lock_path: &Path,
    stale_after: Duration,
) -> AdapterResult<bool> {
    let is_stale = is_managed_state_lock_stale(lock_path, stale_after)?;
    if !is_stale {
        return Ok(false);
    }

    fs::remove_file(lock_path).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to remove stale managed state lock {}: {error}",
            lock_path.display()
        ))
    })?;
    Ok(true)
}

fn is_managed_state_lock_stale(lock_path: &Path, stale_after: Duration) -> AdapterResult<bool> {
    let payload = fs::read(lock_path).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to read managed state lock {}: {error}",
            lock_path.display()
        ))
    })?;

    match serde_json::from_slice::<ManagedStateLockMetadata>(&payload) {
        Ok(metadata) => {
            let age_secs = now_epoch_secs().saturating_sub(metadata.acquired_at_epoch_secs);
            Ok(age_secs > stale_after.as_secs())
        }
        Err(_) => {
            let metadata = fs::metadata(lock_path).map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "Failed to inspect managed state lock {}: {error}",
                    lock_path.display()
                ))
            })?;
            let modified_at = metadata.modified().map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "Failed to read managed state lock timestamp {}: {error}",
                    lock_path.display()
                ))
            })?;
            let age_secs = age_from_system_time_secs(modified_at);
            Ok(age_secs > stale_after.as_secs())
        }
    }
}

fn release_managed_state_lock(lock_path: &Path) -> std::io::Result<()> {
    match fs::remove_file(lock_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> AdapterResult<()> {
    let parent_dir = path.parent().ok_or_else(|| {
        HarnessAdapterError::InstallFailed(format!(
            "Managed state path {} is missing a parent directory",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent_dir).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to create directory {}: {error}",
            parent_dir.display()
        ))
    })?;

    let temp_path = atomic_temp_path(path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to open temp file {} for atomic write: {error}",
                temp_path.display()
            ))
        })?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temp_path);
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Failed to write temp file {} for atomic write: {error}",
            temp_path.display()
        )));
    }
    drop(file);

    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        HarnessAdapterError::InstallFailed(format!(
            "Failed to atomically replace managed state file {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

fn atomic_temp_path(path: &Path) -> AdapterResult<PathBuf> {
    let parent_dir = path.parent().ok_or_else(|| {
        HarnessAdapterError::InstallFailed(format!(
            "Managed state path {} is missing a parent directory",
            path.display()
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        HarnessAdapterError::InstallFailed(format!(
            "Managed state path {} is missing a file name",
            path.display()
        ))
    })?;
    let mut temp_name: OsString = file_name.to_os_string();
    temp_name.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        now_epoch_millis()
    ));
    Ok(parent_dir.join(temp_name))
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn age_from_system_time_secs(system_time: SystemTime) -> u64 {
    SystemTime::now()
        .duration_since(system_time)
        .unwrap_or_default()
        .as_secs()
}

fn read_managed_install_state_if_present(
    path: &Path,
) -> AdapterResult<Option<ManagedInstallState>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to read managed install state {}: {error}",
            path.display()
        ))
    })?;

    match serde_json::from_str::<ManagedInstallState>(&content) {
        Ok(state) => Ok(Some(state)),
        Err(_) => Ok(None),
    }
}

fn build_managed_install_state(
    harness: Harness,
    scope: InstallScope,
    components: &[ManagedComponentSnapshot],
) -> ManagedInstallState {
    let mut state_components = components
        .iter()
        .map(managed_state_component_from_snapshot)
        .collect::<Vec<_>>();
    state_components.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.path.cmp(&right.path))
    });

    ManagedInstallState {
        schema_version: CODEMOD_MANAGED_STATE_SCHEMA_VERSION.to_string(),
        harness: harness.as_str().to_string(),
        scope: scope.as_str().to_string(),
        components: state_components,
    }
}

fn managed_state_component_from_snapshot(
    snapshot: &ManagedComponentSnapshot,
) -> ManagedInstallStateComponent {
    ManagedInstallStateComponent {
        id: snapshot.id.clone(),
        kind: snapshot.kind.as_str().to_string(),
        path: snapshot.path.to_string_lossy().to_string(),
        version: snapshot.version.clone(),
        fingerprint: content_fingerprint(&snapshot.path),
    }
}

fn managed_components_from_state(
    state: &ManagedInstallState,
) -> AdapterResult<Vec<ManagedComponentSnapshot>> {
    state
        .components
        .iter()
        .map(managed_snapshot_from_state_component)
        .collect()
}

fn managed_snapshot_from_state_component(
    component: &ManagedInstallStateComponent,
) -> AdapterResult<ManagedComponentSnapshot> {
    let kind = managed_component_kind_from_state(&component.kind)?;
    Ok(ManagedComponentSnapshot {
        id: component.id.clone(),
        kind,
        path: PathBuf::from(&component.path),
        version: component.version.clone(),
    })
}

fn managed_component_kind_from_state(kind: &str) -> AdapterResult<ManagedComponentKind> {
    match kind {
        "skill" => Ok(ManagedComponentKind::Skill),
        "mcp_config" => Ok(ManagedComponentKind::McpConfig),
        "discovery_guide" => Ok(ManagedComponentKind::DiscoveryGuide),
        unknown => Err(HarnessAdapterError::InstallFailed(format!(
            "Managed install state contains unsupported component kind `{unknown}`"
        ))),
    }
}

fn content_fingerprint(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(format!("{:x}", hasher.finalize()))
}

fn install_mcs_skill_bundle_with_runtime(
    harness: Harness,
    request: &InstallRequest,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<InstalledSkill>> {
    validate_embedded_mcs_bundle()?;

    let skill_root =
        skills_root_for_harness(harness, request.scope, runtime_paths)?.join(MCS_SKILL_DIR_NAME);
    let skill_md_path = skill_root.join("SKILL.md");

    write_skill_file(&skill_md_path, MCS_SKILL_MD, request.force)?;
    for (relative_path, content) in MCS_REFERENCE_FILES {
        write_skill_file(&skill_root.join(relative_path), content, request.force)?;
    }

    let mut installed = vec![InstalledSkill {
        name: MCS_SKILL_COMPONENT_ID.to_string(),
        path: skill_md_path,
        version: Some(MCS_SKILL_VERSION.to_string()),
        scope: Some(request.scope),
    }];

    if let Some(mcp_config_path) = maybe_install_mcp_server_config(harness, request, runtime_paths)?
    {
        installed.push(InstalledSkill {
            name: "codemod-mcp".to_string(),
            path: mcp_config_path,
            version: None,
            scope: Some(request.scope),
        });
    }

    Ok(installed)
}

fn mcs_install_requires_force_with_runtime(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<bool> {
    validate_embedded_mcs_bundle()?;

    let skill_root =
        skills_root_for_harness(harness, scope, runtime_paths)?.join(MCS_SKILL_DIR_NAME);
    if managed_text_file_requires_force(&skill_root.join(SKILL_FILE_NAME), MCS_SKILL_MD)? {
        return Ok(true);
    }

    for (relative_path, content) in MCS_REFERENCE_FILES {
        if managed_text_file_requires_force(&skill_root.join(relative_path), content)? {
            return Ok(true);
        }
    }

    Ok(false)
}

pub(crate) fn install_package_skill_bundle_with_runtime(
    harness: Harness,
    package: &SkillPackageInstallSpec,
    request: &InstallRequest,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<InstalledSkill>> {
    validate_skill_package_install_spec(package)?;
    let skill_dir_name = skill_directory_name_for_package_id(&package.id);

    let skill_root =
        skills_root_for_harness(harness, request.scope, runtime_paths)?.join(skill_dir_name);
    let skill_md_path = skill_root.join("SKILL.md");

    write_package_skill_directory(&package.source_dir, &skill_root, request.force)?;

    Ok(vec![InstalledSkill {
        name: package.id.clone(),
        path: skill_md_path,
        version: Some(package.version.clone()),
        scope: Some(request.scope),
    }])
}

pub(crate) fn package_skill_install_requires_force_with_runtime(
    harness: Harness,
    scope: InstallScope,
    package: &SkillPackageInstallSpec,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<bool> {
    validate_skill_package_install_spec(package)?;

    let skill_dir_name = skill_directory_name_for_package_id(&package.id);
    let skill_root = skills_root_for_harness(harness, scope, runtime_paths)?.join(skill_dir_name);
    if !skill_root.exists() {
        return Ok(false);
    }

    Ok(!package_skill_directories_match(
        &package.source_dir,
        &skill_root,
    )?)
}

fn skill_directory_name_for_package_id(package_id: &str) -> String {
    package_id
        .trim_start_matches('@')
        .replace(['/', '\\'], "__")
}

fn validate_skill_package_install_spec(package: &SkillPackageInstallSpec) -> AdapterResult<()> {
    let id = package.id.trim();
    if id.is_empty() {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            "Skill package id cannot be empty".to_string(),
        ));
    }

    if id.chars().any(char::is_whitespace) {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            "Skill package id cannot contain whitespace".to_string(),
        ));
    }

    if package.version.trim().is_empty() {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            "Skill package version cannot be empty".to_string(),
        ));
    }

    if package.description.trim().is_empty() {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            "Skill package description cannot be empty".to_string(),
        ));
    }

    if !package.source_dir.is_dir() {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Authored skill directory is missing: {}",
            package.source_dir.display()
        )));
    }

    let skill_md_path = package.source_dir.join("SKILL.md");
    if !skill_md_path.is_file() {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Authored skill file is missing: {}",
            skill_md_path.display()
        )));
    }

    let skill_md_content = fs::read_to_string(&skill_md_path).map_err(|error| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Failed to read authored skill file {}: {error}",
            skill_md_path.display()
        ))
    })?;
    validate_skill_content_for_install(&skill_md_content)?;

    Ok(())
}

fn validate_skill_content_for_install(content: &str) -> AdapterResult<()> {
    let Some(frontmatter) = extract_frontmatter(content) else {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            "Authored package SKILL.md is missing YAML frontmatter".to_string(),
        ));
    };

    if let Some(required_key) = missing_required_frontmatter_key(frontmatter) {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Authored package SKILL.md is missing required frontmatter key: {required_key}"
        )));
    }

    serde_yaml::from_str::<serde_yaml::Value>(frontmatter).map_err(|error| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Authored package SKILL.md frontmatter is invalid YAML: {error}"
        ))
    })?;

    if !content.contains(SKILL_PACKAGE_COMPATIBILITY_MARKER) {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            "Authored package SKILL.md is missing compatibility marker".to_string(),
        ));
    }

    if !content.contains(CODEMOD_VERSION_MARKER_PREFIX) {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            "Authored package SKILL.md is missing skill version marker".to_string(),
        ));
    }

    Ok(())
}

fn install_mcp_server_config(
    harness: Harness,
    request: &InstallRequest,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<PathBuf> {
    let mcp_config_path = mcp_config_path_for_harness(harness, request.scope, runtime_paths)?;
    upsert_codemod_mcp_server(harness, &mcp_config_path, request.force, runtime_paths)?;
    Ok(mcp_config_path)
}

fn maybe_install_mcp_server_config(
    harness: Harness,
    request: &InstallRequest,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Option<PathBuf>> {
    if !harness_supports_mcp(harness) {
        return Ok(None);
    }
    install_mcp_server_config(harness, request, runtime_paths).map(Some)
}

fn harness_supports_mcp(harness: Harness) -> bool {
    !matches!(harness, Harness::Antigravity)
}

fn mcp_config_path_for_harness(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<PathBuf> {
    match (harness, scope) {
        (Harness::Claude, InstallScope::Project) => Ok(runtime_paths.cwd.join(".mcp.json")),
        (Harness::Claude, InstallScope::User) => runtime_paths
            .home_dir
            .as_ref()
            .map(|home_dir| home_dir.join(".mcp.json"))
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
        (Harness::Goose, InstallScope::Project) => Ok(runtime_paths.cwd.join(".goose/mcp.json")),
        (Harness::Goose, InstallScope::User) => runtime_paths
            .home_dir
            .as_ref()
            .map(|home_dir| home_dir.join(".goose/mcp.json"))
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
        (Harness::Opencode, InstallScope::Project) => {
            Ok(runtime_paths.cwd.join(".opencode/mcp.json"))
        }
        (Harness::Opencode, InstallScope::User) => runtime_paths
            .home_dir
            .as_ref()
            .map(|home_dir| home_dir.join(".opencode/mcp.json"))
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
        (Harness::Cursor, InstallScope::Project) => Ok(runtime_paths.cwd.join(".cursor/mcp.json")),
        (Harness::Cursor, InstallScope::User) => runtime_paths
            .home_dir
            .as_ref()
            .map(|home_dir| home_dir.join(".cursor/mcp.json"))
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
        (Harness::Codex, InstallScope::Project) => Ok(runtime_paths.cwd.join(".codex/config.toml")),
        (Harness::Codex, InstallScope::User) => runtime_paths
            .home_dir
            .as_ref()
            .map(|home_dir| home_dir.join(".codex/config.toml"))
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
        (Harness::Antigravity, _) => Err(HarnessAdapterError::UnsupportedHarness(
            "antigravity does not support Codemod MCP configuration yet".to_string(),
        )),
        (Harness::Auto, _) => Err(HarnessAdapterError::UnsupportedHarness("auto".to_string())),
    }
}

fn expected_codemod_mcp_server_entry(runtime_paths: &RuntimePaths) -> Value {
    let invocation = codemod_cli_invocation_for_mcp(runtime_paths);
    let mut args = invocation.args_prefix;
    args.push(MCP_SERVER_ARG_COMMAND.to_string());
    json!({
        "command": invocation.command,
        "args": args
    })
}

fn upsert_codemod_mcp_server(
    harness: Harness,
    config_path: &Path,
    force: bool,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    match harness {
        Harness::Codex => upsert_codemod_mcp_server_toml(config_path, force, runtime_paths),
        Harness::Claude | Harness::Goose | Harness::Opencode | Harness::Cursor => {
            upsert_codemod_mcp_server_json(config_path, force, runtime_paths)
        }
        Harness::Antigravity => Err(HarnessAdapterError::UnsupportedHarness(
            "antigravity does not support Codemod MCP configuration yet".to_string(),
        )),
        Harness::Auto => Err(HarnessAdapterError::UnsupportedHarness("auto".to_string())),
    }
}

fn upsert_codemod_mcp_server_json(
    config_path: &Path,
    force: bool,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    if let Some(parent_dir) = config_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    let expected_entry = expected_codemod_mcp_server_entry(runtime_paths);
    let mut config_root = if config_path.exists() {
        let existing_content = fs::read_to_string(config_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read MCP config {}: {error}",
                config_path.display()
            ))
        })?;

        serde_json::from_str::<Value>(&existing_content).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "MCP config {} is not valid JSON: {error}",
                config_path.display()
            ))
        })?
    } else {
        json!({})
    };

    let Some(root_object) = config_root.as_object_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "MCP config {} must contain a top-level JSON object",
            config_path.display()
        )));
    };

    let mcp_servers_value = root_object
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));

    let Some(mcp_servers) = mcp_servers_value.as_object_mut() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "MCP config {} has non-object `mcpServers`; update manually or re-run with --force after fixing JSON",
            config_path.display()
        )));
    };

    if let Some(existing_entry) = mcp_servers.get(MCP_SERVER_NAME) {
        if existing_entry == &expected_entry {
            return Ok(());
        }

        if !force {
            return Err(HarnessAdapterError::InstallFailed(format!(
                "MCP server `{MCP_SERVER_NAME}` already exists in {} with different settings. Re-run with --force to overwrite.",
                config_path.display()
            )));
        }
    }

    mcp_servers.insert(MCP_SERVER_NAME.to_string(), expected_entry);

    let serialized = serde_json::to_string_pretty(&config_root).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to serialize MCP config {}: {error}",
            config_path.display()
        ))
    })?;

    fs::write(config_path, format!("{serialized}\n")).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to write MCP config {}: {error}",
            config_path.display()
        ))
    })?;

    Ok(())
}

fn upsert_codemod_mcp_server_toml(
    config_path: &Path,
    force: bool,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    if let Some(parent_dir) = config_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    let existing_content = if config_path.exists() {
        fs::read_to_string(config_path).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read MCP config {}: {error}",
                config_path.display()
            ))
        })?
    } else {
        String::new()
    };

    let mut document = if existing_content.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing_content.parse::<DocumentMut>().map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "MCP config {} is not valid TOML: {error}",
                config_path.display()
            ))
        })?
    };

    let invocation = codemod_cli_invocation_for_mcp(runtime_paths);
    let mut expected_args = invocation.args_prefix;
    expected_args.push(MCP_SERVER_ARG_COMMAND.to_string());

    if let Some(existing_server) = read_codex_mcp_server(&document)? {
        if !force
            && (existing_server.command != invocation.command
                || existing_server.args != expected_args)
        {
            return Err(HarnessAdapterError::InstallFailed(format!(
                "MCP config {} already contains a conflicting mcp_servers.{} entry. Re-run with --force to overwrite.",
                config_path.display(),
                MCP_SERVER_NAME,
            )));
        }
    }

    if !document.as_table().contains_key("mcp_servers") {
        document["mcp_servers"] = Item::Table(Table::new());
    }
    if !document["mcp_servers"].is_table() {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "MCP config {} has non-table `mcp_servers`; update manually or re-run with --force after fixing TOML",
            config_path.display()
        )));
    }

    document["mcp_servers"][MCP_SERVER_NAME]["command"] = value(invocation.command);
    let mut args = Array::new();
    for arg in expected_args {
        args.push(arg);
    }
    document["mcp_servers"][MCP_SERVER_NAME]["args"] = value(args);

    write_file_if_changed(config_path, format!("{}\n", document).as_bytes())?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexMcpServer {
    command: String,
    args: Vec<String>,
}

fn read_codex_mcp_server(document: &DocumentMut) -> AdapterResult<Option<CodexMcpServer>> {
    let Some(mcp_servers) = document.get("mcp_servers") else {
        return Ok(None);
    };
    let Some(mcp_servers_table) = mcp_servers.as_table_like() else {
        return Err(HarnessAdapterError::InstallFailed(
            "Codex config has non-table `mcp_servers` entry".to_string(),
        ));
    };

    let Some(server) = mcp_servers_table.get(MCP_SERVER_NAME) else {
        return Ok(None);
    };
    let Some(server_table) = server.as_table_like() else {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Codex config mcp_servers.{} must be a table",
            MCP_SERVER_NAME
        )));
    };

    let command = server_table
        .get("command")
        .and_then(Item::as_str)
        .ok_or_else(|| {
            HarnessAdapterError::InstallFailed(format!(
                "Codex config mcp_servers.{} has missing or non-string `command`",
                MCP_SERVER_NAME
            ))
        })?
        .to_string();

    let args_item = server_table.get("args").ok_or_else(|| {
        HarnessAdapterError::InstallFailed(format!(
            "Codex config mcp_servers.{} has missing `args` array",
            MCP_SERVER_NAME
        ))
    })?;
    let args_array = args_item.as_array().ok_or_else(|| {
        HarnessAdapterError::InstallFailed(format!(
            "Codex config mcp_servers.{} has non-array `args`",
            MCP_SERVER_NAME
        ))
    })?;
    let mut args = Vec::with_capacity(args_array.len());
    for value in args_array.iter() {
        let Some(arg) = value.as_str() else {
            return Err(HarnessAdapterError::InstallFailed(format!(
                "Codex config mcp_servers.{} args must contain only strings",
                MCP_SERVER_NAME
            )));
        };
        args.push(arg.to_string());
    }

    Ok(Some(CodexMcpServer { command, args }))
}

fn list_skills_with_runtime(
    harness: Harness,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<InstalledSkill>> {
    let mut installed = Vec::new();

    for (scope, skill_root) in skill_roots_for_listing(harness, runtime_paths)? {
        if !skill_root.exists() {
            continue;
        }

        let root_entries = fs::read_dir(&skill_root).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to read skills directory {}: {error}",
                skill_root.display()
            ))
        })?;

        for entry in root_entries {
            let entry = entry.map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "Failed to read entry in {}: {error}",
                    skill_root.display()
                ))
            })?;

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let Some(skill_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };

            let skill_md_path = path.join("SKILL.md");
            if !skill_md_path.exists() {
                continue;
            }

            // Scope this command family to codemod-managed skills only.
            if !is_codemod_managed_skill(skill_name, &skill_md_path) {
                continue;
            }

            let version = read_skill_version_marker(&skill_md_path).ok();
            installed.push(InstalledSkill {
                name: skill_name.to_string(),
                path: skill_md_path,
                version,
                scope: Some(scope),
            });
        }
    }

    installed.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(installed)
}

fn verify_skills_with_runtime(
    harness: Harness,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<VerificationCheck>> {
    let installed = list_skills_with_runtime(harness, runtime_paths)?;
    let checks = installed
        .iter()
        .map(verify_installed_skill)
        .collect::<Vec<_>>();

    Ok(checks)
}

fn verify_installed_skill(skill: &InstalledSkill) -> VerificationCheck {
    let scope = skill.scope;
    let content = match fs::read_to_string(&skill.path) {
        Ok(content) => content,
        Err(error) => {
            return VerificationCheck {
                skill: skill.name.clone(),
                scope,
                status: VerificationStatus::Fail,
                reason: Some(format!("failed to read SKILL.md: {error}")),
            };
        }
    };

    let frontmatter = match extract_frontmatter(&content) {
        Some(frontmatter) => frontmatter,
        None => {
            return VerificationCheck {
                skill: skill.name.clone(),
                scope,
                status: VerificationStatus::Fail,
                reason: Some("missing YAML frontmatter".to_string()),
            };
        }
    };

    if let Some(required_key) = missing_required_frontmatter_key(frontmatter) {
        return VerificationCheck {
            skill: skill.name.clone(),
            scope,
            status: VerificationStatus::Fail,
            reason: Some(format!("missing required frontmatter key: {required_key}")),
        };
    }

    let validation_profile = detect_skill_validation_profile(&content);

    if validation_profile == SkillValidationProfile::Unknown {
        return VerificationCheck {
            skill: skill.name.clone(),
            scope,
            status: VerificationStatus::Fail,
            reason: Some("missing compatibility marker".to_string()),
        };
    }

    if !content.contains(CODEMOD_VERSION_MARKER_PREFIX) {
        return VerificationCheck {
            skill: skill.name.clone(),
            scope,
            status: VerificationStatus::Fail,
            reason: Some("missing skill version marker".to_string()),
        };
    }

    let allowed_tools = extract_allowed_tools(frontmatter);
    if allowed_tools.is_empty() {
        return VerificationCheck {
            skill: skill.name.clone(),
            scope,
            status: VerificationStatus::Fail,
            reason: Some("allowed-tools must contain at least one entry".to_string()),
        };
    }

    if validation_profile == SkillValidationProfile::Mcs {
        for allowed_tool in &allowed_tools {
            if !is_safe_allowed_tool(allowed_tool) {
                return VerificationCheck {
                    skill: skill.name.clone(),
                    scope,
                    status: VerificationStatus::Fail,
                    reason: Some(format!(
                        "unknown or unsafe allowed-tools entry: {allowed_tool}"
                    )),
                };
            }
        }
    }

    VerificationCheck {
        skill: skill.name.clone(),
        scope,
        status: VerificationStatus::Pass,
        reason: None,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SkillValidationProfile {
    Mcs,
    PackageSkill,
    Unknown,
}

fn detect_skill_validation_profile(content: &str) -> SkillValidationProfile {
    if content.contains(MCS_COMPATIBILITY_MARKER) {
        return SkillValidationProfile::Mcs;
    }

    if content.contains(CODEMOD_COMPATIBILITY_MARKER_PREFIX) {
        return SkillValidationProfile::PackageSkill;
    }

    SkillValidationProfile::Unknown
}

fn extract_frontmatter(content: &str) -> Option<&str> {
    if !content.starts_with("---") {
        return None;
    }

    let remaining = &content[3..];
    let end_marker_index = remaining.find("\n---")?;
    Some(remaining[..end_marker_index].trim())
}

fn extract_allowed_tools(frontmatter: &str) -> Vec<String> {
    let mut allowed_tools = Vec::new();
    let mut reading_allowed_tools = false;

    for line in frontmatter.lines() {
        let trimmed_line = line.trim();

        if !reading_allowed_tools {
            if trimmed_line == "allowed-tools:" {
                reading_allowed_tools = true;
            }
            continue;
        }

        if trimmed_line.starts_with("- ") {
            allowed_tools.push(trimmed_line.trim_start_matches("- ").trim().to_string());
            continue;
        }

        if trimmed_line.is_empty() {
            continue;
        }

        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
    }

    allowed_tools
}

fn missing_required_frontmatter_key(frontmatter: &str) -> Option<&'static str> {
    REQUIRED_FRONTMATTER_KEYS
        .iter()
        .find(|required_key| {
            !frontmatter
                .lines()
                .any(|line| line.trim().starts_with(**required_key))
        })
        .copied()
}

fn is_safe_allowed_tool(allowed_tool: &str) -> bool {
    allowed_tool.starts_with("Bash(codemod ")
}

fn is_codemod_managed_skill(skill_name: &str, skill_md_path: &Path) -> bool {
    if skill_name.starts_with("codemod") {
        return true;
    }

    let Ok(skill_md_content) = fs::read_to_string(skill_md_path) else {
        return false;
    };

    skill_md_content.contains(CODEMOD_COMPATIBILITY_MARKER_PREFIX)
        || skill_md_content.contains(CODEMOD_VERSION_MARKER_PREFIX)
}

fn read_skill_version_marker(skill_md_path: &Path) -> AdapterResult<String> {
    let skill_md_content = fs::read_to_string(skill_md_path).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!(
            "Failed to read {}: {error}",
            skill_md_path.display()
        ))
    })?;

    for line in skill_md_content.lines() {
        let trimmed = line.trim();
        if let Some(version) = trimmed.strip_prefix("codemod-skill-version:") {
            let version = version.trim();
            if !version.is_empty() {
                return Ok(version.to_string());
            }
        }
    }

    Err(HarnessAdapterError::InvalidSkillPackage(format!(
        "Missing skill version marker in {}",
        skill_md_path.display()
    )))
}

fn skill_roots_for_listing(
    harness: Harness,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<(InstallScope, PathBuf)>> {
    let mut roots = vec![(
        InstallScope::Project,
        skills_root_for_harness(harness, InstallScope::Project, runtime_paths)?,
    )];

    if runtime_paths.home_dir.is_some() {
        roots.push((
            InstallScope::User,
            skills_root_for_harness(harness, InstallScope::User, runtime_paths)?,
        ));
    }

    Ok(roots)
}

fn validate_embedded_mcs_bundle() -> AdapterResult<()> {
    let Some(frontmatter) = extract_frontmatter(MCS_SKILL_MD) else {
        return Err(HarnessAdapterError::InvalidSkillPackage(
            "SKILL.md is missing YAML frontmatter".to_string(),
        ));
    };

    if let Some(required_key) = missing_required_frontmatter_key(frontmatter) {
        return Err(HarnessAdapterError::InvalidSkillPackage(format!(
            "SKILL.md is missing required frontmatter key: {required_key}"
        )));
    }

    serde_yaml::from_str::<serde_yaml::Value>(frontmatter).map_err(|error| {
        HarnessAdapterError::InvalidSkillPackage(format!(
            "SKILL.md frontmatter is invalid YAML: {error}"
        ))
    })?;

    if !MCS_SKILL_MD.contains(MCS_COMPATIBILITY_MARKER) {
        return Err(HarnessAdapterError::InvalidSkillPackage(
            "SKILL.md is missing compatibility marker".to_string(),
        ));
    }

    if !MCS_SKILL_MD.contains(MCS_VERSION_MARKER) {
        return Err(HarnessAdapterError::InvalidSkillPackage(
            "SKILL.md is missing skill version marker".to_string(),
        ));
    }

    if !MCS_SKILL_MD.contains(MCS_REFERENCE_INDEX_RELATIVE_PATH) {
        return Err(HarnessAdapterError::InvalidSkillPackage(format!(
            "SKILL.md is missing reference link: {}",
            MCS_REFERENCE_INDEX_RELATIVE_PATH
        )));
    }

    for (relative_path, content) in MCS_REFERENCE_FILES {
        if content.trim().is_empty() {
            return Err(HarnessAdapterError::InvalidSkillPackage(format!(
                "{relative_path} is empty"
            )));
        }
    }

    for reference_path in MCS_INDEX_LINKED_REFERENCE_PATHS {
        if !MCS_REFERENCE_INDEX_MD.contains(reference_path) {
            return Err(HarnessAdapterError::InvalidSkillPackage(format!(
                "references/index.md is missing reference link: {reference_path}"
            )));
        }
    }

    Ok(())
}

fn skills_root_for_harness(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<PathBuf> {
    match scope {
        InstallScope::Project => Ok(match harness {
            Harness::Claude => runtime_paths.cwd.join(".claude/skills"),
            Harness::Goose => runtime_paths.cwd.join(".goose/skills"),
            Harness::Opencode => runtime_paths.cwd.join(".opencode/skills"),
            Harness::Cursor => runtime_paths.cwd.join(".cursor/skills"),
            Harness::Codex => runtime_paths.cwd.join(CODEX_WORKSPACE_SKILLS_RELATIVE_PATH),
            Harness::Antigravity => runtime_paths.cwd.join(".agents/skills"),
            Harness::Auto => {
                return Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()));
            }
        }),
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| match harness {
                Harness::Claude => home.join(".claude/skills"),
                Harness::Goose => home.join(".goose/skills"),
                Harness::Opencode => home.join(".opencode/skills"),
                Harness::Cursor => home.join(".cursor/skills"),
                Harness::Codex => home.join(".agents/skills"),
                Harness::Antigravity => home.join(".gemini/antigravity/skills"),
                Harness::Auto => PathBuf::new(),
            })
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            })
            .and_then(|root| {
                if root.as_os_str().is_empty() {
                    Err(HarnessAdapterError::UnsupportedHarness("auto".to_string()))
                } else {
                    Ok(root)
                }
            }),
    }
}

fn write_skill_file(path: &Path, content: &str, force: bool) -> AdapterResult<()> {
    if path.exists() && !force {
        if managed_text_file_requires_force(path, content)? {
            return Err(HarnessAdapterError::InstallFailed(format!(
                "Skill file already exists at {}. Re-run with --force to overwrite.",
                path.display()
            )));
        }

        return Ok(());
    }

    if let Some(parent_dir) = path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to create directory {}: {error}",
                parent_dir.display()
            ))
        })?;
    }

    fs::write(path, content).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!("Failed to write {}: {error}", path.display()))
    })?;

    Ok(())
}

fn managed_text_file_requires_force(path: &Path, content: &str) -> AdapterResult<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let existing = fs::read_to_string(path).map_err(|error| {
        HarnessAdapterError::InstallFailed(format!("Failed to read {}: {error}", path.display()))
    })?;
    Ok(existing != content)
}

fn write_package_skill_directory(
    source_dir: &Path,
    destination_dir: &Path,
    force: bool,
) -> AdapterResult<()> {
    if destination_dir.exists() {
        if force {
            fs::remove_dir_all(destination_dir).map_err(|error| {
                HarnessAdapterError::SkillPackageInstallFailed(format!(
                    "Failed to remove existing skill directory {}: {error}",
                    destination_dir.display()
                ))
            })?;
        } else if package_skill_directories_match(source_dir, destination_dir)? {
            // Idempotent install: existing files already match authored skill content.
            return Ok(());
        } else {
            return Err(HarnessAdapterError::SkillPackageInstallFailed(format!(
                "Skill directory already exists at {} with different content. Re-run with --force to overwrite.",
                destination_dir.display()
            )));
        }
    }

    copy_directory_recursive(source_dir, destination_dir)
}

fn copy_directory_recursive(source_dir: &Path, destination_dir: &Path) -> AdapterResult<()> {
    fs::create_dir_all(destination_dir).map_err(|error| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Failed to create destination skill directory {}: {error}",
            destination_dir.display()
        ))
    })?;

    let entries = fs::read_dir(source_dir).map_err(|error| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Failed to read source skill directory {}: {error}",
            source_dir.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            HarnessAdapterError::SkillPackageInstallFailed(format!(
                "Failed to read entry in source skill directory {}: {error}",
                source_dir.display()
            ))
        })?;
        let source_path = entry.path();
        let destination_path = destination_dir.join(entry.file_name());

        if source_path.is_dir() {
            copy_directory_recursive(&source_path, &destination_path)?;
        } else if source_path.is_file() {
            if let Some(parent_dir) = destination_path.parent() {
                fs::create_dir_all(parent_dir).map_err(|error| {
                    HarnessAdapterError::SkillPackageInstallFailed(format!(
                        "Failed to create destination directory {}: {error}",
                        parent_dir.display()
                    ))
                })?;
            }
            fs::copy(&source_path, &destination_path).map_err(|error| {
                HarnessAdapterError::SkillPackageInstallFailed(format!(
                    "Failed to copy skill file {} -> {}: {error}",
                    source_path.display(),
                    destination_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn package_skill_directories_match(
    source_dir: &Path,
    destination_dir: &Path,
) -> AdapterResult<bool> {
    let source_files = collect_relative_files(source_dir)?;
    let destination_files = collect_relative_files(destination_dir)?;

    if source_files != destination_files {
        return Ok(false);
    }

    for relative_path in source_files {
        let source_content = fs::read(source_dir.join(&relative_path)).map_err(|error| {
            HarnessAdapterError::SkillPackageInstallFailed(format!(
                "Failed to read source skill file {}: {error}",
                source_dir.join(&relative_path).display()
            ))
        })?;
        let destination_content =
            fs::read(destination_dir.join(&relative_path)).map_err(|error| {
                HarnessAdapterError::SkillPackageInstallFailed(format!(
                    "Failed to read destination skill file {}: {error}",
                    destination_dir.join(&relative_path).display()
                ))
            })?;

        if source_content != destination_content {
            return Ok(false);
        }
    }

    Ok(true)
}

fn collect_relative_files(root_dir: &Path) -> AdapterResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_relative_files_recursive(root_dir, root_dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_relative_files_recursive(
    root_dir: &Path,
    current_dir: &Path,
    files: &mut Vec<PathBuf>,
) -> AdapterResult<()> {
    let entries = fs::read_dir(current_dir).map_err(|error| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Failed to read skill directory {}: {error}",
            current_dir.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            HarnessAdapterError::SkillPackageInstallFailed(format!(
                "Failed to read entry in skill directory {}: {error}",
                current_dir.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files_recursive(root_dir, &path, files)?;
            continue;
        }

        if path.is_file() {
            let relative_path = path.strip_prefix(root_dir).map_err(|error| {
                HarnessAdapterError::SkillPackageInstallFailed(format!(
                    "Failed to normalize skill file path {}: {error}",
                    path.display()
                ))
            })?;
            files.push(relative_path.to_path_buf());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::manifest::CodemodManifest;
    use crate::utils::package_validation::validate_skill_behavior;
    use crate::utils::skill_layout::{
        derive_skill_name_from_package_name, AGENTS_SKILL_ROOT_RELATIVE_PATH,
    };
    use std::path::Path;
    use tempfile::tempdir;

    const ALL_HARNESSES: [Harness; 6] = [
        Harness::Claude,
        Harness::Goose,
        Harness::Opencode,
        Harness::Cursor,
        Harness::Codex,
        Harness::Antigravity,
    ];

    const MCP_CAPABLE_HARNESSES: [Harness; 5] = [
        Harness::Claude,
        Harness::Goose,
        Harness::Opencode,
        Harness::Cursor,
        Harness::Codex,
    ];

    fn expected_harness_root(
        runtime_paths: &RuntimePaths,
        harness: Harness,
        scope: InstallScope,
    ) -> PathBuf {
        match harness {
            Harness::Claude => match scope {
                InstallScope::Project => runtime_paths.cwd.join(".claude"),
                InstallScope::User => runtime_paths.home_dir.as_ref().unwrap().join(".claude"),
            },
            Harness::Goose => match scope {
                InstallScope::Project => runtime_paths.cwd.join(".goose"),
                InstallScope::User => runtime_paths.home_dir.as_ref().unwrap().join(".goose"),
            },
            Harness::Opencode => match scope {
                InstallScope::Project => runtime_paths.cwd.join(".opencode"),
                InstallScope::User => runtime_paths.home_dir.as_ref().unwrap().join(".opencode"),
            },
            Harness::Cursor => match scope {
                InstallScope::Project => runtime_paths.cwd.join(".cursor"),
                InstallScope::User => runtime_paths.home_dir.as_ref().unwrap().join(".cursor"),
            },
            Harness::Codex => match scope {
                InstallScope::Project => runtime_paths.cwd.join(CODEX_CONFIG_DIR_NAME),
                InstallScope::User => runtime_paths
                    .home_dir
                    .as_ref()
                    .unwrap()
                    .join(CODEX_CONFIG_DIR_NAME),
            },
            Harness::Antigravity => match scope {
                InstallScope::Project => runtime_paths.cwd.join(ANTIGRAVITY_WORKSPACE_ROOT),
                InstallScope::User => runtime_paths
                    .home_dir
                    .as_ref()
                    .unwrap()
                    .join(ANTIGRAVITY_USER_ROOT),
            },
            Harness::Auto => panic!("auto is not valid for harness-specific tests"),
        }
    }

    fn expected_project_mcp_path(runtime_paths: &RuntimePaths, harness: Harness) -> PathBuf {
        match harness {
            Harness::Claude => runtime_paths.cwd.join(".mcp.json"),
            Harness::Goose => runtime_paths.cwd.join(".goose/mcp.json"),
            Harness::Opencode => runtime_paths.cwd.join(".opencode/mcp.json"),
            Harness::Cursor => runtime_paths.cwd.join(".cursor/mcp.json"),
            Harness::Codex => runtime_paths.cwd.join(".codex/config.toml"),
            Harness::Antigravity => panic!("antigravity does not have MCP config support"),
            Harness::Auto => panic!("auto is not valid for harness-specific tests"),
        }
    }

    fn expected_managed_state_path(
        runtime_paths: &RuntimePaths,
        harness: Harness,
        scope: InstallScope,
    ) -> PathBuf {
        expected_harness_root(runtime_paths, harness, scope)
            .join(CODEMOD_MANAGED_STATE_RELATIVE_PATH)
    }

    fn expected_periodic_runner_path(
        runtime_paths: &RuntimePaths,
        harness: Harness,
        scope: InstallScope,
    ) -> PathBuf {
        expected_harness_root(runtime_paths, harness, scope)
            .join(CODEMOD_PERIODIC_UPDATE_RELATIVE_DIR)
            .join(CODEMOD_PERIODIC_UPDATE_RUNNER_FILE_NAME)
    }

    fn expected_periodic_integration_path(
        runtime_paths: &RuntimePaths,
        harness: Harness,
        scope: InstallScope,
    ) -> PathBuf {
        let harness_root = expected_harness_root(runtime_paths, harness, scope);
        periodic_update_trigger_strategy(harness, scope, runtime_paths, &harness_root)
            .unwrap()
            .integration_path
    }

    fn runtime_paths_with_temp_roots() -> (RuntimePaths, tempfile::TempDir) {
        let temp_dir = tempdir().unwrap();
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let codemod_executable = bin_dir.join("codemod");
        fs::write(&codemod_executable, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&codemod_executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&codemod_executable, permissions).unwrap();
        }

        let runtime_paths = RuntimePaths {
            cwd: temp_dir.path().join("workspace"),
            home_dir: Some(temp_dir.path().join("home")),
            current_executable: Some(codemod_executable),
        };
        std::fs::create_dir_all(&runtime_paths.cwd).unwrap();
        std::fs::create_dir_all(runtime_paths.home_dir.as_ref().unwrap()).unwrap();
        (runtime_paths, temp_dir)
    }

    fn create_authored_skill_source(base_dir: &Path, package_id: &str) -> PathBuf {
        let source_dir = base_dir
            .join("authored-skill")
            .join(skill_directory_name_for_package_id(package_id));
        fs::create_dir_all(source_dir.join("references")).unwrap();
        let skill_md = format!(
            r#"---
name: "{package_id}"
description: "Migrate Jest test suites to Vitest."
allowed-tools:
  - Bash(codemod *)
---
{compatibility_marker}
codemod-skill-version: 0.1.0
"#,
            compatibility_marker = SKILL_PACKAGE_COMPATIBILITY_MARKER
        );
        fs::write(source_dir.join("SKILL.md"), skill_md).unwrap();
        fs::write(
            source_dir.join("references/index.md"),
            "# References\n\n- [Usage](./usage.md)\n",
        )
        .unwrap();
        fs::write(source_dir.join("references/usage.md"), "# Usage\n").unwrap();
        source_dir
    }

    fn count_occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    fn managed_snapshots_from_install(
        installed: &[InstalledSkill],
        discovery_paths: &[PathBuf],
    ) -> Vec<ManagedComponentSnapshot> {
        let mut snapshots = installed
            .iter()
            .map(|entry| ManagedComponentSnapshot {
                id: entry.name.clone(),
                kind: if entry.name == "codemod-mcp" {
                    ManagedComponentKind::McpConfig
                } else {
                    ManagedComponentKind::Skill
                },
                path: entry.path.clone(),
                version: entry.version.clone(),
            })
            .collect::<Vec<_>>();

        for path in discovery_paths {
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| format!("discovery-guide:{name}"))
                .unwrap_or_else(|| format!("discovery-guide:{}", path.to_string_lossy()));
            snapshots.push(ManagedComponentSnapshot {
                id,
                kind: ManagedComponentKind::DiscoveryGuide,
                path: path.clone(),
                version: None,
            });
        }

        snapshots
    }

    fn create_skill_only_package_layout(
        base_dir: &Path,
        package_name: &str,
    ) -> (CodemodManifest, PathBuf) {
        let package_root = base_dir.join(package_name);
        fs::create_dir_all(&package_root).unwrap();

        let manifest_yaml = format!(
            r#"schema_version: "1.0"
name: "{package_name}"
version: "0.1.0"
description: "Skill-only package for harness install tests"
author: "Codemod Team <team@codemod.com>"
license: "MIT"
workflow: "workflow.yaml"
capabilities: []
"#
        );
        fs::write(package_root.join("codemod.yaml"), &manifest_yaml).unwrap();
        let manifest: CodemodManifest = serde_yaml::from_str(&manifest_yaml).unwrap();
        fs::write(
            package_root.join("workflow.yaml"),
            format!(
                r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install skill
        install-skill:
          package: "{package_name}"
"#
            ),
        )
        .unwrap();

        let skill_name = derive_skill_name_from_package_name(package_name);
        let skill_dir = package_root
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join(skill_name);
        fs::create_dir_all(skill_dir.join("references")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: "sample-skill"
description: "Installable skill package"
allowed-tools:
  - Bash(codemod *)
---
codemod-compatibility: skill-package-v1
codemod-skill-version: 0.1.0
"#,
        )
        .unwrap();
        fs::write(
            skill_dir.join("references/index.md"),
            "- [Usage](./usage.md)\n",
        )
        .unwrap();
        fs::write(skill_dir.join("references/usage.md"), "# Usage\n").unwrap();

        (manifest, skill_dir)
    }

    #[test]
    fn resolve_adapter_returns_known_harnesses() {
        assert_eq!(
            resolve_adapter(Harness::Claude).unwrap().harness,
            Harness::Claude
        );
        assert_eq!(
            resolve_adapter(Harness::Goose).unwrap().harness,
            Harness::Goose
        );
        assert_eq!(
            resolve_adapter(Harness::Opencode).unwrap().harness,
            Harness::Opencode
        );
        assert_eq!(
            resolve_adapter(Harness::Cursor).unwrap().harness,
            Harness::Cursor
        );
        assert_eq!(
            resolve_adapter(Harness::Codex).unwrap().harness,
            Harness::Codex
        );
        assert_eq!(
            resolve_adapter(Harness::Antigravity).unwrap().harness,
            Harness::Antigravity
        );
    }

    #[test]
    fn upsert_skill_discovery_guides_for_claude_only_writes_claude_file() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        let updated_files = upsert_skill_discovery_guides_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();

        assert_eq!(updated_files.len(), 1);
        let agents_path = runtime_paths.cwd.join("AGENTS.md");
        let claude_path = runtime_paths.cwd.join("CLAUDE.md");
        assert!(!agents_path.exists());
        assert!(claude_path.exists());

        let claude_content = fs::read_to_string(&claude_path).unwrap();
        assert!(claude_content.contains(SKILL_DISCOVERY_SECTION_BEGIN));
        assert!(claude_content.contains(SKILL_DISCOVERY_SECTION_END));
        assert!(claude_content.contains("Core skill: `.claude/skills/codemod/SKILL.md`"));
        assert!(claude_content.contains(".claude/skills/codemod/SKILL.md"));
        assert!(!claude_content.contains("Installed Codemod skills root"));
        assert!(!claude_content.contains("Restart or reload your claude session"));
    }

    #[test]
    fn upsert_skill_discovery_guides_is_idempotent_without_duplication() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let claude_path = runtime_paths.cwd.join("CLAUDE.md");
        fs::write(&claude_path, "# Existing guidance\n").unwrap();

        let first_update = upsert_skill_discovery_guides_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        assert!(!first_update.is_empty());

        let second_update = upsert_skill_discovery_guides_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        assert!(second_update.is_empty());

        let content = fs::read_to_string(&claude_path).unwrap();
        assert!(content.contains("# Existing guidance"));
        assert_eq!(
            count_occurrences(&content, SKILL_DISCOVERY_SECTION_BEGIN),
            1
        );
        assert_eq!(count_occurrences(&content, SKILL_DISCOVERY_SECTION_END), 1);
    }

    #[test]
    fn upsert_skill_discovery_guides_writes_user_scope_files_under_home() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        let updated_files = upsert_skill_discovery_guides_with_runtime(
            Harness::Cursor,
            InstallScope::User,
            &runtime_paths,
        )
        .unwrap();

        assert_eq!(updated_files.len(), 2);
        let agents_path = runtime_paths.home_dir.as_ref().unwrap().join("AGENTS.md");
        let content = fs::read_to_string(&agents_path).unwrap();
        assert!(content.contains("~/.cursor/skills/codemod/SKILL.md"));
        assert!(content.contains("npx codemod agent list --harness cursor --format json"));
    }

    #[test]
    fn upsert_periodic_update_trigger_supports_all_harnesses() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        for harness in ALL_HARNESSES {
            let result = upsert_periodic_update_trigger_with_runtime(
                harness,
                InstallScope::Project,
                PeriodicUpdatePolicy::AutoSafe,
                &runtime_paths,
            )
            .unwrap();
            let runner_path =
                expected_periodic_runner_path(&runtime_paths, harness, InstallScope::Project);
            let integration_path =
                expected_periodic_integration_path(&runtime_paths, harness, InstallScope::Project);
            assert!(runner_path.exists());
            assert!(result.tracked_paths.contains(&runner_path));
            assert!(integration_path.exists());
            assert!(result.tracked_paths.contains(&integration_path));

            match harness {
                Harness::Claude => {
                    let settings_path = expected_harness_root(
                        &runtime_paths,
                        Harness::Claude,
                        InstallScope::Project,
                    )
                    .join(CODEMOD_PERIODIC_TRIGGER_CLAUDE_SETTINGS_FILE_NAME);
                    let settings = fs::read_to_string(&settings_path).unwrap();
                    assert!(settings.contains(CODEMOD_PERIODIC_TRIGGER_CLAUDE_SESSION_START_EVENT));
                    assert!(settings.contains(&runner_path.to_string_lossy().to_string()));
                }
                Harness::Goose => {
                    let hints_path = runtime_paths
                        .cwd
                        .join(CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_FILE_NAME);
                    let hints = fs::read_to_string(&hints_path).unwrap();
                    assert!(hints.contains(CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_BEGIN));
                    assert!(hints.contains(&runner_path.to_string_lossy().to_string()));
                }
                Harness::Cursor => {
                    let hooks_path = expected_harness_root(
                        &runtime_paths,
                        Harness::Cursor,
                        InstallScope::Project,
                    )
                    .join(CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOKS_FILE_NAME);
                    let hooks_content = fs::read_to_string(&hooks_path).unwrap();
                    let hooks_json: Value = serde_json::from_str(&hooks_content).unwrap();
                    assert_eq!(hooks_json["version"], json!(1));
                    let commands = hooks_json["hooks"]
                        [CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOK_EVENT_NAME]
                        .as_array()
                        .unwrap();
                    assert!(commands.iter().any(|entry| {
                        entry["command"]
                            .as_str()
                            .is_some_and(|command| command == runner_path.to_string_lossy())
                    }));
                }
                Harness::Opencode => {
                    let plugin_path = expected_harness_root(
                        &runtime_paths,
                        Harness::Opencode,
                        InstallScope::Project,
                    )
                    .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_DIR_NAME)
                    .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_FILE_NAME);
                    let plugin = fs::read_to_string(&plugin_path).unwrap();
                    assert!(plugin.contains("export async function CodemodPeriodicUpdate"));
                    assert!(plugin.contains(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_EVENT_NAME));
                    assert!(plugin.contains(&runner_path.to_string_lossy().to_string()));
                }
                Harness::Codex => {
                    let config_path = expected_harness_root(
                        &runtime_paths,
                        Harness::Codex,
                        InstallScope::Project,
                    )
                    .join(CODEMOD_PERIODIC_TRIGGER_CODEX_CONFIG_FILE_NAME);
                    let config_content = fs::read_to_string(&config_path).unwrap();
                    let config_doc = config_content.parse::<DocumentMut>().unwrap();
                    let notify = read_notify_command(&config_doc).unwrap().unwrap();
                    assert_eq!(notify[0], "sh");
                    assert_eq!(notify[1], runner_path.to_string_lossy());
                }
                Harness::Antigravity => {
                    let hints_path = expected_harness_root(
                        &runtime_paths,
                        Harness::Antigravity,
                        InstallScope::Project,
                    )
                    .join(CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_FILE_NAME);
                    let hints = fs::read_to_string(&hints_path).unwrap();
                    assert!(hints.contains(CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_BEGIN));
                    assert!(hints.contains(&runner_path.to_string_lossy().to_string()));
                }
                Harness::Auto => unreachable!("auto is not part of ALL_HARNESSES"),
            }
        }
    }

    #[test]
    fn periodic_update_runner_script_embeds_selected_policy_and_signed_manifest_default() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let state_path = PathBuf::from("/tmp/codemod/periodic-update/state");
        let auto_safe = build_periodic_update_runner_script(
            Harness::Claude,
            "--project",
            PeriodicUpdatePolicy::AutoSafe,
            &state_path,
            &runtime_paths,
        );
        assert!(auto_safe.contains("--update-policy"));
        assert!(auto_safe.contains("auto-safe"));
        assert!(auto_safe.contains("--require-signed-manifest"));
        assert!(auto_safe.contains("--format"));
        assert!(auto_safe.contains("json"));

        let manual = build_periodic_update_runner_script(
            Harness::Claude,
            "--project",
            PeriodicUpdatePolicy::Manual,
            &state_path,
            &runtime_paths,
        );
        assert!(manual.contains("--update-policy"));
        assert!(manual.contains("manual"));
        assert!(manual.contains("--require-signed-manifest"));
        assert!(manual.contains("--format"));
        assert!(manual.contains("json"));
    }

    #[test]
    fn periodic_update_strategy_uses_opencode_user_plugin_config_dir() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let harness_root =
            expected_harness_root(&runtime_paths, Harness::Opencode, InstallScope::User);
        let strategy = periodic_update_trigger_strategy(
            Harness::Opencode,
            InstallScope::User,
            &runtime_paths,
            &harness_root,
        )
        .unwrap();

        assert!(strategy
            .integration_path
            .ends_with(".config/opencode/plugins/codemod-periodic-update.js"));
    }

    #[test]
    fn upsert_periodic_update_trigger_is_idempotent_for_claude_hook() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        let first = upsert_periodic_update_trigger_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            PeriodicUpdatePolicy::AutoSafe,
            &runtime_paths,
        )
        .unwrap();
        assert!(!first.updated_paths.is_empty());

        let second = upsert_periodic_update_trigger_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            PeriodicUpdatePolicy::AutoSafe,
            &runtime_paths,
        )
        .unwrap();
        assert!(second.updated_paths.is_empty());

        let runner_path =
            expected_periodic_runner_path(&runtime_paths, Harness::Claude, InstallScope::Project);
        let settings_path =
            expected_harness_root(&runtime_paths, Harness::Claude, InstallScope::Project)
                .join(CODEMOD_PERIODIC_TRIGGER_CLAUDE_SETTINGS_FILE_NAME);
        let settings = fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            count_occurrences(&settings, &runner_path.to_string_lossy()),
            1
        );
    }

    #[test]
    fn upsert_periodic_update_trigger_is_idempotent_for_cursor_hook() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        let first = upsert_periodic_update_trigger_with_runtime(
            Harness::Cursor,
            InstallScope::Project,
            PeriodicUpdatePolicy::AutoSafe,
            &runtime_paths,
        )
        .unwrap();
        assert!(!first.updated_paths.is_empty());

        let second = upsert_periodic_update_trigger_with_runtime(
            Harness::Cursor,
            InstallScope::Project,
            PeriodicUpdatePolicy::AutoSafe,
            &runtime_paths,
        )
        .unwrap();
        assert!(second.updated_paths.is_empty());

        let runner_path =
            expected_periodic_runner_path(&runtime_paths, Harness::Cursor, InstallScope::Project);
        let hooks_path =
            expected_harness_root(&runtime_paths, Harness::Cursor, InstallScope::Project)
                .join(CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOKS_FILE_NAME);
        let hooks: Value = serde_json::from_str(&fs::read_to_string(hooks_path).unwrap()).unwrap();
        let entries = hooks["hooks"][CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOK_EVENT_NAME]
            .as_array()
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| {
                    entry["command"]
                        .as_str()
                        .is_some_and(|command| command == runner_path.to_string_lossy())
                })
                .count(),
            1
        );
    }

    #[test]
    fn upsert_periodic_update_trigger_is_idempotent_for_opencode_plugin() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        let first = upsert_periodic_update_trigger_with_runtime(
            Harness::Opencode,
            InstallScope::Project,
            PeriodicUpdatePolicy::AutoSafe,
            &runtime_paths,
        )
        .unwrap();
        assert!(!first.updated_paths.is_empty());

        let second = upsert_periodic_update_trigger_with_runtime(
            Harness::Opencode,
            InstallScope::Project,
            PeriodicUpdatePolicy::AutoSafe,
            &runtime_paths,
        )
        .unwrap();
        assert!(second.updated_paths.is_empty());

        let plugin_path =
            expected_harness_root(&runtime_paths, Harness::Opencode, InstallScope::Project)
                .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_DIR_NAME)
                .join(CODEMOD_PERIODIC_TRIGGER_OPENCODE_PLUGIN_FILE_NAME);
        let plugin = fs::read_to_string(plugin_path).unwrap();
        assert_eq!(
            count_occurrences(&plugin, "export async function CodemodPeriodicUpdate"),
            1
        );
    }

    #[test]
    fn resolve_install_scope_rejects_conflicting_flags() {
        assert!(resolve_install_scope(true, true).is_err());
    }

    #[test]
    fn auto_detect_prefers_claude_if_both_roots_exist() {
        let temp_dir = tempdir().unwrap();
        fs::create_dir_all(temp_dir.path().join(".claude")).unwrap();
        fs::create_dir_all(temp_dir.path().join(".goose")).unwrap();

        let (harness, warnings) = detect_auto_harness(temp_dir.path());
        assert_eq!(harness, Harness::Claude);
        assert!(warnings.is_empty());
    }

    #[test]
    fn auto_detect_uses_goose_when_claude_root_is_absent() {
        let temp_dir = tempdir().unwrap();
        fs::create_dir_all(temp_dir.path().join(".goose")).unwrap();

        let (harness, warnings) = detect_auto_harness(temp_dir.path());
        assert_eq!(harness, Harness::Goose);
        assert!(warnings.is_empty());
    }

    #[test]
    fn auto_detect_falls_back_to_claude_with_warning() {
        let temp_dir = tempdir().unwrap();
        let (harness, warnings) = detect_auto_harness(temp_dir.path());
        assert_eq!(harness, Harness::Claude);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn auto_detect_uses_opencode_when_claude_and_goose_roots_are_absent() {
        let temp_dir = tempdir().unwrap();
        fs::create_dir_all(temp_dir.path().join(".opencode")).unwrap();

        let (harness, warnings) = detect_auto_harness(temp_dir.path());
        assert_eq!(harness, Harness::Opencode);
        assert!(warnings.is_empty());
    }

    #[test]
    fn auto_detect_uses_cursor_when_only_cursor_root_exists() {
        let temp_dir = tempdir().unwrap();
        fs::create_dir_all(temp_dir.path().join(".cursor")).unwrap();

        let (harness, warnings) = detect_auto_harness(temp_dir.path());
        assert_eq!(harness, Harness::Cursor);
        assert!(warnings.is_empty());
    }

    #[test]
    fn install_mcs_skill_bundle_writes_expected_files() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        let installed = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        let installed_skill = installed.first().unwrap();
        let mcp_entry = installed
            .iter()
            .find(|entry| entry.name == "codemod-mcp")
            .expect("expected MCP install entry");

        assert_eq!(installed_skill.name, MCS_SKILL_COMPONENT_ID);
        assert!(installed_skill.path.exists());
        assert!(installed_skill
            .path
            .to_string_lossy()
            .contains(".claude/skills/codemod/SKILL.md"));

        let skill_root = runtime_paths
            .cwd
            .join(".claude")
            .join("skills")
            .join("codemod");

        for (relative_path, _) in MCS_REFERENCE_FILES {
            assert!(
                skill_root.join(relative_path).exists(),
                "expected installed file to exist: {}",
                relative_path
            );
        }

        assert!(mcp_entry.path.exists());
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&mcp_entry.path).unwrap()).unwrap();
        let command = config
            .get("mcpServers")
            .and_then(|servers| servers.get("codemod"))
            .and_then(|server| server.get("command"))
            .and_then(|value| value.as_str())
            .unwrap();
        let args = config
            .get("mcpServers")
            .and_then(|servers| servers.get("codemod"))
            .and_then(|server| server.get("args"))
            .and_then(|value| value.as_array())
            .unwrap();
        assert_eq!(
            command,
            runtime_paths
                .current_executable
                .as_ref()
                .unwrap()
                .to_string_lossy()
        );
        assert_eq!(args.last().and_then(|value| value.as_str()), Some("mcp"));
    }

    #[test]
    fn install_mcs_skill_bundle_supports_all_harnesses() {
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        for harness in ALL_HARNESSES {
            let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
            let installed =
                install_mcs_skill_bundle_with_runtime(harness, &install_request, &runtime_paths)
                    .unwrap();
            let installed_skill = installed.first().unwrap();
            let expected_skills_root =
                skills_root_for_harness(harness, InstallScope::Project, &runtime_paths).unwrap();
            assert_eq!(
                installed_skill.path,
                expected_skills_root.join("codemod").join("SKILL.md")
            );

            if harness_supports_mcp(harness) {
                let mcp_entry = installed
                    .iter()
                    .find(|entry| entry.name == "codemod-mcp")
                    .expect("expected MCP install entry");
                assert_eq!(
                    mcp_entry.path,
                    expected_project_mcp_path(&runtime_paths, harness)
                );
                assert!(mcp_entry.path.exists());
            } else {
                assert!(installed.iter().all(|entry| entry.name != "codemod-mcp"));
            }
        }
    }

    #[test]
    fn persist_managed_install_state_is_created_then_unchanged() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        let installed = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        upsert_skill_discovery_guides_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        let discovery_paths = discovery_guide_paths_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        let snapshots = managed_snapshots_from_install(&installed, &discovery_paths);

        let first = persist_managed_install_state_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &snapshots,
            &runtime_paths,
        )
        .unwrap();
        let second = persist_managed_install_state_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &snapshots,
            &runtime_paths,
        )
        .unwrap();

        assert_eq!(first.status, ManagedStateWriteStatus::Created);
        assert_eq!(second.status, ManagedStateWriteStatus::Unchanged);
        assert!(first.path.exists());
        let lock_path = managed_state_lock_path(&first.path).unwrap();
        assert!(!lock_path.exists());
    }

    #[test]
    fn persist_managed_install_state_reports_updated_when_component_changes() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        let installed = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        upsert_skill_discovery_guides_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        let discovery_paths = discovery_guide_paths_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        let snapshots = managed_snapshots_from_install(&installed, &discovery_paths);

        let first = persist_managed_install_state_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &snapshots,
            &runtime_paths,
        )
        .unwrap();
        assert_eq!(first.status, ManagedStateWriteStatus::Created);

        fs::write(
            installed
                .iter()
                .find(|entry| entry.name == MCS_SKILL_COMPONENT_ID)
                .unwrap()
                .path
                .clone(),
            format!("{MCS_SKILL_MD}\n# updated\n"),
        )
        .unwrap();

        let second = persist_managed_install_state_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &snapshots,
            &runtime_paths,
        )
        .unwrap();
        assert_eq!(second.status, ManagedStateWriteStatus::Updated);
    }

    #[test]
    fn persist_managed_install_state_supports_all_harnesses_and_scopes() {
        for harness in ALL_HARNESSES {
            for scope in [InstallScope::Project, InstallScope::User] {
                let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
                let install_request = InstallRequest {
                    scope,
                    force: false,
                };

                let installed = install_mcs_skill_bundle_with_runtime(
                    harness,
                    &install_request,
                    &runtime_paths,
                )
                .unwrap();
                upsert_skill_discovery_guides_with_runtime(harness, scope, &runtime_paths).unwrap();
                let discovery_paths =
                    discovery_guide_paths_with_runtime(harness, scope, &runtime_paths).unwrap();
                let snapshots = managed_snapshots_from_install(&installed, &discovery_paths);

                let state_write = persist_managed_install_state_with_runtime(
                    harness,
                    scope,
                    &snapshots,
                    &runtime_paths,
                )
                .unwrap();

                assert_eq!(state_write.status, ManagedStateWriteStatus::Created);
                assert_eq!(
                    state_write.path,
                    expected_managed_state_path(&runtime_paths, harness, scope)
                );

                let state_content = fs::read_to_string(&state_write.path).unwrap();
                assert!(state_content.contains("\"schema_version\": \"1\""));
                assert!(state_content.contains(&format!("\"harness\": \"{}\"", harness.as_str())));
                assert!(state_content.contains(&format!("\"scope\": \"{}\"", scope.as_str())));
            }
        }
    }

    #[test]
    fn read_managed_install_state_restores_snapshots_after_persist() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        let installed = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        upsert_skill_discovery_guides_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        let discovery_paths = discovery_guide_paths_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap();
        let snapshots = managed_snapshots_from_install(&installed, &discovery_paths);
        persist_managed_install_state_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &snapshots,
            &runtime_paths,
        )
        .unwrap();

        let loaded = read_managed_install_state_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap()
        .expect("expected managed state");

        assert_eq!(
            loaded.path,
            expected_managed_state_path(&runtime_paths, Harness::Claude, InstallScope::Project)
        );
        assert_eq!(loaded.components.len(), snapshots.len());
        assert!(loaded
            .components
            .iter()
            .any(|component| component.id == "codemod"));
        assert!(loaded
            .components
            .iter()
            .any(|component| component.id == "codemod-mcp"));
        assert!(loaded
            .components
            .iter()
            .any(|component| component.id == "discovery-guide:CLAUDE.md"));
    }

    #[test]
    fn managed_state_lock_recovery_removes_stale_lock() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let state_path =
            expected_managed_state_path(&runtime_paths, Harness::Claude, InstallScope::Project);
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        let lock_path = managed_state_lock_path(&state_path).unwrap();
        let stale_lock = ManagedStateLockMetadata {
            pid: 9999,
            acquired_at_epoch_secs: now_epoch_secs().saturating_sub(120),
        };
        fs::write(&lock_path, serde_json::to_vec(&stale_lock).unwrap()).unwrap();

        let guard = acquire_managed_state_lock_with_policy(
            &state_path,
            ManagedStateLockPolicy {
                timeout: Duration::from_millis(40),
                retry_interval: Duration::from_millis(10),
                stale_after: Duration::from_secs(1),
            },
        )
        .unwrap();
        assert!(lock_path.exists());
        guard.release();
        assert!(!lock_path.exists());
    }

    #[test]
    fn managed_state_lock_timeout_is_deterministic() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let state_path =
            expected_managed_state_path(&runtime_paths, Harness::Claude, InstallScope::Project);
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        let lock_path = managed_state_lock_path(&state_path).unwrap();
        try_create_managed_state_lock(&lock_path).unwrap();

        let lock_error = acquire_managed_state_lock_with_policy(
            &state_path,
            ManagedStateLockPolicy {
                timeout: Duration::from_millis(40),
                retry_interval: Duration::from_millis(10),
                stale_after: Duration::from_secs(600),
            },
        )
        .unwrap_err();

        assert!(matches!(
            lock_error,
            HarnessAdapterError::InstallFailed(message)
                if message.contains("timed out")
                    && message.contains("40ms")
                    && message.contains("10ms")
        ));
        let _ = release_managed_state_lock(&lock_path);
    }

    #[test]
    fn reference_index_links_all_reference_files() {
        for reference_path in MCS_INDEX_LINKED_REFERENCE_PATHS {
            assert!(
                MCS_REFERENCE_INDEX_MD.contains(reference_path),
                "reference index should link to {}",
                reference_path
            );
        }
    }

    #[test]
    fn skill_md_routes_creation_tasks_to_creation_references() {
        assert!(MCS_SKILL_MD.contains(MCS_CREATE_CODEMODS_RELATIVE_PATH));
        assert!(MCS_SKILL_MD.contains(MCS_MAINTAINER_MONOREPO_RELATIVE_PATH));
    }

    #[test]
    fn install_mcs_skill_bundle_is_idempotent_without_force() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        install_mcs_skill_bundle_with_runtime(Harness::Claude, &install_request, &runtime_paths)
            .unwrap();

        let second_install = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &install_request,
            &runtime_paths,
        );
        assert!(second_install.is_ok());
    }

    #[test]
    fn install_mcs_skill_bundle_supports_forced_overwrite() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let first_install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let forced_install_request = InstallRequest {
            scope: InstallScope::Project,
            force: true,
        };

        install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &first_install_request,
            &runtime_paths,
        )
        .unwrap();

        let forced = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &forced_install_request,
            &runtime_paths,
        );

        assert!(forced.is_ok());
    }

    #[test]
    fn install_package_skill_bundle_writes_expected_skill_file() {
        let (runtime_paths, temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let source_dir = create_authored_skill_source(temp_dir.path(), "jest-to-vitest");
        let package_skill = SkillPackageInstallSpec {
            id: "jest-to-vitest".to_string(),
            version: "0.1.0".to_string(),
            description: "Migrate Jest test suites to Vitest.".to_string(),
            source_dir,
        };

        let installed = install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &package_skill,
            &install_request,
            &runtime_paths,
        )
        .unwrap();

        assert_eq!(installed.len(), 1);
        let installed_skill = installed.first().unwrap();
        assert_eq!(installed_skill.name, "jest-to-vitest");
        assert_eq!(installed_skill.version, Some("0.1.0".to_string()));
        assert!(installed_skill.path.exists());
        assert!(installed_skill
            .path
            .to_string_lossy()
            .contains(".claude/skills/jest-to-vitest/SKILL.md"));
    }

    #[test]
    fn install_package_skill_bundle_copies_all_authored_files_recursively() {
        let (runtime_paths, temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let source_dir = create_authored_skill_source(temp_dir.path(), "jest-to-vitest");
        fs::create_dir_all(source_dir.join("references/deep")).unwrap();
        fs::write(
            source_dir.join("references/deep/additional.md"),
            "# Extra guidance\n\nCustom guidance.\n",
        )
        .unwrap();
        fs::write(source_dir.join("notes.md"), "author note\n").unwrap();
        let authored_skill = fs::read_to_string(source_dir.join("SKILL.md")).unwrap();
        fs::write(
            source_dir.join("SKILL.md"),
            format!("{authored_skill}\n# custom-authored\n"),
        )
        .unwrap();

        let package_skill = SkillPackageInstallSpec {
            id: "jest-to-vitest".to_string(),
            version: "0.1.0".to_string(),
            description: "Migrate Jest test suites to Vitest.".to_string(),
            source_dir: source_dir.clone(),
        };

        let installed = install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &package_skill,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        let installed_skill_root = installed[0].path.parent().unwrap();

        assert_eq!(
            fs::read_to_string(installed_skill_root.join("SKILL.md")).unwrap(),
            fs::read_to_string(source_dir.join("SKILL.md")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(installed_skill_root.join("notes.md")).unwrap(),
            "author note\n"
        );
        assert_eq!(
            fs::read_to_string(installed_skill_root.join("references/deep/additional.md")).unwrap(),
            "# Extra guidance\n\nCustom guidance.\n"
        );
    }

    #[test]
    fn install_package_skill_bundle_supports_all_harnesses() {
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        for harness in ALL_HARNESSES {
            let (runtime_paths, temp_dir) = runtime_paths_with_temp_roots();
            let source_dir = create_authored_skill_source(temp_dir.path(), "jest-to-vitest");
            let package_skill = SkillPackageInstallSpec {
                id: "jest-to-vitest".to_string(),
                version: "0.1.0".to_string(),
                description: "Migrate Jest test suites to Vitest.".to_string(),
                source_dir,
            };
            let installed = install_package_skill_bundle_with_runtime(
                harness,
                &package_skill,
                &install_request,
                &runtime_paths,
            )
            .unwrap();
            let expected_root =
                skills_root_for_harness(harness, InstallScope::Project, &runtime_paths).unwrap();
            assert_eq!(
                installed[0].path,
                expected_root.join("jest-to-vitest").join("SKILL.md")
            );
        }
    }

    #[test]
    fn skill_only_package_validate_then_install_flow_works_across_harnesses() {
        let package_temp_dir = tempdir().unwrap();
        let (manifest, skill_source_dir) =
            create_skill_only_package_layout(package_temp_dir.path(), "sample-skill");
        let package_root = package_temp_dir.path().join("sample-skill");

        let validation_summary = validate_skill_behavior(&package_root, &manifest)
            .expect("skill-only package layout should pass shared validation");
        assert_eq!(validation_summary.linked_reference_count, 1);

        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let package_skill = SkillPackageInstallSpec {
            id: "sample-skill".to_string(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            source_dir: skill_source_dir,
        };

        for harness in ALL_HARNESSES {
            let (runtime_paths, _runtime_temp) = runtime_paths_with_temp_roots();
            let installed = install_package_skill_bundle_with_runtime(
                harness,
                &package_skill,
                &install_request,
                &runtime_paths,
            )
            .unwrap();
            assert_eq!(installed.len(), 1);

            let listed = list_skills_with_runtime(harness, &runtime_paths).unwrap();
            assert!(listed.iter().any(|skill| skill.name == "sample-skill"));

            let checks = verify_skills_with_runtime(harness, &runtime_paths).unwrap();
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0].status, VerificationStatus::Pass);
        }
    }

    #[test]
    fn install_package_skill_bundle_is_idempotent_without_force() {
        let (runtime_paths, temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let source_dir = create_authored_skill_source(temp_dir.path(), "jest-to-vitest");
        let package_skill = SkillPackageInstallSpec {
            id: "jest-to-vitest".to_string(),
            version: "0.1.0".to_string(),
            description: "Migrate Jest test suites to Vitest.".to_string(),
            source_dir,
        };

        let first_install = install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &package_skill,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        let second_install = install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &package_skill,
            &install_request,
            &runtime_paths,
        )
        .unwrap();

        assert_eq!(first_install.len(), 1);
        assert_eq!(second_install.len(), 1);
        assert_eq!(first_install[0].path, second_install[0].path);
    }

    #[test]
    fn install_package_skill_bundle_requires_force_when_authored_content_changes() {
        let (runtime_paths, temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let source_dir = create_authored_skill_source(temp_dir.path(), "jest-to-vitest");
        let package_skill = SkillPackageInstallSpec {
            id: "jest-to-vitest".to_string(),
            version: "0.1.0".to_string(),
            description: "Migrate Jest test suites to Vitest.".to_string(),
            source_dir: source_dir.clone(),
        };

        install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &package_skill,
            &install_request,
            &runtime_paths,
        )
        .unwrap();

        fs::write(source_dir.join("references/new-notes.md"), "# New notes\n").unwrap();

        let second_install = install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &package_skill,
            &install_request,
            &runtime_paths,
        );

        assert!(matches!(
            second_install,
            Err(HarnessAdapterError::SkillPackageInstallFailed(message))
                if message.contains("--force")
        ));
    }

    #[test]
    fn mcs_install_requires_force_detects_conflicting_embedded_files() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        install_mcs_skill_bundle_with_runtime(Harness::Claude, &install_request, &runtime_paths)
            .unwrap();

        assert!(!mcs_install_requires_force_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths
        )
        .unwrap());

        let skill_path = runtime_paths.cwd.join(".claude/skills/codemod/SKILL.md");
        fs::write(&skill_path, "# changed\n").unwrap();

        assert!(mcs_install_requires_force_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &runtime_paths
        )
        .unwrap());
    }

    #[test]
    fn package_skill_install_requires_force_detects_conflicting_authored_content() {
        let (runtime_paths, temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let source_dir = create_authored_skill_source(temp_dir.path(), "jest-to-vitest");
        let package_skill = SkillPackageInstallSpec {
            id: "jest-to-vitest".to_string(),
            version: "0.1.0".to_string(),
            description: "Migrate Jest test suites to Vitest.".to_string(),
            source_dir: source_dir.clone(),
        };

        assert!(!package_skill_install_requires_force_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &package_skill,
            &runtime_paths
        )
        .unwrap());

        install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &package_skill,
            &install_request,
            &runtime_paths,
        )
        .unwrap();

        assert!(!package_skill_install_requires_force_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &package_skill,
            &runtime_paths
        )
        .unwrap());

        fs::write(source_dir.join("references/new-notes.md"), "# New notes\n").unwrap();

        assert!(package_skill_install_requires_force_with_runtime(
            Harness::Claude,
            InstallScope::Project,
            &package_skill,
            &runtime_paths
        )
        .unwrap());
    }

    #[test]
    fn package_skill_directory_name_sanitizes_scoped_ids() {
        let scoped_name = skill_directory_name_for_package_id("@codemod/jest-to-vitest");
        assert_eq!(scoped_name, "codemod__jest-to-vitest");
    }

    #[test]
    fn package_skill_error_codes_and_exit_codes_match_contract() {
        let not_found = HarnessAdapterError::SkillPackageNotFound("missing-package".to_string());
        assert_eq!(not_found.code(), "E_SKILL_PACKAGE_NOT_FOUND");
        assert_eq!(not_found.exit_code(), 27);

        let install_failed =
            HarnessAdapterError::SkillPackageInstallFailed("permission denied".to_string());
        assert_eq!(install_failed.code(), "E_SKILL_PACKAGE_INSTALL_FAILED");
        assert_eq!(install_failed.exit_code(), 28);
    }

    #[test]
    fn validate_skill_package_install_spec_accepts_authored_skill_bundle() {
        let temp_dir = tempdir().unwrap();
        let source_dir = create_authored_skill_source(temp_dir.path(), "@codemod/jest-to-vitest");
        let package_skill = SkillPackageInstallSpec {
            id: "@codemod/jest-to-vitest".to_string(),
            version: "0.1.0".to_string(),
            description: "Migrate tests: Jest -> Vitest".to_string(),
            source_dir,
        };

        let validation = validate_skill_package_install_spec(&package_skill);
        assert!(validation.is_ok());
    }

    #[test]
    fn resolve_adapter_auto_prefers_workspace_signals() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        fs::create_dir_all(runtime_paths.cwd.join(".goose")).unwrap();

        let resolved = resolve_adapter_with_runtime(Harness::Auto, &runtime_paths).unwrap();
        assert_eq!(resolved.harness, Harness::Goose);
        assert!(resolved.warnings.is_empty());
    }

    #[test]
    fn list_skills_returns_project_and_user_entries() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let project_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let user_request = InstallRequest {
            scope: InstallScope::User,
            force: false,
        };

        install_mcs_skill_bundle_with_runtime(Harness::Claude, &project_request, &runtime_paths)
            .unwrap();
        install_mcs_skill_bundle_with_runtime(Harness::Claude, &user_request, &runtime_paths)
            .unwrap();

        let skills = list_skills_with_runtime(Harness::Claude, &runtime_paths).unwrap();
        assert_eq!(skills.len(), 2);
        assert!(skills
            .iter()
            .any(|skill| skill.scope == Some(InstallScope::Project)));
        assert!(skills
            .iter()
            .any(|skill| skill.scope == Some(InstallScope::User)));
    }

    #[test]
    fn verify_skills_passes_for_installed_mcs_bundle() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        install_mcs_skill_bundle_with_runtime(Harness::Claude, &install_request, &runtime_paths)
            .unwrap();

        let checks = verify_skills_with_runtime(Harness::Claude, &runtime_paths).unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, VerificationStatus::Pass);
    }

    #[test]
    fn verify_skills_passes_for_installed_mcs_bundle_on_all_harnesses() {
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        for harness in ALL_HARNESSES {
            let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
            install_mcs_skill_bundle_with_runtime(harness, &install_request, &runtime_paths)
                .unwrap();

            let checks = verify_skills_with_runtime(harness, &runtime_paths).unwrap();
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0].status, VerificationStatus::Pass);
        }
    }

    #[test]
    fn verify_skills_fails_for_missing_compatibility_marker() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        let installed = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        let skill_path = &installed[0].path;

        let original_content = fs::read_to_string(skill_path).unwrap();
        let updated_content = original_content.replace(MCS_COMPATIBILITY_MARKER, "");
        fs::write(skill_path, updated_content).unwrap();

        let checks = verify_skills_with_runtime(Harness::Claude, &runtime_paths).unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, VerificationStatus::Fail);
        assert_eq!(
            checks[0].reason,
            Some("missing compatibility marker".to_string())
        );
    }

    #[test]
    fn list_skills_includes_package_skill_with_non_codemod_name_when_marked() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let package_skill_path = runtime_paths
            .cwd
            .join(".claude")
            .join("skills")
            .join("jest-to-vitest")
            .join("SKILL.md");
        let package_skill_content = r#"---
name: jest-to-vitest
description: Migrate Jest tests to Vitest
allowed-tools:
  - Bash(node *)
---
codemod-compatibility: skill-package-v1
codemod-skill-version: 0.1.0
"#;

        write_skill_file(&package_skill_path, package_skill_content, false).unwrap();

        let skills = list_skills_with_runtime(Harness::Claude, &runtime_paths).unwrap();
        assert!(skills.iter().any(|skill| skill.name == "jest-to-vitest"));
    }

    #[test]
    fn list_skills_excludes_non_codemod_skill_without_markers() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let unrelated_skill_path = runtime_paths
            .cwd
            .join(".claude")
            .join("skills")
            .join("general-helper")
            .join("SKILL.md");
        let unrelated_skill_content = r#"---
name: general-helper
description: Generic helper
allowed-tools:
  - Bash(echo *)
---
"#;

        write_skill_file(&unrelated_skill_path, unrelated_skill_content, false).unwrap();

        let skills = list_skills_with_runtime(Harness::Claude, &runtime_paths).unwrap();
        assert!(!skills.iter().any(|skill| skill.name == "general-helper"));
    }

    #[test]
    fn verify_skills_passes_for_package_skill_profile_with_non_mcs_allowed_tool() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let package_skill_path = runtime_paths
            .cwd
            .join(".claude")
            .join("skills")
            .join("jest-to-vitest")
            .join("SKILL.md");
        let package_skill_content = r#"---
name: jest-to-vitest
description: Migrate Jest tests to Vitest
allowed-tools:
  - Bash(node *)
---
codemod-compatibility: skill-package-v1
codemod-skill-version: 0.1.0
"#
        .to_string();

        write_skill_file(&package_skill_path, &package_skill_content, false).unwrap();

        let checks = verify_skills_with_runtime(Harness::Claude, &runtime_paths).unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, VerificationStatus::Pass);
    }

    #[test]
    fn verify_skills_enforces_safe_allowed_tools_for_mcs_profile() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        let installed = install_mcs_skill_bundle_with_runtime(
            Harness::Claude,
            &install_request,
            &runtime_paths,
        )
        .unwrap();
        let skill_path = &installed[0].path;
        let original_content = fs::read_to_string(skill_path).unwrap();
        let updated_content = original_content.replace("Bash(codemod *)", "Bash(node *)");
        fs::write(skill_path, updated_content).unwrap();

        let checks = verify_skills_with_runtime(Harness::Claude, &runtime_paths).unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, VerificationStatus::Fail);
        assert!(checks[0]
            .reason
            .as_ref()
            .unwrap()
            .contains("unknown or unsafe allowed-tools entry"));
    }

    #[test]
    fn install_package_skill_bundle_rejects_invalid_package_inputs() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };
        let invalid_package_skill = SkillPackageInstallSpec {
            id: " ".to_string(),
            version: "0.1.0".to_string(),
            description: "Migrate Jest test suites to Vitest.".to_string(),
            source_dir: PathBuf::from("/tmp/missing-skill-source"),
        };

        let install_result = install_package_skill_bundle_with_runtime(
            Harness::Claude,
            &invalid_package_skill,
            &install_request,
            &runtime_paths,
        );
        assert!(matches!(
            install_result,
            Err(HarnessAdapterError::SkillPackageInstallFailed(message)) if message.contains("cannot be empty")
        ));
    }

    #[test]
    fn upsert_mcp_server_creates_expected_config() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let config_path = runtime_paths.cwd.join(".mcp.json");

        upsert_codemod_mcp_server(Harness::Claude, &config_path, false, &runtime_paths).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed
                .get("mcpServers")
                .and_then(|servers| servers.get("codemod"))
                .and_then(|server| server.get("command"))
                .and_then(|value| value.as_str()),
            runtime_paths
                .current_executable
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .as_deref()
        );
        assert_eq!(
            parsed
                .get("mcpServers")
                .and_then(|servers| servers.get("codemod"))
                .and_then(|server| server.get("args"))
                .and_then(|args| args.as_array())
                .and_then(|args| args.last())
                .and_then(|value| value.as_str()),
            Some("mcp")
        );
    }

    #[test]
    fn upsert_mcp_server_preserves_existing_non_codemod_entries() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let config_path = runtime_paths.cwd.join(".mcp.json");
        let existing = serde_json::json!({
            "mcpServers": {
                "custom": {
                    "command": "node",
                    "args": ["custom-server.js"]
                }
            }
        });
        fs::write(
            &config_path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        upsert_codemod_mcp_server(Harness::Claude, &config_path, false, &runtime_paths).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed
            .get("mcpServers")
            .and_then(|servers| servers.get("custom"))
            .is_some());
        assert!(parsed
            .get("mcpServers")
            .and_then(|servers| servers.get("codemod"))
            .is_some());
    }

    #[test]
    fn upsert_mcp_server_requires_force_for_conflicting_codemod_entry() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let config_path = runtime_paths.cwd.join(".mcp.json");
        let existing = serde_json::json!({
            "mcpServers": {
                "codemod": {
                    "command": "node",
                    "args": ["local-mcp.js"]
                }
            }
        });
        fs::write(
            &config_path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let update_result =
            upsert_codemod_mcp_server(Harness::Claude, &config_path, false, &runtime_paths);
        assert!(matches!(
            update_result,
            Err(HarnessAdapterError::InstallFailed(message))
                if message.contains("already exists") && message.contains("--force")
        ));
    }

    #[test]
    fn upsert_mcp_server_force_overwrites_conflicting_codemod_entry() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let config_path = runtime_paths.cwd.join(".mcp.json");
        let existing = serde_json::json!({
            "mcpServers": {
                "codemod": {
                    "command": "node",
                    "args": ["local-mcp.js"]
                }
            }
        });
        fs::write(
            &config_path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        upsert_codemod_mcp_server(Harness::Claude, &config_path, true, &runtime_paths).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed
                .get("mcpServers")
                .and_then(|servers| servers.get("codemod"))
                .and_then(|server| server.get("command"))
                .and_then(|value| value.as_str()),
            runtime_paths
                .current_executable
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .as_deref()
        );
        assert_eq!(
            parsed
                .get("mcpServers")
                .and_then(|servers| servers.get("codemod"))
                .and_then(|server| server.get("args"))
                .and_then(|args| args.as_array())
                .and_then(|args| args.last())
                .and_then(|value| value.as_str()),
            Some("mcp")
        );
    }

    #[test]
    fn upsert_mcp_server_writes_codex_toml_entry() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let config_path = runtime_paths.cwd.join(".codex/config.toml");

        upsert_codemod_mcp_server(Harness::Codex, &config_path, false, &runtime_paths).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        let document = content.parse::<DocumentMut>().unwrap();
        let server = read_codex_mcp_server(&document)
            .unwrap()
            .expect("expected codex MCP server entry");
        assert_eq!(
            server.command,
            runtime_paths
                .current_executable
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap()
        );
        assert_eq!(server.args.last().map(String::as_str), Some("mcp"));
    }

    #[test]
    fn install_mcs_skill_bundle_skips_mcp_for_antigravity() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let install_request = InstallRequest {
            scope: InstallScope::Project,
            force: false,
        };

        let installed = install_mcs_skill_bundle_with_runtime(
            Harness::Antigravity,
            &install_request,
            &runtime_paths,
        )
        .unwrap();

        assert!(installed.iter().all(|entry| entry.name != "codemod-mcp"));
        assert_eq!(installed.len(), 1);
        assert!(installed[0]
            .path
            .ends_with(".agents/skills/codemod/SKILL.md"));
    }

    #[test]
    fn mcp_config_path_supports_all_harnesses_for_project_and_user_scope() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        for harness in MCP_CAPABLE_HARNESSES {
            let project_path =
                mcp_config_path_for_harness(harness, InstallScope::Project, &runtime_paths)
                    .unwrap();
            let user_path =
                mcp_config_path_for_harness(harness, InstallScope::User, &runtime_paths).unwrap();
            assert!(project_path.starts_with(&runtime_paths.cwd));
            assert!(user_path.starts_with(runtime_paths.home_dir.as_ref().unwrap()));
        }
    }

    #[test]
    fn skills_root_for_harness_supports_all_concrete_harnesses() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();

        let project_opencode =
            skills_root_for_harness(Harness::Opencode, InstallScope::Project, &runtime_paths)
                .unwrap();
        assert!(project_opencode.ends_with(".opencode/skills"));

        let project_cursor =
            skills_root_for_harness(Harness::Cursor, InstallScope::Project, &runtime_paths)
                .unwrap();
        assert!(project_cursor.ends_with(".cursor/skills"));

        let user_opencode =
            skills_root_for_harness(Harness::Opencode, InstallScope::User, &runtime_paths).unwrap();
        assert!(user_opencode.ends_with(".opencode/skills"));

        let user_cursor =
            skills_root_for_harness(Harness::Cursor, InstallScope::User, &runtime_paths).unwrap();
        assert!(user_cursor.ends_with(".cursor/skills"));

        let project_codex =
            skills_root_for_harness(Harness::Codex, InstallScope::Project, &runtime_paths).unwrap();
        assert!(project_codex.ends_with(".agents/skills"));

        let user_codex =
            skills_root_for_harness(Harness::Codex, InstallScope::User, &runtime_paths).unwrap();
        assert!(user_codex.ends_with(".agents/skills"));

        let project_antigravity =
            skills_root_for_harness(Harness::Antigravity, InstallScope::Project, &runtime_paths)
                .unwrap();
        assert!(project_antigravity.ends_with(".agents/skills"));

        let user_antigravity =
            skills_root_for_harness(Harness::Antigravity, InstallScope::User, &runtime_paths)
                .unwrap();
        assert!(user_antigravity.ends_with(".gemini/antigravity/skills"));
    }
}
