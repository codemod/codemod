use anyhow::{anyhow, Context};
use butterflow_core::utils::{parse_workflow_file, validate_workflow};
use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool, ErrorData as McpError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
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
    pub coverage_contract_json: bool,
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
    pub coverage_contract_present: bool,
    pub supported_shape_coverage_complete: bool,
    pub starter_transform_detected: bool,
    pub generic_readme_detected: bool,
    pub risky_regex_transform_detected: bool,
    pub manual_parsing_detected: bool,
    pub missing_runtime_capabilities: bool,
    pub broad_ai_step_detected: bool,
    pub typescript_escape_hatch_detected: bool,
    pub nonstandard_test_runner_detected: bool,
    pub issues: Vec<ValidationIssue>,
    pub default_test: Option<ProcessCheckResult>,
    pub check_types: Option<ProcessCheckResult>,
    pub ready: bool,
}

#[derive(Debug, Deserialize)]
struct CodemodManifestLite {
    workflow: Option<String>,
    capabilities: Option<Vec<String>>,
    registry: Option<RegistryLite>,
}

#[derive(Debug, Deserialize)]
struct RegistryLite {
    access: Option<String>,
    visibility: Option<String>,
}

#[derive(Debug, Default)]
struct PackageJsonLite {
    scripts: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct CoverageContract {
    supported_shapes: Vec<String>,
    #[serde(default)]
    unsupported_shapes: Vec<String>,
    #[serde(default)]
    manual_follow_up_shapes: Vec<String>,
    #[serde(default)]
    cases: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default)]
struct TransformRiskSummary {
    risky_regex_lines: Vec<usize>,
    manual_parsing_lines: Vec<usize>,
    allowed_string_cleanup_lines: Vec<usize>,
}

#[derive(Debug, Default)]
struct CapabilityRiskSummary {
    missing_capabilities: Vec<String>,
}

#[derive(Debug, Default)]
struct AiStepRiskSummary {
    broad_ai_detected: bool,
}

#[derive(Clone)]
pub struct PackageValidationHandler;

impl PackageValidationHandler {
    pub fn new() -> Self {
        Self
    }

