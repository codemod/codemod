use crate::commands::harness_adapter::{
    resolve_adapter, resolve_install_scope, Harness, HarnessAdapterError, InstallRequest,
    InstallScope, InstalledSkill, OutputFormat, VerificationCheck, VerificationStatus,
};
use crate::suitability::{
    search_registry, summarize_search_coverage, RegistrySearchRequest, SearchCoverageSummary,
};
use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Args, Subcommand};
use inquire::{Confirm, Select};
use serde::Serialize;
use std::fmt;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};
use uuid::Uuid;

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: AgentAction,
}

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Install MCS and baseline codemod skills into harness-specific paths
    InstallSkills(InstallSkillsCommand),
    /// Validate skill metadata, paths, and compatibility markers
    VerifySkills(VerifySkillsCommand),
    /// List installed codemod skills for a harness
    ListSkills(ListSkillsCommand),
    /// Run MCS orchestration for a natural-language migration intent
    Run(RunCommand),
}

#[derive(Args, Debug)]
struct InstallSkillsCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Prompt for missing install options in an interactive wizard
    #[arg(long)]
    interactive: bool,
    /// Install into current repo workspace
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Install into user-level skills path
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// Overwrite existing skill files
    #[arg(long)]
    force: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct VerifySkillsCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct ListSkillsCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct RunCommand {
    /// Natural language migration intent
    #[arg(value_name = "INTENT")]
    intent: String,
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Existing session identifier
    #[arg(long)]
    session: Option<String>,
    /// Directory used for run artifacts
    #[arg(long)]
    artifacts_dir: Option<PathBuf>,
    /// Max orchestration iterations
    #[arg(long)]
    max_iterations: Option<u32>,
    /// Stop after dry-run planning phase
    #[arg(long)]
    dry_run_only: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

pub async fn handler(args: &Command) -> Result<()> {
    match &args.action {
        AgentAction::InstallSkills(command) => {
            let install_inputs = resolve_install_inputs(command).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let resolved_adapter =
                resolve_adapter(install_inputs.harness).unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let _ = resolved_adapter.adapter.metadata();
            let request = InstallRequest {
                scope: install_inputs.scope,
                force: install_inputs.force,
            };
            let installed = resolved_adapter
                .adapter
                .install_skills(&request)
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });

            let output = build_install_output(
                resolved_adapter.harness,
                install_inputs.scope,
                installed,
                resolved_adapter.warnings,
            );
            print_install_output(&output, command.format)?;
            Ok(())
        }
        AgentAction::VerifySkills(command) => {
            let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let _ = resolved_adapter.adapter.metadata();
            let checks = resolved_adapter
                .adapter
                .verify_skills()
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let output =
                build_verify_output(resolved_adapter.harness, checks, resolved_adapter.warnings);
            print_verify_output(&output, command.format)?;

            if !output.ok {
                std::process::exit(23);
            }

            Ok(())
        }
        AgentAction::ListSkills(command) => {
            let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let _ = resolved_adapter.adapter.metadata();
            let listed_skills = resolved_adapter
                .adapter
                .list_skills()
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let output = build_list_output(
                resolved_adapter.harness,
                listed_skills,
                resolved_adapter.warnings,
            );
            print_list_output(&output, command.format)?;
            Ok(())
        }
        AgentAction::Run(command) => {
            let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let _ = resolved_adapter.adapter.metadata();

            let output = execute_run(command, resolved_adapter.harness, resolved_adapter.warnings)
                .await
                .unwrap_or_else(|error| exit_run_error(error, command.format));
            print_run_output(&output, command.format)?;
            Ok(())
        }
    }
}

