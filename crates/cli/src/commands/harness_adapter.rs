use clap::ValueEnum;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MCS_SKILL_NAME: &str = "codemod-cli";
const MCS_SKILL_VERSION: &str = "1.0.0";
const CODEMOD_COMPATIBILITY_MARKER_PREFIX: &str = "codemod-compatibility:";
const MCS_COMPATIBILITY_MARKER: &str = "codemod-compatibility: mcs-v1";
const MCS_VERSION_MARKER: &str = "codemod-skill-version: 1.0.0";
const CODEMOD_VERSION_MARKER_PREFIX: &str = "codemod-skill-version:";
const MCS_REFERENCE_INDEX_RELATIVE_PATH: &str = "references/index.md";
const MCS_AI_NATIVE_RECIPES_RELATIVE_PATH: &str = "references/cli-ai-native-recipes.md";
const MCS_SEARCH_DISCOVERY_RELATIVE_PATH: &str = "references/cli-core-search-and-discovery.md";
const MCS_SCAFFOLD_RUN_RELATIVE_PATH: &str = "references/cli-core-scaffold-and-run.md";
const MCS_DRY_RUN_VERIFY_RELATIVE_PATH: &str = "references/cli-core-dry-run-and-verify.md";
const MCS_TROUBLESHOOTING_RELATIVE_PATH: &str = "references/cli-core-troubleshooting.md";
const MCS_SKILL_MD: &str = include_str!("../templates/ai-native-cli/codemod-cli/SKILL.md");
const MCS_REFERENCE_INDEX_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/index.md");
const MCS_AI_NATIVE_RECIPES_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/cli-ai-native-recipes.md");
const MCS_SEARCH_DISCOVERY_MD: &str = include_str!(
    "../templates/ai-native-cli/codemod-cli/references/cli-core-search-and-discovery.md"
);
const MCS_SCAFFOLD_RUN_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/cli-core-scaffold-and-run.md");
const MCS_DRY_RUN_VERIFY_MD: &str = include_str!(
    "../templates/ai-native-cli/codemod-cli/references/cli-core-dry-run-and-verify.md"
);
const MCS_TROUBLESHOOTING_MD: &str =
    include_str!("../templates/ai-native-cli/codemod-cli/references/cli-core-troubleshooting.md");
const MCS_REFERENCE_FILES: [(&str, &str); 6] = [
    (MCS_REFERENCE_INDEX_RELATIVE_PATH, MCS_REFERENCE_INDEX_MD),
    (
        MCS_AI_NATIVE_RECIPES_RELATIVE_PATH,
        MCS_AI_NATIVE_RECIPES_MD,
    ),
    (MCS_SEARCH_DISCOVERY_RELATIVE_PATH, MCS_SEARCH_DISCOVERY_MD),
    (MCS_SCAFFOLD_RUN_RELATIVE_PATH, MCS_SCAFFOLD_RUN_MD),
    (MCS_DRY_RUN_VERIFY_RELATIVE_PATH, MCS_DRY_RUN_VERIFY_MD),
    (MCS_TROUBLESHOOTING_RELATIVE_PATH, MCS_TROUBLESHOOTING_MD),
];
const MCS_INDEX_LINKED_REFERENCE_PATHS: [&str; 5] = [
    MCS_AI_NATIVE_RECIPES_RELATIVE_PATH,
    MCS_SEARCH_DISCOVERY_RELATIVE_PATH,
    MCS_SCAFFOLD_RUN_RELATIVE_PATH,
    MCS_DRY_RUN_VERIFY_RELATIVE_PATH,
    MCS_TROUBLESHOOTING_RELATIVE_PATH,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Harness {
    #[default]
    Auto,
    Claude,
    Goose,
    Opencode,
    Cursor,
}

impl Harness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Claude => "claude",
            Self::Goose => "goose",
            Self::Opencode => "opencode",
            Self::Cursor => "cursor",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
    Yaml,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatibilityMetadata {
    pub harness: Harness,
    pub supports_project_scope: bool,
    pub supports_user_scope: bool,
    pub supports_verify: bool,
}

#[derive(Debug, Error)]
pub enum HarnessAdapterError {
    #[error("Unsupported harness: {0}")]
    UnsupportedHarness(String),
    #[error("Invalid skill package: {0}")]
    InvalidSkillPackage(String),
    #[error("Skill install failed: {0}")]
    InstallFailed(String),
}

impl HarnessAdapterError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedHarness(_) => "E_UNSUPPORTED_HARNESS",
            Self::InvalidSkillPackage(_) => "E_SKILL_INVALID",
            Self::InstallFailed(_) => "E_SKILL_INSTALL_FAILED",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::UnsupportedHarness(_) => 20,
            Self::InvalidSkillPackage(_) => 21,
            Self::InstallFailed(_) => 22,
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Self::UnsupportedHarness(_) => {
                "Use --harness claude, --harness goose, --harness opencode, or --harness cursor."
            }
            Self::InvalidSkillPackage(_) => {
                "Run `codemod agent verify-skills --format json` after reinstalling skills."
            }
            Self::InstallFailed(_) => "Retry with --force or check filesystem permissions.",
        }
    }
}

