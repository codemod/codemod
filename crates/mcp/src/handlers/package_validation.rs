use anyhow::{Context, Result};
use butterflow_core::utils::{parse_workflow_file, validate_workflow};
use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool, ErrorData as McpError};
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use walkdir::WalkDir;

const DEFAULT_PACKAGE_VALIDATION_TIMEOUT_SECS: u64 = 120;
const STARTER_TRANSFORM_MARKER: &str = "pattern: \"var $VAR = $VALUE\"";
const STARTER_README_MARKERS: [&str; 3] = [
    "Converting `var` declarations to `const`/`let`",
    "Modernizing syntax patterns",
    "This codemod transforms",
];
const STARTER_FIXTURE_INPUT_MARKER: &str = "var oldVariable = \"should be const\";";
const STARTER_FIXTURE_EXPECTED_MARKER: &str = "const oldVariable = \"should be const\";";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ValidateCodemodPackageRequest {
    /// Package directory to inspect. Defaults to the current working directory.
    #[serde(default)]
    pub package_path: Option<String>,
    /// Run the package's default `test` script if present.
    #[serde(default = "default_true")]
    pub run_default_test: bool,
    /// Run the package's `check-types` script if present.
    #[serde(default = "default_true")]
    pub run_check_types: bool,
    /// Timeout for spawned package commands, in seconds.
    #[serde(default = "default_timeout_seconds")]
    pub command_timeout_seconds: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_PACKAGE_VALIDATION_TIMEOUT_SECS
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct PackageFilePresence {
    pub codemod_yaml: bool,
    pub workflow_yaml: bool,
    pub package_json: bool,
    pub readme_md: bool,
    pub scripts_codemod_ts: bool,
    pub tests_dir: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct PackageScriptSummary {
    pub test: Option<String>,
    pub check_types: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct TestCaseSummary {
    pub total_case_dirs: usize,
    pub positive_case_dirs: usize,
    pub negative_case_dirs: usize,
    pub edge_case_dirs: usize,
    pub starter_fixtures_detected: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ProcessCheckResult {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ValidationIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub path: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ValidateCodemodPackageResponse {
    pub package_root: String,
    pub files: PackageFilePresence,
    pub scripts: PackageScriptSummary,
    pub workflow_valid: bool,
    pub test_cases: TestCaseSummary,
    pub starter_transform_detected: bool,
    pub generic_readme_detected: bool,
    pub risky_regex_transform_detected: bool,
    pub issues: Vec<ValidationIssue>,
    pub default_test: Option<ProcessCheckResult>,
    pub check_types: Option<ProcessCheckResult>,
    pub ready: bool,
}

#[derive(Debug, Default)]
struct PackageJsonLite {
    scripts: BTreeMap<String, String>,
    package_manager: Option<String>,
}

#[derive(Debug, Default)]
struct TransformRiskSummary {
    risky_regex_lines: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValidationPackageKind {
    Jssg,
    Hybrid,
    AstGrepYaml,
    Shell,
    SkillOnly,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ValidationRequirements {
    package_json: bool,
    scripts_codemod_ts: bool,
    tests_dir: bool,
}

#[derive(Debug)]
struct WorkflowPathResolution {
    path: PathBuf,
    codemod_yaml_invalid: bool,
}

impl ValidationPackageKind {
    fn requirements(self) -> ValidationRequirements {
        match self {
            Self::Jssg | Self::Hybrid => ValidationRequirements {
                package_json: true,
                scripts_codemod_ts: true,
                tests_dir: true,
            },
            Self::AstGrepYaml => ValidationRequirements {
                package_json: false,
                scripts_codemod_ts: false,
                tests_dir: true,
            },
            Self::Shell | Self::SkillOnly | Self::Unknown => ValidationRequirements {
                package_json: false,
                scripts_codemod_ts: false,
                tests_dir: false,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageManager {
    Npm,
    Yarn,
    Pnpm,
    Bun,
}

impl PackageManager {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "npm" => Some(Self::Npm),
            "yarn" => Some(Self::Yarn),
            "pnpm" => Some(Self::Pnpm),
            "bun" => Some(Self::Bun),
            _ => None,
        }
    }

    fn from_package_root(package_root: &Path) -> Self {
        if package_root.join("pnpm-lock.yaml").exists() {
            Self::Pnpm
        } else if package_root.join("yarn.lock").exists() {
            Self::Yarn
        } else if package_root.join("bun.lockb").exists() || package_root.join("bun.lock").exists()
        {
            Self::Bun
        } else {
            Self::Npm
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Yarn => "yarn",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
        }
    }

    fn allowed_command_names(self) -> &'static [&'static str] {
        match self {
            Self::Npm => &["npm", "npx"],
            Self::Yarn => &["yarn"],
            Self::Pnpm => &["pnpm"],
            Self::Bun => &["bun", "bunx"],
        }
    }
}

enum PackageCommand<'a> {
    RunScript {
        manager: PackageManager,
        script: &'a str,
    },
}

impl PackageCommand<'_> {
    fn program(&self) -> &'static str {
        match self {
            Self::RunScript { manager, .. } => manager.binary(),
        }
    }

    fn apply(&self, command: &mut Command) {
        match self {
            Self::RunScript {
                manager: PackageManager::Yarn,
                script,
            } => {
                command.arg(script);
            }
            Self::RunScript { script, .. } => {
                command.args(["run", script]);
            }
        }
    }
}

impl std::fmt::Display for PackageCommand<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RunScript {
                manager: PackageManager::Yarn,
                script,
            } => write!(formatter, "yarn {script}"),
            Self::RunScript { manager, script } => {
                write!(formatter, "{} run {script}", manager.binary())
            }
        }
    }
}

#[derive(Clone)]
pub struct PackageValidationHandler;

impl PackageValidationHandler {
    pub fn new() -> Self {
        Self
    }

    #[tool(
        description = "Validate whether a codemod package is real and complete. Use this before stopping work on a codemod package. It checks starter-scaffold leftovers, workflow validity, test coverage, and optionally runs the package default tests and type-check script."
    )]
    pub async fn validate_codemod_package(
        &self,
        Parameters(request): Parameters<ValidateCodemodPackageRequest>,
    ) -> Result<CallToolResult, McpError> {
        let response = self
            .validate_package(request)
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;

        let content = serde_json::to_string_pretty(&response).map_err(|error| {
            McpError::internal_error(format!("Failed to serialize response: {error}"), None)
        })?;

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    async fn validate_package(
        &self,
        request: ValidateCodemodPackageRequest,
    ) -> Result<ValidateCodemodPackageResponse> {
        let package_root = canonicalize_package_root(request.package_path.as_deref())?;
        let workflow_resolution = workflow_path_for_package(&package_root);
        let workflow_path = workflow_resolution.path.clone();
        let files = collect_file_presence(&package_root, &workflow_path);
        let package_kind = infer_validation_package_kind(&package_root, &files);
        let workflow_valid = validate_workflow_at_path(&workflow_path).is_ok();
        let transform_path = package_root.join("scripts/codemod.ts");
        let readme_path = package_root.join("README.md");
        let tests_path = package_root.join("tests");

        let transform_content = read_file_if_exists(&transform_path);
        let readme_content = read_file_if_exists(&readme_path);
        let transform_risks = detect_transform_risks(transform_content.as_deref());

        let starter_transform_detected = transform_content
            .as_deref()
            .is_some_and(|content| content.contains(STARTER_TRANSFORM_MARKER));
        let generic_readme_detected = readme_content.as_deref().is_some_and(|content| {
            STARTER_README_MARKERS
                .iter()
                .any(|marker| content.contains(marker))
        });
        let test_cases = summarize_test_cases(&tests_path);
        let package_json = load_package_json(&package_root);
        let preferred_package_manager = preferred_package_manager(&package_root, &package_json);

        let scripts = PackageScriptSummary {
            test: package_json.scripts.get("test").cloned(),
            check_types: package_json.scripts.get("check-types").cloned(),
        };

        let default_test = if request.run_default_test && scripts.test.is_some() {
            Some(
                run_package_script(
                    &package_root,
                    preferred_package_manager,
                    "test",
                    request.command_timeout_seconds,
                )
                .await,
            )
        } else {
            None
        };

        let check_types = if request.run_check_types && scripts.check_types.is_some() {
            Some(
                run_package_script(
                    &package_root,
                    preferred_package_manager,
                    "check-types",
                    request.command_timeout_seconds,
                )
                .await,
            )
        } else {
            None
        };

        let mut issues = Vec::new();

        if workflow_resolution.codemod_yaml_invalid {
            issues.push(issue(
                "error",
                "codemod_yaml_invalid",
                "codemod.yaml failed to parse; falling back to workflow.yaml.",
                Some(package_root.join("codemod.yaml")),
            ));
        }

        push_missing_file_issues(
            &files,
            package_kind,
            &package_root,
            &workflow_path,
            &mut issues,
        );

        if !workflow_valid && files.workflow_yaml {
            issues.push(issue(
                "error",
                "workflow_invalid",
                "workflow.yaml failed schema or structural validation.",
                Some(workflow_path.clone()),
            ));
        }

        if starter_transform_detected {
            issues.push(issue(
                "error",
                "starter_transform_present",
                "scripts/codemod.ts still contains the starter transform scaffold.",
                Some(transform_path.clone()),
            ));
        }

        if generic_readme_detected {
            issues.push(issue(
                "error",
                "generic_readme_present",
                "README.md still contains starter or generic package text.",
                Some(readme_path.clone()),
            ));
        }

        if test_cases.starter_fixtures_detected {
            issues.push(issue(
                "error",
                "starter_fixtures_present",
                "tests still contain the default starter fixtures.",
                Some(tests_path.clone()),
            ));
        }

        if package_kind.requirements().tests_dir
            && test_cases.total_case_dirs == 0
            && files.tests_dir
        {
            issues.push(issue(
                "error",
                "missing_real_test_cases",
                "tests/ exists but does not contain any real input/expected fixture cases.",
                Some(tests_path.clone()),
            ));
        }

        if !transform_risks.risky_regex_lines.is_empty() {
            issues.push(issue(
                "error",
                "risky_regex_transform_detected",
                &format!(
                    "scripts/codemod.ts uses regex or string operations in likely raw-source transform logic at lines {}.",
                    format_line_list(&transform_risks.risky_regex_lines)
                ),
                Some(transform_path.clone()),
            ));
        }

        let package_manager_drift_scripts =
            detect_package_manager_drift_in_scripts(&package_json, preferred_package_manager);
        if !package_manager_drift_scripts.is_empty() {
            issues.push(issue(
                "error",
                "package_manager_drift_in_scripts",
                &format!(
                    "package.json scripts {} use commands from a different package manager than the scaffold-selected package manager (`{}`). Keep package-local install/run/test invocations consistent.",
                    format_script_list(&package_manager_drift_scripts),
                    preferred_package_manager.binary()
                ),
                Some(package_root.join("package.json")),
            ));
        }

        if let Some(result) = &default_test {
            if !result.success {
                issues.push(issue(
                    "error",
                    "default_test_failed",
                    "The package default test command failed.",
                    Some(package_root.clone()),
                ));
            }
        }

        if let Some(result) = &check_types {
            if !result.success {
                issues.push(issue(
                    "error",
                    "check_types_failed",
                    "The package check-types command failed.",
                    Some(package_root.clone()),
                ));
            }
        }

        let ready = issues.iter().all(|issue| issue.severity != "error");

        Ok(ValidateCodemodPackageResponse {
            package_root: package_root.display().to_string(),
            files,
            scripts,
            workflow_valid,
            test_cases,
            starter_transform_detected,
            generic_readme_detected,
            risky_regex_transform_detected: !transform_risks.risky_regex_lines.is_empty(),
            issues,
            default_test,
            check_types,
            ready,
        })
    }
}

fn canonicalize_package_root(path: Option<&str>) -> Result<PathBuf> {
    let root = path
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    Ok(root)
}

fn collect_file_presence(package_root: &Path, workflow_path: &Path) -> PackageFilePresence {
    PackageFilePresence {
        codemod_yaml: package_root.join("codemod.yaml").is_file(),
        workflow_yaml: workflow_path.is_file(),
        package_json: package_root.join("package.json").is_file(),
        readme_md: package_root.join("README.md").is_file(),
        scripts_codemod_ts: package_root.join("scripts/codemod.ts").is_file(),
        tests_dir: package_root.join("tests").is_dir(),
    }
}

fn infer_validation_package_kind(
    package_root: &Path,
    files: &PackageFilePresence,
) -> ValidationPackageKind {
    let has_rules = package_root.join("rules").is_dir();
    let has_shell_scripts = package_root.join("scripts/transform.sh").is_file();
    let has_authored_skill = package_root.join("agents/skill").is_dir();

    match (
        files.scripts_codemod_ts || files.package_json,
        has_rules,
        has_shell_scripts,
    ) {
        (true, true, _) => ValidationPackageKind::Hybrid,
        (true, false, _) => ValidationPackageKind::Jssg,
        (false, true, _) => ValidationPackageKind::AstGrepYaml,
        (false, false, true) => ValidationPackageKind::Shell,
        (false, false, false) if has_authored_skill => ValidationPackageKind::SkillOnly,
        _ => ValidationPackageKind::Unknown,
    }
}

fn workflow_path_for_package(package_root: &Path) -> WorkflowPathResolution {
    let manifest_path = package_root.join("codemod.yaml");
    let Some(content) = read_file_if_exists(&manifest_path) else {
        return WorkflowPathResolution {
            path: package_root.join("workflow.yaml"),
            codemod_yaml_invalid: false,
        };
    };

    let Ok(manifest) = serde_yaml::from_str::<YamlValue>(&content) else {
        return WorkflowPathResolution {
            path: package_root.join("workflow.yaml"),
            codemod_yaml_invalid: true,
        };
    };
    let workflow = manifest
        .get("workflow")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("workflow.yaml");
    WorkflowPathResolution {
        path: package_root.join(workflow),
        codemod_yaml_invalid: false,
    }
}

fn validate_workflow_at_path(workflow_path: &Path) -> Result<()> {
    let workflow = parse_workflow_file(workflow_path)
        .with_context(|| format!("Failed to parse workflow {}", workflow_path.display()))?;
    let parent_dir = workflow_path
        .parent()
        .context("Workflow path has no parent directory")?;
    validate_workflow(&workflow, parent_dir)
        .with_context(|| format!("Workflow validation failed for {}", workflow_path.display()))
}

fn read_file_if_exists(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn summarize_test_cases(tests_path: &Path) -> TestCaseSummary {
    if !tests_path.is_dir() {
        return TestCaseSummary {
            total_case_dirs: 0,
            positive_case_dirs: 0,
            negative_case_dirs: 0,
            edge_case_dirs: 0,
            starter_fixtures_detected: false,
        };
    }

    let mut total_case_dirs = 0;
    let mut positive_case_dirs = 0;
    let mut negative_case_dirs = 0;
    let mut edge_case_dirs = 0;
    let mut starter_fixtures_detected = false;

    for entry in WalkDir::new(tests_path)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_dir())
    {
        let dir = entry.path();
        let dir_name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let has_single_file_fixture = contains_named_fixture_pair(dir);
        let has_directory_snapshot = dir.join("input").is_dir() && dir.join("expected").is_dir();

        if has_single_file_fixture || has_directory_snapshot {
            total_case_dirs += 1;
            if dir_name.contains("positive") {
                positive_case_dirs += 1;
            }
            if dir_name.contains("negative")
                || dir_name.contains("preserve")
                || dir_name.contains("noop")
                || dir_name.contains("no-op")
            {
                negative_case_dirs += 1;
            }
            if dir_name.contains("edge") {
                edge_case_dirs += 1;
            }
        }

        if dir_name == "fixtures" {
            let input = read_file_if_exists(&dir.join("input.js"));
            let expected = read_file_if_exists(&dir.join("expected.js"));
            if input
                .as_deref()
                .is_some_and(|content| content.contains(STARTER_FIXTURE_INPUT_MARKER))
                && expected
                    .as_deref()
                    .is_some_and(|content| content.contains(STARTER_FIXTURE_EXPECTED_MARKER))
            {
                starter_fixtures_detected = true;
            }
        }
    }

    TestCaseSummary {
        total_case_dirs,
        positive_case_dirs,
        negative_case_dirs,
        edge_case_dirs,
        starter_fixtures_detected,
    }
}

fn contains_named_fixture_pair(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };

    let mut has_input = false;
    let mut has_expected = false;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with("input.") {
            has_input = true;
        }
        if file_name.starts_with("expected.") {
            has_expected = true;
        }
    }

    has_input && has_expected
}