    #[tool(
        description = "Validate whether a codemod package is real and complete. Use this before stopping work on a codemod package. It detects starter scaffolds, generic README text, missing package surface updates, missing test coverage, invalid workflow structure, and optionally runs the package's default tests and type-check script."
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
    ) -> anyhow::Result<ValidateCodemodPackageResponse> {
        let package_root = canonicalize_package_root(request.package_path.as_deref())?;

        let files = collect_file_presence(&package_root);
        let manifest = load_manifest(&package_root);
        let package_json = load_package_json(&package_root);
        let workflow_path = package_root.join(
            manifest
                .as_ref()
                .and_then(|manifest| manifest.workflow.as_deref())
                .filter(|workflow| !workflow.trim().is_empty())
                .unwrap_or("workflow.yaml"),
        );

        let workflow_valid = validate_workflow_at_path(&workflow_path).is_ok();
        let transform_path = package_root.join("scripts/codemod.ts");
        let readme_path = package_root.join("README.md");
        let tests_path = package_root.join("tests");
        let coverage_contract_path = tests_path.join("coverage-contract.json");
        let workflow_content = read_file_if_exists(&workflow_path);

        let transform_content = read_file_if_exists(&transform_path);
        let readme_content = read_file_if_exists(&readme_path);
        let transform_risks = detect_transform_risks(transform_content.as_deref());
        let capability_risks = detect_missing_runtime_capabilities(
            transform_content.as_deref(),
            manifest
                .as_ref()
                .and_then(|manifest| manifest.capabilities.as_ref())
                .cloned()
                .unwrap_or_default(),
        );
        let ai_step_risks = detect_ai_step_risks(workflow_content.as_deref());
        let ts_escape_hatch_detected = transform_content
            .as_deref()
            .is_some_and(|content| content.contains("@ts-nocheck"));

        let starter_transform_detected = transform_content
            .as_deref()
            .is_some_and(|content| content.contains(STARTER_TRANSFORM_MARKER));
        let generic_readme_detected = readme_content.as_deref().is_some_and(|content| {
            STARTER_README_MARKERS
                .iter()
                .any(|marker| content.contains(marker))
        });

        let test_cases = summarize_test_cases(&tests_path);
        let coverage_contract = load_coverage_contract(&coverage_contract_path);
        let coverage_contract_present = matches!(coverage_contract, Ok(Some(_)));
        let coverage_issues = validate_coverage_contract(
            coverage_contract.as_ref(),
            &coverage_contract_path,
            &tests_path,
            &test_cases,
        );
        let supported_shape_coverage_complete = coverage_issues
            .iter()
            .all(|issue| issue.code != "missing_supported_shape_coverage"
                && issue.code != "missing_coverage_contract"
                && issue.code != "coverage_contract_parse_failed");

        let scripts = PackageScriptSummary {
            test: package_json.scripts.get("test").cloned(),
            check_types: package_json.scripts.get("check-types").cloned(),
        };
        let nonstandard_test_runner_detected =
            detect_nonstandard_test_runner(&scripts, &package_root, &test_cases);

        let default_test = if request.run_default_test && scripts.test.is_some() {
            Some(
                run_package_script(
                    &package_root,
                    infer_package_manager_command(&package_root),
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
                    infer_package_manager_command(&package_root),
                    "check-types",
                    request.command_timeout_seconds,
                )
                .await,
            )
        } else {
            None
        };

        let mut issues = Vec::new();
        push_missing_file_issues(&files, &package_root, &mut issues);
        issues.extend(coverage_issues);

        if manifest.is_none() {
            issues.push(issue(
                "error",
                "manifest_parse_failed",
                "Failed to parse codemod.yaml.",
                Some(package_root.join("codemod.yaml")),
            ));
        }

        if package_json.scripts.is_empty() && files.package_json {
            issues.push(issue(
                "warning",
                "package_json_missing_scripts",
                "package.json has no usable scripts metadata.",
                Some(package_root.join("package.json")),
            ));
        }

        if manifest
            .as_ref()
            .and_then(|manifest| manifest.registry.as_ref())
            .is_some_and(|registry| {
                matches!(registry.access.as_deref(), Some("private"))
                    || matches!(registry.visibility.as_deref(), Some("private"))
            })
            && readme_content
                .as_deref()
                .is_some_and(|content| content.to_ascii_lowercase().contains("reusable"))
        {
            issues.push(issue(
                "warning",
                "private_registry_default_detected",
                "The package describes itself as reusable, but codemod.yaml defaults registry access/visibility to private.",
                Some(package_root.join("codemod.yaml")),
            ));
        }

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
                "README.md still contains starter/generic package text.",
                Some(readme_path.clone()),
            ));
        }

        if starter_transform_detected || generic_readme_detected {
            issues.push(issue(
                "error",
                "starter_or_generic_package_content_present",
                "The package still contains starter scaffold content and should not be treated as complete.",
                Some(package_root.clone()),
            ));
        }

        if test_cases.starter_fixtures_detected {
            issues.push(issue(
                "error",
                "starter_fixtures_present",
                "tests still contain the default starter fixtures.",
                Some(tests_path.join("fixtures")),
            ));
        }

        if !transform_risks.risky_regex_lines.is_empty() {
            issues.push(issue(
                "error",
                "risky_regex_transform_detected",
                &format!(
                    "scripts/codemod.ts uses regex or string operations in likely source-transform logic at lines {}.",
                    format_line_list(&transform_risks.risky_regex_lines)
                ),
                Some(transform_path.clone()),
            ));
        }

        if !transform_risks.manual_parsing_lines.is_empty() {
            issues.push(issue(
                "error",
                "manual_parsing_detected",
                &format!(
                    "scripts/codemod.ts appears to manually parse source text at lines {}.",
                    format_line_list(&transform_risks.manual_parsing_lines)
                ),
                Some(transform_path.clone()),
            ));
        }

        if !capability_risks.missing_capabilities.is_empty() {
            issues.push(issue(
                "error",
                "missing_capability_for_runtime_api",
                &format!(
                    "codemod.yaml is missing required capabilities for runtime APIs used by the transform: {}.",
                    capability_risks.missing_capabilities.join(", ")
                ),
                Some(package_root.join("codemod.yaml")),
            ));
        }

        if ai_step_risks.broad_ai_detected {
            issues.push(issue(
                "error",
                "ai_step_without_narrow_scope",
                "workflow.yaml contains AI steps that appear to be broad or primary instead of a narrow fallback for unresolved cases.",
                Some(workflow_path.clone()),
            ));
        }

        if ts_escape_hatch_detected {
            issues.push(issue(
                "warning",
                "typescript_escape_hatch_detected",
                "scripts/codemod.ts uses `@ts-nocheck`, which is usually a sign the transform is bypassing normal type safety.",
                Some(transform_path.clone()),
            ));
        }

        if nonstandard_test_runner_detected {
            issues.push(issue(
                "warning",
                "nonstandard_test_runner_detected",
                "package.json uses a custom test runner instead of the standard `codemod jssg test` flow for codemod verification.",
                Some(package_root.join("package.json")),
            ));
        }

        if test_cases.total_case_dirs == 0 {
            issues.push(issue(
                "error",
                "missing_test_cases",
                "No codemod test cases were detected under tests/.",
                Some(tests_path.clone()),
            ));
        }

        if test_cases.positive_case_dirs == 0 {
            issues.push(issue(
                "warning",
                "missing_positive_cases",
                "No clearly labeled positive test cases were detected.",
                Some(tests_path.clone()),
            ));
        }

        if test_cases.negative_case_dirs == 0 {
            issues.push(issue(
                "warning",
                "missing_negative_cases",
                "No clearly labeled negative/preserve/no-op test cases were detected.",
                Some(tests_path.clone()),
            ));
        }

        if test_cases.edge_case_dirs == 0 {
            issues.push(issue(
                "warning",
                "missing_edge_cases",
                "No clearly labeled edge-case test cases were detected.",
                Some(tests_path.clone()),
            ));
        }

        if let Some(result) = &default_test {
            if !result.success {
                issues.push(issue(
                    "error",
                    "default_test_failed",
                    "The package default test command failed.",
                    Some(package_root.join("package.json")),
                ));
            }
        } else if scripts.test.is_none() && files.package_json {
            issues.push(issue(
                "error",
                "missing_test_script",
                "package.json is missing a `test` script.",
                Some(package_root.join("package.json")),
            ));
        }

        if let Some(result) = &check_types {
            if !result.success {
                issues.push(issue(
                    "error",
                    "check_types_failed",
                    "The package `check-types` script failed.",
                    Some(package_root.join("package.json")),
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
            coverage_contract_present,
            supported_shape_coverage_complete,
            starter_transform_detected,
            generic_readme_detected,
            risky_regex_transform_detected: !transform_risks.risky_regex_lines.is_empty(),
            manual_parsing_detected: !transform_risks.manual_parsing_lines.is_empty(),
            missing_runtime_capabilities: !capability_risks.missing_capabilities.is_empty(),
            broad_ai_step_detected: ai_step_risks.broad_ai_detected,
            typescript_escape_hatch_detected: ts_escape_hatch_detected,
            nonstandard_test_runner_detected,
            issues,
            default_test,
            check_types,
            ready,
        })
    }
}