#[derive(Serialize)]
struct InstallSkillsOutput {
    ok: bool,
    harness: String,
    scope: String,
    installed: Vec<InstalledSkillOutput>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct InstalledSkillOutput {
    name: String,
    path: String,
    version: Option<String>,
}

#[derive(Serialize)]
struct InstallErrorEnvelope {
    ok: bool,
    code: String,
    exit_code: i32,
    message: String,
    hint: String,
}

#[derive(Tabled)]
struct InstalledSkillRow {
    #[tabled(rename = "Skill")]
    name: String,
    #[tabled(rename = "Version")]
    version: String,
    #[tabled(rename = "Path")]
    path: String,
}

#[derive(Serialize)]
struct ListSkillsOutput {
    ok: bool,
    harness: String,
    skills: Vec<ListedSkillOutput>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ListedSkillOutput {
    name: String,
    scope: Option<String>,
    path: String,
    version: Option<String>,
}

#[derive(Serialize)]
struct VerifySkillsOutput {
    ok: bool,
    harness: String,
    checks: Vec<VerifySkillCheckOutput>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct VerifySkillCheckOutput {
    skill: String,
    scope: Option<String>,
    status: String,
    reason: Option<String>,
}

#[derive(Tabled)]
struct ListedSkillRow {
    #[tabled(rename = "Skill")]
    name: String,
    #[tabled(rename = "Scope")]
    scope: String,
    #[tabled(rename = "Version")]
    version: String,
    #[tabled(rename = "Path")]
    path: String,
}

#[derive(Tabled)]
struct VerifySkillRow {
    #[tabled(rename = "Skill")]
    skill: String,
    #[tabled(rename = "Scope")]
    scope: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Reason")]
    reason: String,
}

#[derive(Serialize, Clone)]
struct RunOutput {
    ok: bool,
    session_id: String,
    intent: String,
    harness: String,
    registry: String,
    artifacts_dir: String,
    decision: RunDecisionOutput,
    metadata_coverage: SearchCoverageSummary,
    candidate_evaluation_path: String,
    warnings: Vec<String>,
}

#[derive(Serialize, Clone)]
struct RunDecisionOutput {
    kind: String,
    reason: String,
    ready_for_threshold_routing: bool,
    missing_fields: Vec<String>,
}

#[derive(Serialize)]
struct CandidateEvaluationArtifact {
    schema_version: String,
    generated_at: String,
    session_id: String,
    intent: String,
    harness: String,
    registry: String,
    decision: RunDecisionOutput,
    metadata_coverage: SearchCoverageSummary,
}

#[derive(Serialize)]
struct RunErrorEnvelope {
    ok: bool,
    code: String,
    exit_code: i32,
    message: String,
    hint: String,
}

#[derive(Tabled)]
struct RunDecisionRow {
    #[tabled(rename = "Decision")]
    kind: String,
    #[tabled(rename = "Ready")]
    ready: String,
    #[tabled(rename = "Reason")]
    reason: String,
    #[tabled(rename = "Missing Fields")]
    missing_fields: String,
}

async fn execute_run(
    command: &RunCommand,
    resolved_harness: Harness,
    mut warnings: Vec<String>,
) -> Result<RunOutput> {
    let search_request = RegistrySearchRequest {
        query: Some(command.intent.clone()),
        size: 20,
        from: 0,
        ..RegistrySearchRequest::default()
    };
    let search_result = search_registry(search_request)
        .await
        .context("failed to query registry for agent run preflight")?;
    let metadata_coverage = summarize_search_coverage(&search_result.response.packages);
    let decision = build_run_decision(&metadata_coverage);

    if decision.kind == "insufficient_metadata" {
        warnings.push(
            "routing degraded: registry payload is missing suitability contract fields".to_string(),
        );
    }
    if command.max_iterations.is_some() {
        warnings.push(
            "--max-iterations is accepted, but preflight-only run mode does not execute routing yet"
                .to_string(),
        );
    }
    if command.dry_run_only {
        warnings.push(
            "--dry-run-only acknowledged; current implementation performs preflight only"
                .to_string(),
        );
    }

    let session_id = resolve_session_id(command.session.as_deref());
    let artifacts_dir = resolve_run_artifacts_dir(command.artifacts_dir.as_deref(), &session_id);
    fs::create_dir_all(&artifacts_dir).with_context(|| {
        format!(
            "failed to create artifacts directory `{}`",
            artifacts_dir.display()
        )
    })?;

    let artifact = CandidateEvaluationArtifact {
        schema_version: "1.0.0".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        session_id: session_id.clone(),
        intent: command.intent.clone(),
        harness: resolved_harness.as_str().to_string(),
        registry: search_result.registry_url.clone(),
        decision: decision.clone(),
        metadata_coverage: metadata_coverage.clone(),
    };
    let candidate_evaluation_path = write_candidate_evaluation_artifact(&artifacts_dir, &artifact)?;

    Ok(RunOutput {
        ok: true,
        session_id,
        intent: command.intent.clone(),
        harness: resolved_harness.as_str().to_string(),
        registry: search_result.registry_url,
        artifacts_dir: format_output_path(&artifacts_dir),
        decision,
        metadata_coverage,
        candidate_evaluation_path: format_output_path(&candidate_evaluation_path),
        warnings,
    })
}

fn build_run_decision(metadata_coverage: &SearchCoverageSummary) -> RunDecisionOutput {
    let mut missing_fields = metadata_coverage
        .missing_field_counts
        .iter()
        .filter_map(|(field, count)| (*count > 0).then_some((*field).to_string()))
        .collect::<Vec<_>>();
    missing_fields.sort();

    if metadata_coverage.total_packages == 0 {
        return RunDecisionOutput {
            kind: "no_candidates".to_string(),
            reason: "Registry search returned no candidates for the provided intent.".to_string(),
            ready_for_threshold_routing: false,
            missing_fields: Vec::new(),
        };
    }

    if metadata_coverage.packages_missing_contract_fields > 0 {
        return RunDecisionOutput {
            kind: "insufficient_metadata".to_string(),
            reason: "Registry candidates are missing required suitability contract fields; threshold routing skipped.".to_string(),
            ready_for_threshold_routing: false,
            missing_fields,
        };
    }

    RunDecisionOutput {
        kind: "ready_for_threshold_routing".to_string(),
        reason: "All required suitability fields are present; threshold routing can proceed."
            .to_string(),
        ready_for_threshold_routing: true,
        missing_fields,
    }
}

fn resolve_session_id(existing_session: Option<&str>) -> String {
    if let Some(session) = existing_session {
        let trimmed = session.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let entropy = Uuid::new_v4().simple().to_string();
    let suffix = &entropy[..6];
    format!("cmod-{timestamp}-{suffix}")
}

fn resolve_run_artifacts_dir(override_dir: Option<&Path>, session_id: &str) -> PathBuf {
    match override_dir {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from(".codemod-cli")
            .join("sessions")
            .join(session_id),
    }
}

fn write_candidate_evaluation_artifact(
    artifacts_dir: &Path,
    artifact: &CandidateEvaluationArtifact,
) -> Result<PathBuf> {
    let artifact_path = artifacts_dir.join("candidate-evaluation.json");
    let payload = serde_json::to_string_pretty(artifact)
        .context("failed to serialize candidate evaluation artifact")?;
    fs::write(&artifact_path, format!("{payload}\n")).with_context(|| {
        format!(
            "failed to write candidate evaluation artifact `{}`",
            artifact_path.display()
        )
    })?;
    Ok(artifact_path)
}

fn print_run_output(output: &RunOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(output)?),
        OutputFormat::Yaml => println!("{}", serde_yaml::to_string(output)?),
        OutputFormat::Table => print_run_output_table(output),
    }
    Ok(())
}

fn print_run_output_table(output: &RunOutput) {
    println!("Session: {}", output.session_id);
    println!("Harness: {}", output.harness);
    println!("Registry: {}", output.registry);
    println!("Artifacts: {}", output.artifacts_dir);

    let row = RunDecisionRow {
        kind: output.decision.kind.clone(),
        ready: output.decision.ready_for_threshold_routing.to_string(),
        reason: output.decision.reason.clone(),
        missing_fields: if output.decision.missing_fields.is_empty() {
            "-".to_string()
        } else {
            output.decision.missing_fields.join(", ")
        },
    };

    let mut table = Table::new(vec![row]);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");

    println!(
        "Candidate evaluation artifact: {}",
        output.candidate_evaluation_path
    );

    if !output.warnings.is_empty() {
        println!("Warnings:");
        for warning in &output.warnings {
            println!("  - {warning}");
        }
    }
}

fn build_install_output(
    harness: Harness,
    scope: InstallScope,
    installed: Vec<InstalledSkill>,
    warnings: Vec<String>,
) -> InstallSkillsOutput {
    InstallSkillsOutput {
        ok: true,
        harness: harness.as_str().to_string(),
        scope: scope.as_str().to_string(),
        installed: installed
            .into_iter()
            .map(|skill| InstalledSkillOutput {
                name: skill.name,
                path: format_output_path(&skill.path),
                version: skill.version,
            })
            .collect(),
        warnings,
    }
}

fn print_install_output(output: &InstallSkillsOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(output)?);
        }
        OutputFormat::Table => {
            print_install_output_table(output);
        }
    }

    Ok(())
}