fn load_package_json(package_root: &Path) -> PackageJsonLite {
    let package_json_path = package_root.join("package.json");
    let Some(content) = read_file_if_exists(&package_json_path) else {
        return PackageJsonLite::default();
    };

    let parsed = serde_json::from_str::<serde_json::Value>(&content).ok();
    let scripts = parsed
        .as_ref()
        .and_then(|value| value.get("scripts"))
        .and_then(|value| value.as_object())
        .map(|scripts| {
            scripts
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let package_manager = parsed
        .as_ref()
        .and_then(|value| value.get("packageManager"))
        .and_then(|value| value.as_str())
        .map(parse_package_manager_name)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    PackageJsonLite {
        scripts,
        package_manager,
    }
}

fn push_missing_file_issues(
    files: &PackageFilePresence,
    package_kind: ValidationPackageKind,
    package_root: &Path,
    workflow_path: &Path,
    issues: &mut Vec<ValidationIssue>,
) {
    let requirements = package_kind.requirements();
    let checks = [
        (
            files.codemod_yaml,
            "missing_codemod_yaml",
            "codemod.yaml is missing.",
            package_root.join("codemod.yaml"),
        ),
        (
            files.workflow_yaml,
            "missing_workflow_yaml",
            "workflow.yaml is missing.",
            workflow_path.to_path_buf(),
        ),
        (
            !requirements.package_json || files.package_json,
            "missing_package_json",
            "package.json is missing.",
            package_root.join("package.json"),
        ),
        (
            files.readme_md,
            "missing_readme",
            "README.md is missing.",
            package_root.join("README.md"),
        ),
        (
            !requirements.scripts_codemod_ts || files.scripts_codemod_ts,
            "missing_transform_script",
            "scripts/codemod.ts is missing.",
            package_root.join("scripts/codemod.ts"),
        ),
        (
            !requirements.tests_dir || files.tests_dir,
            "missing_tests_dir",
            "tests/ directory is missing.",
            package_root.join("tests"),
        ),
    ];

    for (present, code, message, path) in checks {
        if !present {
            issues.push(issue("error", code, message, Some(path)));
        }
    }
}

fn issue(severity: &str, code: &str, message: &str, path: Option<PathBuf>) -> ValidationIssue {
    ValidationIssue {
        severity: severity.to_string(),
        code: code.to_string(),
        message: message.to_string(),
        path: path.map(|path| path.display().to_string()),
    }
}

fn format_line_list(lines: &[usize]) -> String {
    lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_script_list(scripts: &[String]) -> String {
    match scripts {
        [] => String::new(),
        [only] => format!("(`{only}`)"),
        _ => format!(
            "({})",
            scripts
                .iter()
                .map(|script| format!("`{script}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn parse_package_manager_name(value: &str) -> &str {
    value.split('@').next().unwrap_or(value).trim()
}

fn preferred_package_manager(
    package_root: &Path,
    package_json: &PackageJsonLite,
) -> PackageManager {
    if let Some(package_manager) = package_json.package_manager.as_deref() {
        return PackageManager::from_name(package_manager).unwrap_or(PackageManager::Npm);
    }

    infer_package_manager(package_root)
}

fn detect_package_manager_drift_in_scripts(
    package_json: &PackageJsonLite,
    preferred_package_manager: PackageManager,
) -> Vec<String> {
    package_json
        .scripts
        .iter()
        .filter_map(|(name, command)| {
            package_manager_commands_in_script(command)
                .iter()
                .any(|command_name| {
                    !preferred_package_manager
                        .allowed_command_names()
                        .contains(&command_name.as_str())
                })
                .then_some(name.clone())
        })
        .collect()
}

fn package_manager_commands_in_script(command: &str) -> Vec<String> {
    const PACKAGE_MANAGER_COMMANDS: [&str; 6] = ["npm", "npx", "yarn", "pnpm", "bun", "bunx"];

    command
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .map(str::to_ascii_lowercase)
        .filter(|token| PACKAGE_MANAGER_COMMANDS.contains(&token.as_str()))
        .collect()
}

fn detect_transform_risks(transform_content: Option<&str>) -> TransformRiskSummary {
    let mut summary = TransformRiskSummary::default();
    let Some(content) = transform_content else {
        return summary;
    };

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        let lower = line.to_ascii_lowercase();

        let has_regex_or_string_op = [
            "new regexp(",
            "regexp(",
            ".replace(",
            ".replaceall(",
            ".match(",
            ".matchall(",
            ".split(",
        ]
        .iter()
        .any(|needle| lower.contains(needle));

        let source_transform_context = [
            "sourcetext",
            "source_text",
            "source",
            "bodytext",
            "body_text",
            "renderbodytext",
            "render_body_text",
            "nextbodytext",
            "next_body_text",
            "originaltext",
            "original_text",
            "rawtext",
            "raw_text",
            "filetext",
            "file_text",
            "content",
            "code",
        ]
        .iter()
        .any(|needle| lower.contains(needle));

        if has_regex_or_string_op && source_transform_context {
            summary.risky_regex_lines.push(line_number);
        }
    }

    summary.risky_regex_lines.sort_unstable();
    summary.risky_regex_lines.dedup();
    summary
}

async fn run_package_script(
    package_root: &Path,
    package_manager: PackageManager,
    script: &str,
    timeout_seconds: u64,
) -> ProcessCheckResult {
    let package_command = PackageCommand::RunScript {
        manager: package_manager,
        script,
    };
    let command_display = package_command.to_string();
    let mut command = Command::new(package_command.program());
    package_command.apply(&mut command);

    let result = tokio::time::timeout(
        Duration::from_secs(timeout_seconds),
        command.current_dir(package_root).output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => ProcessCheckResult {
            command: command_display.clone(),
            success: output.status.success(),
            exit_code: output.status.code(),
            timed_out: false,
            stdout_tail: truncate_tail(&String::from_utf8_lossy(&output.stdout), 2000),
            stderr_tail: truncate_tail(&String::from_utf8_lossy(&output.stderr), 2000),
        },
        Ok(Err(error)) => ProcessCheckResult {
            command: command_display.clone(),
            success: false,
            exit_code: None,
            timed_out: false,
            stdout_tail: String::new(),
            stderr_tail: error.to_string(),
        },
        Err(_) => ProcessCheckResult {
            command: command_display,
            success: false,
            exit_code: None,
            timed_out: true,
            stdout_tail: String::new(),
            stderr_tail: format!("Timed out after {timeout_seconds}s"),
        },
    }
}

fn truncate_tail(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let truncated = value
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();

    format!("…{truncated}")
}

fn infer_package_manager(package_root: &Path) -> PackageManager {
    PackageManager::from_package_root(package_root)
}

impl Default for PackageValidationHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("expected monotonic time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("codemod-mcp-validate-{}", unique));
        fs::create_dir_all(&dir).expect("expected temp dir");
        dir
    }

    #[tokio::test]
    async fn starter_scaffold_markers_are_detected() {
        let dir = unique_temp_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/fixtures")).unwrap();
        fs::write(dir.join("codemod.yaml"), "workflow: workflow.yaml\n").unwrap();
        fs::write(dir.join("workflow.yaml"), "version: \"1\"\nnodes: []\n").unwrap();
        fs::write(dir.join("package.json"), "{\"scripts\":{}}\n").unwrap();
        fs::write(dir.join("README.md"), STARTER_README_MARKERS[0]).unwrap();
        fs::write(dir.join("scripts/codemod.ts"), STARTER_TRANSFORM_MARKER).unwrap();
        fs::write(
            dir.join("tests/fixtures/input.js"),
            STARTER_FIXTURE_INPUT_MARKER,
        )
        .unwrap();
        fs::write(
            dir.join("tests/fixtures/expected.js"),
            STARTER_FIXTURE_EXPECTED_MARKER,
        )
        .unwrap();

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .unwrap();

        assert!(response.starter_transform_detected);
        assert!(response.generic_readme_detected);
        assert!(response.test_cases.starter_fixtures_detected);
        assert!(!response.ready);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn custom_workflow_path_is_respected_for_presence_and_validation() {
        let dir = unique_temp_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/basic")).unwrap();
        fs::create_dir_all(dir.join("config")).unwrap();
        fs::write(
            dir.join("codemod.yaml"),
            "workflow: config/custom-workflow.yaml\n",
        )
        .unwrap();
        fs::write(
            dir.join("config/custom-workflow.yaml"),
            "version: \"1\"\nnodes: []\n",
        )
        .unwrap();
        fs::write(dir.join("package.json"), "{\"scripts\":{}}\n").unwrap();
        fs::write(dir.join("README.md"), "# Example\n").unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            "export default function() { return null; }\n",
        )
        .unwrap();
        fs::write(dir.join("tests/basic/input.ts"), "console.log('x');\n").unwrap();
        fs::write(dir.join("tests/basic/expected.ts"), "console.log('x');\n").unwrap();

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .unwrap();

        assert!(response.files.workflow_yaml);
        assert!(response.workflow_valid);
        assert!(response
            .issues
            .iter()
            .all(|issue| issue.code != "missing_workflow_yaml"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn shell_packages_do_not_require_jssg_artifacts() {
        let dir = unique_temp_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("codemod.yaml"), "workflow: workflow.yaml\n").unwrap();
        fs::write(dir.join("workflow.yaml"), "version: \"1\"\nnodes: []\n").unwrap();
        fs::write(dir.join("README.md"), "# Shell package\n").unwrap();
        fs::write(dir.join("scripts/setup.sh"), "#!/usr/bin/env bash\n").unwrap();
        fs::write(dir.join("scripts/transform.sh"), "#!/usr/bin/env bash\n").unwrap();
        fs::write(dir.join("scripts/cleanup.sh"), "#!/usr/bin/env bash\n").unwrap();

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .unwrap();

        assert!(response
            .issues
            .iter()
            .all(|issue| issue.code != "missing_package_json"));
        assert!(response
            .issues
            .iter()
            .all(|issue| issue.code != "missing_transform_script"));
        assert!(response
            .issues
            .iter()
            .all(|issue| issue.code != "missing_tests_dir"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn yaml_packages_do_not_require_jssg_package_files() {
        let dir = unique_temp_dir();
        fs::create_dir_all(dir.join("rules")).unwrap();
        fs::create_dir_all(dir.join("tests/input")).unwrap();
        fs::create_dir_all(dir.join("tests/expected")).unwrap();
        fs::write(dir.join("codemod.yaml"), "workflow: workflow.yaml\n").unwrap();
        fs::write(dir.join("workflow.yaml"), "version: \"1\"\nnodes: []\n").unwrap();
        fs::write(dir.join("README.md"), "# YAML package\n").unwrap();
        fs::write(dir.join("rules/config.yml"), "id: sample\n").unwrap();

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .unwrap();

        assert!(response
            .issues
            .iter()
            .all(|issue| issue.code != "missing_package_json"));
        assert!(response
            .issues
            .iter()
            .all(|issue| issue.code != "missing_transform_script"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn raw_source_rewrite_is_flagged_but_ast_node_replace_is_not() {
        let risky = detect_transform_risks(Some(
            "const nextBodyText = bodyText.replace(/foo/g, \"bar\");",
        ));
        assert_eq!(risky.risky_regex_lines, vec![1]);

        let safe =
            detect_transform_risks(Some("const nextNode = callee.replace(\"amazing.log\");"));
        assert!(safe.risky_regex_lines.is_empty());
    }

    #[test]
    fn package_manager_drift_is_detected_in_scripts() {
        let package_json = PackageJsonLite {
            scripts: BTreeMap::from([(
                "test".to_string(),
                "npx codemod@latest jssg test ./scripts/codemod.ts".to_string(),
            )]),
            package_manager: Some("yarn".to_string()),
        };

        let issues = detect_package_manager_drift_in_scripts(&package_json, PackageManager::Yarn);
        assert_eq!(issues, vec!["test".to_string()]);
    }

    #[test]
    fn matching_package_manager_commands_are_not_flagged_in_scripts() {
        let package_json = PackageJsonLite {
            scripts: BTreeMap::from([(
                "test".to_string(),
                "yarn dlx codemod@latest jssg test ./scripts/codemod.ts".to_string(),
            )]),
            package_manager: Some("yarn".to_string()),
        };

        let issues = detect_package_manager_drift_in_scripts(&package_json, PackageManager::Yarn);
        assert!(issues.is_empty());
    }

    #[test]
    fn package_manager_from_manifest_prevents_fresh_package_false_positive() {
        let package_json = PackageJsonLite {
            scripts: BTreeMap::from([(
                "test".to_string(),
                "pnpm dlx codemod@latest jssg test ./scripts/codemod.ts".to_string(),
            )]),
            package_manager: Some("pnpm".to_string()),
        };

        let issues = detect_package_manager_drift_in_scripts(&package_json, PackageManager::Pnpm);
        assert!(issues.is_empty());
    }

    #[tokio::test]
    async fn invalid_codemod_yaml_is_reported_without_failing_tool() {
        let dir = unique_temp_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/basic")).unwrap();
        fs::write(dir.join("codemod.yaml"), "workflow: [\n").unwrap();
        fs::write(dir.join("workflow.yaml"), "version: \"1\"\nnodes: []\n").unwrap();
        fs::write(dir.join("package.json"), "{\"scripts\":{}}\n").unwrap();
        fs::write(dir.join("README.md"), "# Example\n").unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            "export default function() { return null; }\n",
        )
        .unwrap();
        fs::write(dir.join("tests/basic/input.ts"), "console.log('x');\n").unwrap();
        fs::write(dir.join("tests/basic/expected.ts"), "console.log('x');\n").unwrap();

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .unwrap();

        assert!(response
            .issues
            .iter()
            .any(|issue| issue.code == "codemod_yaml_invalid"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn yarn_command_display_matches_actual_invocation_shape() {
        assert_eq!(
            PackageCommand::RunScript {
                manager: PackageManager::Yarn,
                script: "test",
            }
            .to_string(),
            "yarn test"
        );
        assert_eq!(
            PackageCommand::RunScript {
                manager: PackageManager::Npm,
                script: "test",
            }
            .to_string(),
            "npm run test"
        );
    }
}