fn canonicalize_package_root(package_path: Option<&str>) -> anyhow::Result<PathBuf> {
    let raw_path = package_path.unwrap_or(".");
    let root = PathBuf::from(raw_path);
    std::fs::canonicalize(&root)
        .with_context(|| format!("Failed to resolve package path {}", root.display()))
}

fn collect_file_presence(package_root: &Path) -> PackageFilePresence {
    PackageFilePresence {
        codemod_yaml: package_root.join("codemod.yaml").is_file(),
        workflow_yaml: package_root.join("workflow.yaml").is_file(),
        package_json: package_root.join("package.json").is_file(),
        readme_md: package_root.join("README.md").is_file(),
        scripts_codemod_ts: package_root.join("scripts/codemod.ts").is_file(),
        tests_dir: package_root.join("tests").is_dir(),
        coverage_contract_json: package_root.join("tests/coverage-contract.json").is_file(),
    }
}

fn load_manifest(package_root: &Path) -> Option<CodemodManifestLite> {
    let path = package_root.join("codemod.yaml");
    let content = fs::read_to_string(path).ok()?;
    serde_yaml::from_str::<CodemodManifestLite>(&content).ok()
}

fn load_package_json(package_root: &Path) -> PackageJsonLite {
    let path = package_root.join("package.json");
    let Ok(content) = fs::read_to_string(path) else {
        return PackageJsonLite::default();
    };

    let Ok(json) = serde_json::from_str::<Value>(&content) else {
        return PackageJsonLite::default();
    };

    let mut scripts = BTreeMap::new();
    if let Some(obj) = json.get("scripts").and_then(|value| value.as_object()) {
        for (name, value) in obj {
            if let Some(command) = value.as_str() {
                scripts.insert(name.clone(), command.to_string());
            }
        }
    }

    PackageJsonLite { scripts }
}