fn print_install_output_table(output: &InstallSkillsOutput) {
    println!("Harness: {}", output.harness);
    println!("Scope: {}", output.scope);

    if !output.warnings.is_empty() {
        println!("Warnings:");
        for warning in &output.warnings {
            println!("  - {warning}");
        }
    }

    if output.installed.is_empty() {
        println!("No skills were installed.");
        return;
    }

    let rows = output
        .installed
        .iter()
        .map(|installed_skill| InstalledSkillRow {
            name: installed_skill.name.clone(),
            version: installed_skill
                .version
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            path: installed_skill.path.clone(),
        })
        .collect::<Vec<_>>();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");
}

fn build_list_output(
    harness: Harness,
    listed_skills: Vec<InstalledSkill>,
    warnings: Vec<String>,
) -> ListSkillsOutput {
    ListSkillsOutput {
        ok: true,
        harness: harness.as_str().to_string(),
        skills: listed_skills
            .into_iter()
            .map(|skill| ListedSkillOutput {
                name: skill.name,
                scope: skill.scope.map(|scope| scope.as_str().to_string()),
                path: format_output_path(&skill.path),
                version: skill.version,
            })
            .collect(),
        warnings,
    }
}

fn print_list_output(output: &ListSkillsOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(output)?);
        }
        OutputFormat::Table => {
            print_list_output_table(output);
        }
    }

    Ok(())
}