pub type AdapterResult<T> = std::result::Result<T, HarnessAdapterError>;

pub trait HarnessAdapter: Send + Sync {
    fn metadata(&self) -> CompatibilityMetadata;
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
struct RuntimePaths {
    cwd: PathBuf,
    home_dir: Option<PathBuf>,
}

impl RuntimePaths {
    fn current() -> AdapterResult<Self> {
        let cwd = std::env::current_dir().map_err(|error| {
            HarnessAdapterError::InstallFailed(format!(
                "Unable to read current working directory: {error}"
            ))
        })?;

        Ok(Self {
            cwd,
            home_dir: dirs::home_dir(),
        })
    }
}

#[derive(Debug, Default)]
pub struct ClaudeHarnessAdapter;

impl HarnessAdapter for ClaudeHarnessAdapter {
    fn metadata(&self) -> CompatibilityMetadata {
        CompatibilityMetadata {
            harness: Harness::Claude,
            supports_project_scope: true,
            supports_user_scope: true,
            supports_verify: true,
        }
    }

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
    fn metadata(&self) -> CompatibilityMetadata {
        CompatibilityMetadata {
            harness: Harness::Goose,
            supports_project_scope: true,
            supports_user_scope: true,
            supports_verify: true,
        }
    }

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
    fn metadata(&self) -> CompatibilityMetadata {
        CompatibilityMetadata {
            harness: Harness::Opencode,
            supports_project_scope: true,
            supports_user_scope: true,
            supports_verify: true,
        }
    }

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
    fn metadata(&self) -> CompatibilityMetadata {
        CompatibilityMetadata {
            harness: Harness::Cursor,
            supports_project_scope: true,
            supports_user_scope: true,
            supports_verify: true,
        }
    }

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

pub fn resolve_adapter(harness: Harness) -> AdapterResult<ResolvedAdapter> {
    let runtime_paths = RuntimePaths::current()?;
    resolve_adapter_with_runtime(harness, &runtime_paths)
}

fn resolve_adapter_with_runtime(
    harness: Harness,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<ResolvedAdapter> {
    let (resolved_harness, warnings) = match harness {
        Harness::Auto => detect_auto_harness(&runtime_paths.cwd),
        Harness::Claude => (Harness::Claude, Vec::new()),
        Harness::Goose => (Harness::Goose, Vec::new()),
        Harness::Opencode => (Harness::Opencode, Vec::new()),
        Harness::Cursor => (Harness::Cursor, Vec::new()),
    };

    let adapter: Box<dyn HarnessAdapter> = match resolved_harness {
        Harness::Claude => Box::new(ClaudeHarnessAdapter),
        Harness::Goose => Box::new(GooseHarnessAdapter),
        Harness::Opencode => Box::new(OpencodeHarnessAdapter),
        Harness::Cursor => Box::new(CursorHarnessAdapter),
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
    ] {
        if cwd.join(root_dir).exists() {
            return (harness, Vec::new());
        }
    }

    (
        Harness::Claude,
        vec![
            "No .claude, .goose, .opencode, or .cursor directory found; defaulting to Claude harness.".to_string(),
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

fn install_mcs_skill_bundle_with_runtime(
    harness: Harness,
    request: &InstallRequest,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<Vec<InstalledSkill>> {
    validate_embedded_mcs_bundle()?;

    let skill_root =
        skills_root_for_harness(harness, request.scope, runtime_paths)?.join(MCS_SKILL_NAME);
    let skill_md_path = skill_root.join("SKILL.md");

    write_skill_file(&skill_md_path, MCS_SKILL_MD, request.force)?;
    for (relative_path, content) in MCS_REFERENCE_FILES {
        write_skill_file(&skill_root.join(relative_path), content, request.force)?;
    }

    Ok(vec![InstalledSkill {
        name: MCS_SKILL_NAME.to_string(),
        path: skill_md_path,
        version: Some(MCS_SKILL_VERSION.to_string()),
        scope: Some(request.scope),
    }])
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

    for key in ["name:", "description:", "allowed-tools:"] {
        if !frontmatter.contains(key) {
            return VerificationCheck {
                skill: skill.name.clone(),
                scope,
                status: VerificationStatus::Fail,
                reason: Some(format!("missing required frontmatter key: {key}")),
            };
        }
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
    Tcs,
    Unknown,
}

fn detect_skill_validation_profile(content: &str) -> SkillValidationProfile {
    if content.contains(MCS_COMPATIBILITY_MARKER) {
        return SkillValidationProfile::Mcs;
    }

    if content.contains(CODEMOD_COMPATIBILITY_MARKER_PREFIX) {
        return SkillValidationProfile::Tcs;
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
    if !MCS_SKILL_MD.starts_with("---") {
        return Err(HarnessAdapterError::InvalidSkillPackage(
            "SKILL.md is missing YAML frontmatter".to_string(),
        ));
    }

    for required_key in ["name:", "description:", "allowed-tools:"] {
        if !MCS_SKILL_MD.contains(required_key) {
            return Err(HarnessAdapterError::InvalidSkillPackage(format!(
                "SKILL.md is missing required frontmatter key: {required_key}"
            )));
        }
    }

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
    let harness_dir = harness_hidden_dir(harness)?;
    match scope {
        InstallScope::Project => Ok(runtime_paths.cwd.join(harness_dir).join("skills")),
        InstallScope::User => runtime_paths
            .home_dir
            .as_ref()
            .map(|home| home.join(harness_dir).join("skills"))
            .ok_or_else(|| {
                HarnessAdapterError::InstallFailed(
                    "Could not determine home directory for --user install".to_string(),
                )
            }),
    }
}

fn harness_hidden_dir(harness: Harness) -> AdapterResult<&'static str> {
    match harness {
        Harness::Claude => Ok(".claude"),
        Harness::Goose => Ok(".goose"),
        Harness::Opencode => Ok(".opencode"),
        Harness::Cursor => Ok(".cursor"),
        Harness::Auto => Err(HarnessAdapterError::UnsupportedHarness("auto".to_string())),
    }
}

fn write_skill_file(path: &Path, content: &str, force: bool) -> AdapterResult<()> {
    if path.exists() && !force {
        return Err(HarnessAdapterError::InstallFailed(format!(
            "Skill file already exists at {}. Re-run with --force to overwrite.",
            path.display()
        )));
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn runtime_paths_with_temp_roots() -> (RuntimePaths, tempfile::TempDir) {
        let temp_dir = tempdir().unwrap();
        let runtime_paths = RuntimePaths {
            cwd: temp_dir.path().join("workspace"),
            home_dir: Some(temp_dir.path().join("home")),
        };
        std::fs::create_dir_all(&runtime_paths.cwd).unwrap();
        std::fs::create_dir_all(runtime_paths.home_dir.as_ref().unwrap()).unwrap();
        (runtime_paths, temp_dir)
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

        assert_eq!(installed_skill.name, MCS_SKILL_NAME);
        assert!(installed_skill.path.exists());
        assert!(installed_skill
            .path
            .to_string_lossy()
            .contains(".claude/skills/codemod-cli/SKILL.md"));

        let skill_root = runtime_paths
            .cwd
            .join(".claude")
            .join("skills")
            .join("codemod-cli");

        for (relative_path, _) in MCS_REFERENCE_FILES {
            assert!(
                skill_root.join(relative_path).exists(),
                "expected installed file to exist: {}",
                relative_path
            );
        }
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
    fn install_mcs_skill_bundle_rejects_overwrite_without_force() {
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
        assert!(matches!(
            second_install,
            Err(HarnessAdapterError::InstallFailed(message)) if message.contains("--force")
        ));
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
    fn list_skills_includes_tcs_with_non_codemod_name_when_marked() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let tcs_skill_path = runtime_paths
            .cwd
            .join(".claude")
            .join("skills")
            .join("jest-to-vitest")
            .join("SKILL.md");
        let tcs_skill_content = r#"---
name: jest-to-vitest
description: Migrate Jest tests to Vitest
allowed-tools:
  - Bash(node *)
---
codemod-compatibility: tcs-v1
codemod-skill-version: 0.1.0
"#;

        write_skill_file(&tcs_skill_path, tcs_skill_content, false).unwrap();

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
    fn verify_skills_passes_for_tcs_profile_with_non_mcs_allowed_tool() {
        let (runtime_paths, _temp_dir) = runtime_paths_with_temp_roots();
        let tcs_skill_path = runtime_paths
            .cwd
            .join(".claude")
            .join("skills")
            .join("jest-to-vitest")
            .join("SKILL.md");
        let tcs_skill_content = r#"---
name: jest-to-vitest
description: Migrate Jest tests to Vitest
allowed-tools:
  - Bash(node *)
---
codemod-compatibility: tcs-v1
codemod-skill-version: 0.1.0
"#
        .to_string();

        write_skill_file(&tcs_skill_path, &tcs_skill_content, false).unwrap();

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
    }
}