fn validate_workflow_at_path(workflow_path: &Path) -> anyhow::Result<()> {
    let workflow = parse_workflow_file(workflow_path)
        .with_context(|| format!("Failed to parse workflow file {}", workflow_path.display()))?;
    let parent_dir = workflow_path
        .parent()
        .ok_or_else(|| anyhow!("Workflow path has no parent: {}", workflow_path.display()))?;
    validate_workflow(&workflow, parent_dir)
        .with_context(|| format!("Workflow validation failed for {}", workflow_path.display()))
}

fn read_file_if_exists(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn load_coverage_contract(path: &Path) -> anyhow::Result<Option<CoverageContract>> {
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(None);
    };
    let contract = serde_json::from_str::<CoverageContract>(&content).with_context(|| {
        format!(
            "Failed to parse coverage contract JSON at {}",
            path.display()
        )
    })?;
    Ok(Some(contract))
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

fn push_missing_file_issues(
    files: &PackageFilePresence,
    package_root: &Path,
    issues: &mut Vec<ValidationIssue>,
) {
    let checks = [
        (
            files.codemod_yaml,
            "missing_codemod_yaml",
            "codemod.yaml is missing.",
        ),
        (
            files.workflow_yaml,
            "missing_workflow_yaml",
            "workflow.yaml is missing.",
        ),
        (
            files.package_json,
            "missing_package_json",
            "package.json is missing.",
        ),
        (files.readme_md, "missing_readme", "README.md is missing."),
        (
            files.scripts_codemod_ts,
            "missing_transform_script",
            "scripts/codemod.ts is missing.",
        ),
        (
            files.tests_dir,
            "missing_tests_dir",
            "tests/ directory is missing.",
        ),
    ];

    for (present, code, message) in checks {
        if !present {
            issues.push(issue(
                "error",
                code,
                message,
                Some(package_root.join(code_to_path(code))),
            ));
        }
    }
}

fn code_to_path(code: &str) -> &'static str {
    match code {
        "missing_codemod_yaml" => "codemod.yaml",
        "missing_workflow_yaml" => "workflow.yaml",
        "missing_package_json" => "package.json",
        "missing_readme" => "README.md",
        "missing_transform_script" => "scripts/codemod.ts",
        "missing_tests_dir" => "tests",
        _ => ".",
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
        .take(8)
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join(", ")
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

        let has_manual_parse_op = [
            ".indexof(",
            ".lastindexof(",
            ".slice(",
            ".substring(",
            "findmatchingdelimiter(",
            "findmatchingindex(",
            "splittoplevel(",
            "parsetypemembers(",
            "parseobjectparameter(",
            "extracttoplevelpropertyvalue(",
        ]
        .iter()
        .any(|needle| lower.contains(needle));

        let allowed_context = [
            "path",
            "filepath",
            "filename",
            "import",
            "specifier",
            "module",
            "helper",
            "metadata",
            "stdout",
            "stderr",
            "metrics",
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
            "content",
            "code",
        ]
        .iter()
        .any(|needle| lower.contains(needle));

        if has_regex_or_string_op {
            if source_transform_context || !allowed_context {
                summary.risky_regex_lines.push(line_number);
            } else {
                summary.allowed_string_cleanup_lines.push(line_number);
            }
        }

        if has_manual_parse_op && (source_transform_context || !allowed_context) {
            summary.manual_parsing_lines.push(line_number);
        }
    }

    summary.risky_regex_lines.sort_unstable();
    summary.risky_regex_lines.dedup();
    summary.manual_parsing_lines.sort_unstable();
    summary.manual_parsing_lines.dedup();
    summary.allowed_string_cleanup_lines.sort_unstable();
    summary.allowed_string_cleanup_lines.dedup();

    summary
}