fn print_list_output_table(output: &ListSkillsOutput) {
    println!("Harness: {}", output.harness);
    if !output.warnings.is_empty() {
        println!("Warnings:");
        for warning in &output.warnings {
            println!("  - {warning}");
        }
    }

    if output.skills.is_empty() {
        println!("No codemod skills found.");
        return;
    }

    let rows = output
        .skills
        .iter()
        .map(|skill| ListedSkillRow {
            name: skill.name.clone(),
            scope: skill.scope.clone().unwrap_or_else(|| "unknown".to_string()),
            version: skill
                .version
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            path: skill.path.clone(),
        })
        .collect::<Vec<_>>();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");
}

fn build_verify_output(
    harness: Harness,
    checks: Vec<VerificationCheck>,
    warnings: Vec<String>,
) -> VerifySkillsOutput {
    let normalized_checks = checks
        .into_iter()
        .map(|check| VerifySkillCheckOutput {
            skill: check.skill,
            scope: check.scope.map(|scope| scope.as_str().to_string()),
            status: match check.status {
                VerificationStatus::Pass => "pass".to_string(),
                VerificationStatus::Fail => "fail".to_string(),
            },
            reason: check.reason,
        })
        .collect::<Vec<_>>();

    let ok = normalized_checks.iter().all(|check| check.status == "pass");

    VerifySkillsOutput {
        ok,
        harness: harness.as_str().to_string(),
        checks: normalized_checks,
        warnings,
    }
}

fn print_verify_output(output: &VerifySkillsOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(output)?);
        }
        OutputFormat::Table => {
            print_verify_output_table(output);
        }
    }

    Ok(())
}

fn print_verify_output_table(output: &VerifySkillsOutput) {
    println!("Harness: {}", output.harness);
    if !output.warnings.is_empty() {
        println!("Warnings:");
        for warning in &output.warnings {
            println!("  - {warning}");
        }
    }

    if output.checks.is_empty() {
        println!("No codemod skills found to verify.");
        return;
    }

    let rows = output
        .checks
        .iter()
        .map(|check| VerifySkillRow {
            skill: check.skill.clone(),
            scope: check.scope.clone().unwrap_or_else(|| "unknown".to_string()),
            status: check.status.clone(),
            reason: check.reason.clone().unwrap_or_else(|| "-".to_string()),
        })
        .collect::<Vec<_>>();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");
}