fn detect_missing_runtime_capabilities(
    transform_content: Option<&str>,
    manifest_capabilities: Vec<String>,
) -> CapabilityRiskSummary {
    let mut required = BTreeSet::new();
    let Some(content) = transform_content else {
        return CapabilityRiskSummary::default();
    };

    let lower = content.to_ascii_lowercase();
    if lower.contains("from \"fs\"")
        || lower.contains("from 'fs'")
        || lower.contains("from \"node:fs\"")
        || lower.contains("from 'node:fs'")
        || lower.contains("writefilesync(")
        || lower.contains("readfilesync(")
        || lower.contains("mkdirsync(")
        || lower.contains("statsync(")
        || lower.contains("accesssync(")
        || lower.contains("appendfilesync(")
    {
        required.insert("fs".to_string());
    }
    if lower.contains("fetch(") {
        required.insert("fetch".to_string());
    }
    if lower.contains("from \"child_process\"")
        || lower.contains("from 'child_process'")
        || lower.contains("from \"node:child_process\"")
        || lower.contains("from 'node:child_process'")
        || lower.contains("exec(")
        || lower.contains("spawn(")
        || lower.contains("fork(")
    {
        required.insert("child_process".to_string());
    }

    let declared = manifest_capabilities
        .into_iter()
        .map(|capability| capability.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();

    CapabilityRiskSummary {
        missing_capabilities: required
            .into_iter()
            .filter(|capability| !declared.contains(&capability.to_ascii_lowercase()))
            .collect(),
    }
}

fn detect_ai_step_risks(workflow_content: Option<&str>) -> AiStepRiskSummary {
    let Some(content) = workflow_content else {
        return AiStepRiskSummary::default();
    };

    let ai_steps = content
        .lines()
        .filter(|line| line.trim_start().starts_with("ai:"))
        .count();
    let deterministic_steps = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("js-ast-grep:")
                || trimmed.starts_with("ast-grep:")
                || trimmed.starts_with("command:")
        })
        .count();

    AiStepRiskSummary {
        broad_ai_detected: ai_steps > 1 || (ai_steps > 0 && deterministic_steps == 0),
    }
}

fn detect_nonstandard_test_runner(
    scripts: &PackageScriptSummary,
    package_root: &Path,
    test_cases: &TestCaseSummary,
) -> bool {
    let Some(test_script) = scripts.test.as_deref() else {
        return false;
    };

    if test_script.contains("codemod") && test_script.contains("jssg test") {
        return false;
    }

    if package_root.join("tests/run.mjs").is_file() || package_root.join("tests/run.js").is_file() {
        return true;
    }

    test_cases.total_case_dirs > 0 && test_script.contains("node ")
}

fn validate_coverage_contract(
    contract: Result<&Option<CoverageContract>, &anyhow::Error>,
    contract_path: &Path,
    tests_path: &Path,
    test_cases: &TestCaseSummary,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    if !tests_path.is_dir() {
        return issues;
    }

    let contract = match contract {
        Ok(Some(contract)) => contract,
        Ok(None) => {
            issues.push(issue(
                "error",
                "missing_coverage_contract",
                "tests/coverage-contract.json is required so supported shapes and test coverage are explicit.",
                Some(contract_path.to_path_buf()),
            ));
            return issues;
        }
        Err(_) => {
            issues.push(issue(
                "error",
                "coverage_contract_parse_failed",
                "tests/coverage-contract.json is present but could not be parsed.",
                Some(contract_path.to_path_buf()),
            ));
            return issues;
        }
    };

    if contract.supported_shapes.is_empty() {
        issues.push(issue(
            "error",
            "missing_supported_shape_coverage",
            "coverage-contract.json has no supported_shapes entries.",
            Some(contract_path.to_path_buf()),
        ));
    }

    let unsupported = contract
        .unsupported_shapes
        .iter()
        .chain(contract.manual_follow_up_shapes.iter())
        .cloned()
        .collect::<BTreeSet<_>>();

    for shape in &contract.supported_shapes {
        if unsupported.contains(shape) {
            issues.push(issue(
                "error",
                "unsupported_shape_not_documented",
                &format!(
                    "Shape `{shape}` is listed as both supported and unsupported/manual in coverage-contract.json."
                ),
                Some(contract_path.to_path_buf()),
            ));
            continue;
        }

        let Some(cases) = contract.cases.get(shape) else {
            issues.push(issue(
                "error",
                "missing_supported_shape_coverage",
                &format!(
                    "Supported shape `{shape}` does not map to any fixture cases in coverage-contract.json."
                ),
                Some(contract_path.to_path_buf()),
            ));
            continue;
        };

        if cases.is_empty() {
            issues.push(issue(
                "error",
                "missing_supported_shape_coverage",
                &format!(
                    "Supported shape `{shape}` maps to an empty case list in coverage-contract.json."
                ),
                Some(contract_path.to_path_buf()),
            ));
        }
    }

    let case_dirs = list_case_dir_names(tests_path);
    for (shape, cases) in &contract.cases {
        for case_name in cases {
            if !case_dirs.contains(case_name) {
                issues.push(issue(
                    "error",
                    "coverage_contract_missing_case_dir",
                    &format!(
                        "coverage-contract.json maps shape `{shape}` to missing test case directory `{case_name}`."
                    ),
                    Some(contract_path.to_path_buf()),
                ));
            }
        }
    }

    if test_cases.total_case_dirs > 0 && contract.cases.is_empty() {
        issues.push(issue(
            "error",
            "missing_supported_shape_coverage",
            "coverage-contract.json must map supported shapes to at least one real case directory.",
            Some(contract_path.to_path_buf()),
        ));
    }

    issues
}

fn list_case_dir_names(tests_path: &Path) -> BTreeSet<String> {
    let Ok(entries) = fs::read_dir(tests_path) else {
        return BTreeSet::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|file_type| file_type.is_dir())
                .and_then(|_| entry.file_name().into_string().ok())
        })
        .collect()
}

fn infer_package_manager_command(package_root: &Path) -> &'static str {
    if package_root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if package_root.join("yarn.lock").is_file() {
        "yarn"
    } else if package_root.join("bun.lockb").is_file() || package_root.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    }
}

async fn run_package_script(
    package_root: &Path,
    package_manager: &str,
    script_name: &str,
    timeout_seconds: u64,
) -> ProcessCheckResult {
    let command = format!("{package_manager} run {script_name}");
    let child = Command::new("sh")
        .arg("-lc")
        .arg(&command)
        .current_dir(package_root)
        .output();

    match tokio::time::timeout(Duration::from_secs(timeout_seconds), child).await {
        Ok(Ok(output)) => ProcessCheckResult {
            command,
            success: output.status.success(),
            exit_code: output.status.code(),
            timed_out: false,
            stdout_tail: tail_string(&String::from_utf8_lossy(&output.stdout)),
            stderr_tail: tail_string(&String::from_utf8_lossy(&output.stderr)),
        },
        Ok(Err(error)) => ProcessCheckResult {
            command,
            success: false,
            exit_code: None,
            timed_out: false,
            stdout_tail: String::new(),
            stderr_tail: error.to_string(),
        },
        Err(_) => ProcessCheckResult {
            command,
            success: false,
            exit_code: None,
            timed_out: true,
            stdout_tail: String::new(),
            stderr_tail: format!("Timed out after {timeout_seconds}s"),
        },
    }
}

fn tail_string(value: &str) -> String {
    let mut lines = value.lines().collect::<Vec<_>>();
    if lines.len() > 40 {
        lines = lines.split_off(lines.len() - 40);
    }
    lines.join("\n")
}