fn exit_adapter_error(error: HarnessAdapterError, format: OutputFormat) -> ! {
    let envelope = InstallErrorEnvelope {
        ok: false,
        code: error.code().to_string(),
        exit_code: error.exit_code(),
        message: error.to_string(),
        hint: error.hint().to_string(),
    };

    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(&envelope) {
            Ok(json) => println!("{json}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Yaml => match serde_yaml::to_string(&envelope) {
            Ok(yaml) => println!("{yaml}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Table => {
            eprintln!("Error [{}]: {}", envelope.code, envelope.message);
            eprintln!("Hint: {}", envelope.hint);
        }
    }

    std::process::exit(envelope.exit_code);
}

fn exit_run_error(error: anyhow::Error, format: OutputFormat) -> ! {
    let envelope = RunErrorEnvelope {
        ok: false,
        code: "AICLI_RUN_PREFLIGHT_FAILED".to_string(),
        exit_code: 25,
        message: error.to_string(),
        hint: "Retry with --format json to inspect metadata coverage and missing fields; full threshold routing requires registry suitability contract fields.".to_string(),
    };

    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(&envelope) {
            Ok(json) => println!("{json}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Yaml => match serde_yaml::to_string(&envelope) {
            Ok(yaml) => println!("{yaml}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Table => {
            eprintln!("Error [{}]: {}", envelope.code, envelope.message);
            eprintln!("Hint: {}", envelope.hint);
        }
    }

    std::process::exit(envelope.exit_code);
}

fn format_output_path(path: &std::path::Path) -> String {
    if let Ok(current_dir) = std::env::current_dir() {
        if let Ok(relative_path) = path.strip_prefix(current_dir) {
            return relative_path.display().to_string();
        }
    }

    if let Some(home_dir) = dirs::home_dir() {
        if let Ok(home_relative_path) = path.strip_prefix(home_dir) {
            return format!("~/{}", home_relative_path.display());
        }
    }

    path.display().to_string()
}

#[derive(Clone, Copy)]
struct InstallInputs {
    harness: Harness,
    scope: InstallScope,
    force: bool,
}

#[derive(Clone, Copy)]
struct HarnessPromptOption {
    harness: Harness,
    label: &'static str,
}

impl fmt::Display for HarnessPromptOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

#[derive(Clone, Copy)]
struct ScopePromptOption {
    scope: InstallScope,
    label: &'static str,
}

impl fmt::Display for ScopePromptOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

fn resolve_install_inputs(
    command: &InstallSkillsCommand,
) -> std::result::Result<InstallInputs, HarnessAdapterError> {
    if !command.interactive {
        let scope = resolve_install_scope(command.project, command.user)?;
        return Ok(InstallInputs {
            harness: command.harness,
            scope,
            force: command.force,
        });
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(HarnessAdapterError::InstallFailed(
            "--interactive requires a TTY terminal; re-run without --interactive in CI/headless environments".to_string(),
        ));
    }

    let harness = if command.harness != Harness::Auto {
        command.harness
    } else {
        let options = vec![
            HarnessPromptOption {
                harness: Harness::Auto,
                label: "auto (recommended)",
            },
            HarnessPromptOption {
                harness: Harness::Claude,
                label: "claude",
            },
            HarnessPromptOption {
                harness: Harness::Goose,
                label: "goose",
            },
            HarnessPromptOption {
                harness: Harness::Opencode,
                label: "opencode",
            },
            HarnessPromptOption {
                harness: Harness::Cursor,
                label: "cursor",
            },
        ];

        Select::new("Choose harness adapter:", options)
            .with_starting_cursor(0)
            .prompt()
            .map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "interactive harness prompt failed: {error}"
                ))
            })?
            .harness
    };

    let scope = if command.project || command.user {
        resolve_install_scope(command.project, command.user)?
    } else {
        let options = vec![
            ScopePromptOption {
                scope: InstallScope::Project,
                label: "project (current workspace)",
            },
            ScopePromptOption {
                scope: InstallScope::User,
                label: "user (~/.<harness>/skills)",
            },
        ];

        Select::new("Choose install scope:", options)
            .with_starting_cursor(0)
            .prompt()
            .map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "interactive scope prompt failed: {error}"
                ))
            })?
            .scope
    };

    let force = if command.force {
        true
    } else {
        Confirm::new("Overwrite existing skill files if they already exist?")
            .with_default(false)
            .prompt()
            .map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "interactive overwrite prompt failed: {error}"
                ))
            })?
    };

    Ok(InstallInputs {
        harness,
        scope,
        force,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suitability::REQUIRED_SUITABILITY_FIELDS;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn install_output_json_includes_codemod_mcp_entry() {
        let output = build_install_output(
            Harness::Claude,
            InstallScope::Project,
            vec![
                InstalledSkill {
                    name: "codemod-cli".to_string(),
                    path: PathBuf::from("/tmp/.claude/skills/codemod-cli/SKILL.md"),
                    version: Some("1.0.0".to_string()),
                    scope: Some(InstallScope::Project),
                },
                InstalledSkill {
                    name: "codemod-mcp".to_string(),
                    path: PathBuf::from("/tmp/.mcp.json"),
                    version: None,
                    scope: Some(InstallScope::Project),
                },
            ],
            Vec::new(),
        );

        let output_json = serde_json::to_value(&output).expect("install output should serialize");
        let installed = output_json
            .get("installed")
            .and_then(Value::as_array)
            .expect("installed should be an array");

        let codemod_mcp = installed
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some("codemod-mcp"))
            .expect("expected codemod-mcp installed entry");

        assert_eq!(output_json.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            output_json.get("harness").and_then(Value::as_str),
            Some("claude")
        );
        assert_eq!(
            output_json.get("scope").and_then(Value::as_str),
            Some("project")
        );
        assert!(codemod_mcp.get("path").and_then(Value::as_str).is_some());
        assert!(codemod_mcp.get("version").is_some_and(Value::is_null));
    }

    fn sample_coverage_summary(
        total_packages: usize,
        packages_missing_contract_fields: usize,
        missing_fields: &[&'static str],
    ) -> SearchCoverageSummary {
        let mut missing_field_counts: BTreeMap<&'static str, usize> = REQUIRED_SUITABILITY_FIELDS
            .iter()
            .map(|field| (*field, 0usize))
            .collect();
        for field in missing_fields {
            missing_field_counts.insert(*field, 1);
        }

        SearchCoverageSummary {
            total_packages,
            required_fields: REQUIRED_SUITABILITY_FIELDS.to_vec(),
            packages_ready_for_threshold_routing: total_packages
                .saturating_sub(packages_missing_contract_fields),
            packages_missing_contract_fields,
            missing_field_counts,
        }
    }

    #[test]
    fn build_run_decision_returns_insufficient_metadata_when_fields_missing() {
        let coverage = sample_coverage_summary(2, 2, &["frameworks", "quality_score"]);
        let decision = build_run_decision(&coverage);

        assert_eq!(decision.kind, "insufficient_metadata");
        assert!(!decision.ready_for_threshold_routing);
        assert_eq!(decision.missing_fields, vec!["frameworks", "quality_score"]);
    }

    #[test]
    fn build_run_decision_returns_no_candidates_for_empty_search_result() {
        let coverage = sample_coverage_summary(0, 0, &[]);
        let decision = build_run_decision(&coverage);

        assert_eq!(decision.kind, "no_candidates");
        assert!(!decision.ready_for_threshold_routing);
        assert!(decision.missing_fields.is_empty());
    }

    #[test]
    fn write_candidate_evaluation_artifact_writes_expected_contract() {
        let temp_dir = tempdir().expect("tempdir should be created");
        let coverage = sample_coverage_summary(1, 1, &["frameworks"]);
        let decision = build_run_decision(&coverage);
        let artifact = CandidateEvaluationArtifact {
            schema_version: "1.0.0".to_string(),
            generated_at: "2026-02-20T00:00:00Z".to_string(),
            session_id: "cmod-test-session".to_string(),
            intent: "migrate jest to vitest".to_string(),
            harness: "claude".to_string(),
            registry: "https://app.codemod.com".to_string(),
            decision,
            metadata_coverage: coverage,
        };

        let artifact_path =
            write_candidate_evaluation_artifact(temp_dir.path(), &artifact).expect("artifact");

        let artifact_raw = std::fs::read_to_string(&artifact_path).expect("artifact should exist");
        let artifact_json: Value = serde_json::from_str(&artifact_raw).expect("json should parse");

        assert_eq!(
            artifact_json.get("session_id").and_then(Value::as_str),
            Some("cmod-test-session")
        );
        assert_eq!(
            artifact_json
                .get("decision")
                .and_then(Value::as_object)
                .and_then(|decision| decision.get("kind"))
                .and_then(Value::as_str),
            Some("insufficient_metadata")
        );
        assert_eq!(
            artifact_json
                .get("metadata_coverage")
                .and_then(Value::as_object)
                .and_then(|coverage| coverage.get("packages_missing_contract_fields"))
                .and_then(Value::as_u64),
            Some(1)
        );
    }
}