impl Default for PackageValidationHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_package_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("expected monotonic time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("codemod-mcp-package-{}", unique));
        fs::create_dir_all(&dir).expect("expected temp package dir");
        dir
    }

    fn write_coverage_contract(
        dir: &Path,
        supported_shapes: &[&str],
        cases: &[(&str, &[&str])],
    ) {
        fs::create_dir_all(dir.join("tests")).unwrap();
        let cases_json = cases
            .iter()
            .map(|(shape, mapped_cases)| {
                let values = mapped_cases
                    .iter()
                    .map(|case_name| format!("\"{case_name}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("    \"{shape}\": [{values}]")
            })
            .collect::<Vec<_>>()
            .join(",\n");
        let supported_json = supported_shapes
            .iter()
            .map(|shape| format!("\"{shape}\""))
            .collect::<Vec<_>>()
            .join(", ");

        fs::write(
            dir.join("tests/coverage-contract.json"),
            format!(
                "{{\n  \"supported_shapes\": [{supported_json}],\n  \"cases\": {{\n{cases_json}\n  }}\n}}\n"
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn detects_starter_scaffold_markers() {
        let dir = temp_package_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/fixtures")).unwrap();
        fs::write(
            dir.join("codemod.yaml"),
            r#"schema_version: "1.0"
name: "example"
version: "0.1.0"
description: "desc"
workflow: "workflow.yaml"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("workflow.yaml"),
            r#"version: "1"
nodes:
  - id: apply
    name: Apply
    type: automatic
    steps:
      - name: Run codemod
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: "typescript"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"name":"example","scripts":{"test":"echo ok"}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("README.md"),
            "This codemod transforms typescript code by:\n- Converting `var` declarations to `const`/`let`\n",
        )
        .unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            r#"const transform = () => ({ rule: { pattern: "var $VAR = $VALUE" } });"#,
        )
        .unwrap();
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
        write_coverage_contract(
            &dir,
            &["starter"],
            &[("starter", &["fixtures"])],
        );

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .expect("expected package validation response");

        assert!(response.starter_transform_detected);
        assert!(response.generic_readme_detected);
        assert!(response.test_cases.starter_fixtures_detected);
        assert!(!response.ready);

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn detects_missing_files() {
        let dir = temp_package_dir();
        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .expect("expected package validation response");

        assert!(!response.files.codemod_yaml);
        assert!(response
            .issues
            .iter()
            .any(|issue| issue.code == "missing_codemod_yaml"));
        assert!(!response.ready);

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn detects_risky_regex_transform_usage() {
        let dir = temp_package_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/case-a")).unwrap();
        fs::write(
            dir.join("codemod.yaml"),
            r#"schema_version: "1.0"
name: "example"
version: "0.1.0"
description: "desc"
workflow: "workflow.yaml"
capabilities: []
"#,
        )
        .unwrap();
        fs::write(
            dir.join("workflow.yaml"),
            r#"version: "1"
nodes:
  - id: apply
    name: Apply
    type: automatic
    steps:
      - name: Run codemod
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: "typescript"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"name":"example","scripts":{"test":"echo ok","check-types":"echo ok"}}"#,
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Example\n\nReal package.\n").unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            r#"export default async function transform(root) {
  const bodyText = root.root().text();
  const nextBodyText = bodyText.replace(/foo/g, "bar");
  return nextBodyText;
}"#,
        )
        .unwrap();
        fs::write(dir.join("tests/case-a/input.ts"), "foo();").unwrap();
        fs::write(dir.join("tests/case-a/expected.ts"), "bar();").unwrap();
        write_coverage_contract(&dir, &["shape-a"], &[("shape-a", &["case-a"])]);

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .expect("expected package validation response");

        assert!(response.risky_regex_transform_detected);
        assert!(response
            .issues
            .iter()
            .any(|issue| issue.code == "risky_regex_transform_detected"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn allows_path_normalization_string_cleanup() {
        let dir = temp_package_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/case-a")).unwrap();
        fs::write(
            dir.join("codemod.yaml"),
            r#"schema_version: "1.0"
name: "example"
version: "0.1.0"
description: "desc"
workflow: "workflow.yaml"
capabilities: []
"#,
        )
        .unwrap();
        fs::write(
            dir.join("workflow.yaml"),
            r#"version: "1"
nodes:
  - id: apply
    name: Apply
    type: automatic
    steps:
      - name: Run codemod
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: "typescript"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"name":"example","scripts":{"test":"echo ok","check-types":"echo ok"}}"#,
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Example\n\nReal package.\n").unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            r#"function normalizePath(pathValue: string) {
  return pathValue.replace(/\\/g, "/");
}
export default async function transform() {
  return null;
}"#,
        )
        .unwrap();
        fs::write(dir.join("tests/case-a/input.ts"), "foo();").unwrap();
        fs::write(dir.join("tests/case-a/expected.ts"), "foo();").unwrap();
        write_coverage_contract(&dir, &["shape-a"], &[("shape-a", &["case-a"])]);

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .expect("expected package validation response");

        assert!(!response.risky_regex_transform_detected);
        assert!(!response
            .issues
            .iter()
            .any(|issue| issue.code == "risky_regex_transform_detected"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn detects_missing_runtime_capabilities() {
        let dir = temp_package_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/case-a")).unwrap();
        fs::write(
            dir.join("codemod.yaml"),
            r#"schema_version: "1.0"
name: "example"
version: "0.1.0"
description: "desc"
workflow: "workflow.yaml"
capabilities: []
"#,
        )
        .unwrap();
        fs::write(
            dir.join("workflow.yaml"),
            r#"version: "1"
nodes:
  - id: apply
    name: Apply
    type: automatic
    steps:
      - name: Run codemod
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: "typescript"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"name":"example","scripts":{"test":"echo ok","check-types":"echo ok"}}"#,
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Example\n\nReal package.\n").unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            r#"import { writeFileSync } from "fs";
export default async function transform() {
  writeFileSync("out.txt", "hello");
  return null;
}"#,
        )
        .unwrap();
        fs::write(dir.join("tests/case-a/input.ts"), "foo();").unwrap();
        fs::write(dir.join("tests/case-a/expected.ts"), "foo();").unwrap();
        write_coverage_contract(&dir, &["shape-a"], &[("shape-a", &["case-a"])]);

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .expect("expected package validation response");

        assert!(response.missing_runtime_capabilities);
        assert!(response
            .issues
            .iter()
            .any(|issue| issue.code == "missing_capability_for_runtime_api"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn warns_on_typescript_escape_hatch_and_custom_runner() {
        let dir = temp_package_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/case-a")).unwrap();
        fs::write(dir.join("tests/run.mjs"), "console.log('runner');").unwrap();
        fs::write(
            dir.join("codemod.yaml"),
            r#"schema_version: "1.0"
name: "example"
version: "0.1.0"
description: "desc"
workflow: "workflow.yaml"
capabilities: []
"#,
        )
        .unwrap();
        fs::write(
            dir.join("workflow.yaml"),
            r#"version: "1"
nodes:
  - id: apply
    name: Apply
    type: automatic
    steps:
      - name: Run codemod
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: "typescript"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"name":"example","scripts":{"test":"node ./tests/run.mjs","check-types":"echo ok"}}"#,
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Example\n\nReal package.\n").unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            "// @ts-nocheck\nexport default async function transform() { return null; }\n",
        )
        .unwrap();
        fs::write(dir.join("tests/case-a/input.ts"), "foo();").unwrap();
        fs::write(dir.join("tests/case-a/expected.ts"), "foo();").unwrap();
        write_coverage_contract(&dir, &["shape-a"], &[("shape-a", &["case-a"])]);

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .expect("expected package validation response");

        assert!(response.typescript_escape_hatch_detected);
        assert!(response.nonstandard_test_runner_detected);
        assert!(response
            .issues
            .iter()
            .any(|issue| issue.code == "typescript_escape_hatch_detected"));
        assert!(response
            .issues
            .iter()
            .any(|issue| issue.code == "nonstandard_test_runner_detected"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn fails_when_supported_shape_has_no_fixture_mapping() {
        let dir = temp_package_dir();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("tests/case-a")).unwrap();
        fs::write(
            dir.join("codemod.yaml"),
            r#"schema_version: "1.0"
name: "example"
version: "0.1.0"
description: "desc"
workflow: "workflow.yaml"
capabilities: []
"#,
        )
        .unwrap();
        fs::write(
            dir.join("workflow.yaml"),
            r#"version: "1"
nodes:
  - id: apply
    name: Apply
    type: automatic
    steps:
      - name: Run codemod
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: "typescript"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"name":"example","scripts":{"test":"echo ok","check-types":"echo ok"}}"#,
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Example\n\nReusable package.\n").unwrap();
        fs::write(
            dir.join("scripts/codemod.ts"),
            "export default async function transform() { return null; }\n",
        )
        .unwrap();
        fs::write(dir.join("tests/case-a/input.ts"), "foo();").unwrap();
        fs::write(dir.join("tests/case-a/expected.ts"), "foo();").unwrap();
        write_coverage_contract(&dir, &["app/page.tsx", "app/contact/page.tsx"], &[("app/page.tsx", &["case-a"])]);

        let handler = PackageValidationHandler::new();
        let response = handler
            .validate_package(ValidateCodemodPackageRequest {
                package_path: Some(dir.display().to_string()),
                run_default_test: false,
                run_check_types: false,
                command_timeout_seconds: 5,
            })
            .await
            .expect("expected package validation response");

        assert!(!response.supported_shape_coverage_complete);
        assert!(response
            .issues
            .iter()
            .any(|issue| issue.code == "missing_supported_shape_coverage"));

        fs::remove_dir_all(dir).unwrap();
    }
}
