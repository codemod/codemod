use crate::auth::TokenStorage;
use crate::commands::harness_adapter::{
    install_restart_hint, persist_managed_install_state, resolve_adapter, resolve_install_scope,
    skill_discovery_guide_paths, upsert_skill_discovery_guides, Harness, HarnessAdapterError,
    InstallRequest, InstallScope, InstalledSkill, ManagedComponentKind, ManagedComponentSnapshot,
    ManagedStateWriteResult, ManagedStateWriteStatus, OutputFormat, VerificationStatus,
};
use crate::commands::output::{exit_adapter_error, format_output_path};
use anyhow::Result;
use clap::{Args, Subcommand};
use inquire::{Confirm, Select};
use serde::Serialize;
use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

const MANAGED_UPDATE_POLICY_TRIGGER: &str = "agent_install";
const MANAGED_UPDATE_POLICY_ENV_VAR: &str = "CODEMOD_AGENT_UPDATE_POLICY";
const MANAGED_UPDATE_REMOTE_SOURCE_ENV_VAR: &str = "CODEMOD_AGENT_UPDATE_REMOTE_SOURCE";
const MANAGED_UPDATE_POLICY_LOCAL_SOURCE: &str = "local_embedded_only";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdatePolicyMode {
    Manual,
    Notify,
    AutoSafe,
}

impl UpdatePolicyMode {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Notify => "notify",
            Self::AutoSafe => "auto-safe",
        }
    }
}

#[derive(Clone, Debug)]
struct UpdatePolicyContext {
    mode: UpdatePolicyMode,
    remote_source: String,
    fallback_applied: bool,
    warnings: Vec<String>,
}

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: AgentAction,
}

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Install MCS and baseline codemod skills into harness-specific paths
    Install(InstallCommand),
    /// List installed codemod skills for a harness
    List(ListCommand),
}

#[derive(Args, Debug)]
struct InstallCommand {
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
struct ListCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

pub async fn handler(args: &Command) -> Result<()> {
    match &args.action {
        AgentAction::Install(command) => {
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
            let verification_checks =
                resolved_adapter
                    .adapter
                    .verify_skills()
                    .unwrap_or_else(|error| {
                        exit_adapter_error(error, command.format);
                    });

            if let Some(failed_check) = verification_checks
                .iter()
                .find(|check| check.status == VerificationStatus::Fail)
            {
                let reason = failed_check
                    .reason
                    .as_ref()
                    .map(|text| format!(": {text}"))
                    .unwrap_or_default();
                exit_adapter_error(
                    HarnessAdapterError::InvalidSkillPackage(format!(
                        "installed skill `{}` failed validation{reason}",
                        failed_check.skill
                    )),
                    command.format,
                );
            }

            let update_policy = resolve_update_policy_context();
            let mut warnings = resolved_adapter.warnings;
            warnings.extend(update_policy.warnings.iter().cloned());

            match upsert_skill_discovery_guides(resolved_adapter.harness, install_inputs.scope) {
                Ok(updated_files) if !updated_files.is_empty() => warnings.push(format!(
                    "Updated discovery hints in: {}",
                    updated_files
                        .iter()
                        .map(|path| format_output_path(path))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                Ok(_) => {}
                Err(error) => warnings.push(format!(
                    "Installed skills, but failed to update AGENTS.md/CLAUDE.md discovery hints: {error}"
                )),
            }

            let discovery_paths = match skill_discovery_guide_paths(
                resolved_adapter.harness,
                install_inputs.scope,
            ) {
                Ok(paths) => paths,
                Err(error) => {
                    warnings.push(format!(
                            "Installed skills, but failed to resolve AGENTS.md/CLAUDE.md paths for managed-state tracking: {error}"
                        ));
                    Vec::new()
                }
            };

            let managed_components = managed_components_from_install(&installed, &discovery_paths);
            let managed_state = match persist_managed_install_state(
                resolved_adapter.harness,
                install_inputs.scope,
                &managed_components,
            ) {
                Ok(state_write) => Some(state_write),
                Err(error) => {
                    warnings.push(format!(
                        "Installed skills, but failed to persist managed install state: {error}"
                    ));
                    None
                }
            };
            if let Some(policy_runtime_message) =
                update_policy_runtime_message(&update_policy, managed_state.as_ref())
            {
                warnings.push(policy_runtime_message);
            }

            warnings.push(install_restart_hint(resolved_adapter.harness));

            let output = build_install_output(
                resolved_adapter.harness,
                install_inputs.scope,
                installed,
                managed_state,
                &update_policy,
                warnings,
            );
            print_install_output(&output, command.format)?;
            Ok(())
        }
        AgentAction::List(command) => {
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
    }
}

#[derive(Serialize)]
struct InstallSkillsOutput {
    ok: bool,
    harness: String,
    scope: String,
    installed: Vec<InstalledSkillOutput>,
    managed_state: Option<ManagedStateOutput>,
    update_policy: UpdatePolicyOutput,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ManagedStateOutput {
    path: String,
    status: String,
}

#[derive(Serialize)]
struct UpdatePolicyOutput {
    mode: String,
    trigger: String,
    behavior: String,
    remote_source: String,
    fallback_applied: bool,
}

#[derive(Serialize)]
struct InstalledSkillOutput {
    name: String,
    path: String,
    version: Option<String>,
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

fn build_install_output(
    harness: Harness,
    scope: InstallScope,
    installed: Vec<InstalledSkill>,
    managed_state: Option<ManagedStateWriteResult>,
    update_policy: &UpdatePolicyContext,
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
        managed_state: managed_state.map(|state| ManagedStateOutput {
            path: format_output_path(&state.path),
            status: state.status.as_str().to_string(),
        }),
        update_policy: build_update_policy_output(update_policy),
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
    if let Some(state) = &output.managed_state {
        println!("Managed state: {} ({})", state.path, state.status);
    } else {
        println!("Managed state: unavailable");
    }
    println!(
        "Update policy: {} (trigger: {}, source: {}, fallback: {})",
        output.update_policy.mode,
        output.update_policy.trigger,
        output.update_policy.remote_source,
        output.update_policy.fallback_applied
    );

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

fn managed_components_from_install(
    installed: &[InstalledSkill],
    discovery_paths: &[PathBuf],
) -> Vec<ManagedComponentSnapshot> {
    let mut components = installed
        .iter()
        .map(|entry| ManagedComponentSnapshot {
            id: entry.name.clone(),
            kind: managed_component_kind_from_install_entry(entry),
            path: entry.path.clone(),
            version: entry.version.clone(),
        })
        .collect::<Vec<_>>();

    for discovery_path in discovery_paths {
        let component_id = discovery_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| format!("discovery-guide:{name}"))
            .unwrap_or_else(|| format!("discovery-guide:{}", discovery_path.to_string_lossy()));

        components.push(ManagedComponentSnapshot {
            id: component_id,
            kind: ManagedComponentKind::DiscoveryGuide,
            path: discovery_path.clone(),
            version: None,
        });
    }

    components
}

fn managed_component_kind_from_install_entry(entry: &InstalledSkill) -> ManagedComponentKind {
    if entry.name == "codemod-mcp" {
        ManagedComponentKind::McpConfig
    } else {
        ManagedComponentKind::Skill
    }
}

fn resolve_update_policy_context() -> UpdatePolicyContext {
    let (mode, mode_warning) = resolve_update_policy_mode();
    let (remote_source, remote_source_warning) = resolve_update_remote_source();
    let mut warnings = Vec::new();
    if let Some(warning) = mode_warning {
        warnings.push(warning);
    }
    if let Some(warning) = remote_source_warning {
        warnings.push(warning);
    }

    let fallback_applied = mode != UpdatePolicyMode::Manual;
    if fallback_applied {
        warnings.push(format!(
            "Update policy `{}` requested, but remote update lookup is not implemented yet; applying deterministic local fallback.",
            mode.as_str()
        ));
    }

    UpdatePolicyContext {
        mode,
        remote_source,
        fallback_applied,
        warnings,
    }
}

fn resolve_update_policy_mode() -> (UpdatePolicyMode, Option<String>) {
    let raw_value = match std::env::var(MANAGED_UPDATE_POLICY_ENV_VAR) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => {
            return (UpdatePolicyMode::Manual, None);
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return (
                UpdatePolicyMode::Manual,
                Some(format!(
                    "Invalid {} value (non-unicode). Falling back to `manual` policy.",
                    MANAGED_UPDATE_POLICY_ENV_VAR
                )),
            );
        }
    };

    match parse_update_policy_mode(&raw_value) {
        Some(mode) => (mode, None),
        None => (
            UpdatePolicyMode::Manual,
            Some(format!(
                "Unsupported {} value `{}`. Supported values: manual, notify, auto-safe. Falling back to `manual` policy.",
                MANAGED_UPDATE_POLICY_ENV_VAR, raw_value
            )),
        ),
    }
}

fn resolve_update_remote_source() -> (String, Option<String>) {
    let env_override = match std::env::var(MANAGED_UPDATE_REMOTE_SOURCE_ENV_VAR) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return (
                MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
                Some(format!(
                    "Invalid {} value (non-unicode). Falling back to `{}`.",
                    MANAGED_UPDATE_REMOTE_SOURCE_ENV_VAR, MANAGED_UPDATE_POLICY_LOCAL_SOURCE
                )),
            );
        }
    };

    if let Some(raw_override) = env_override {
        return parse_update_remote_source_override(&raw_override);
    }

    match resolve_default_registry_source() {
        Ok(source) => (source, None),
        Err(error) => (
            MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
            Some(format!(
                "Failed to resolve default registry update source ({error}). Falling back to `{}`.",
                MANAGED_UPDATE_POLICY_LOCAL_SOURCE
            )),
        ),
    }
}

fn parse_update_remote_source_override(raw_override: &str) -> (String, Option<String>) {
    let normalized = raw_override.trim();
    if normalized.is_empty() {
        return (
            MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
            Some(format!(
                "Empty {} value. Falling back to `{}`.",
                MANAGED_UPDATE_REMOTE_SOURCE_ENV_VAR, MANAGED_UPDATE_POLICY_LOCAL_SOURCE
            )),
        );
    }

    let normalized_lower = normalized.to_ascii_lowercase();
    if normalized_lower == "local" || normalized_lower == "embedded" {
        return (MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(), None);
    }

    if normalized_lower == "registry" {
        return match resolve_default_registry_source() {
            Ok(source) => (source, None),
            Err(error) => (
                MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
                Some(format!(
                    "Could not resolve registry update source ({error}). Falling back to `{}`.",
                    MANAGED_UPDATE_POLICY_LOCAL_SOURCE
                )),
            ),
        };
    }

    match url::Url::parse(normalized) {
        Ok(parsed) => (format!("url:{parsed}"), None),
        Err(error) => (
            MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
            Some(format!(
                "Unsupported {} value `{}` ({error}). Use `local`, `registry`, or an absolute URL. Falling back to `{}`.",
                MANAGED_UPDATE_REMOTE_SOURCE_ENV_VAR,
                normalized,
                MANAGED_UPDATE_POLICY_LOCAL_SOURCE
            )),
        ),
    }
}

fn resolve_default_registry_source() -> std::result::Result<String, String> {
    let storage = TokenStorage::new()
        .map_err(|error| format!("failed to initialize token storage: {error}"))?;
    let config = storage
        .load_config()
        .map_err(|error| format!("failed to load CLI config: {error}"))?;
    let registry_url = config.default_registry.trim();
    if registry_url.is_empty() {
        return Err("default registry is empty".to_string());
    }

    let parsed = url::Url::parse(registry_url)
        .map_err(|error| format!("invalid default registry URL `{registry_url}`: {error}"))?;
    Ok(format!("registry:{parsed}"))
}

fn parse_update_policy_mode(raw_value: &str) -> Option<UpdatePolicyMode> {
    let normalized = raw_value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" => None,
        "manual" => Some(UpdatePolicyMode::Manual),
        "notify" => Some(UpdatePolicyMode::Notify),
        "auto-safe" | "autosafe" | "auto_safe" => Some(UpdatePolicyMode::AutoSafe),
        _ => None,
    }
}

fn build_update_policy_output(context: &UpdatePolicyContext) -> UpdatePolicyOutput {
    UpdatePolicyOutput {
        mode: context.mode.as_str().to_string(),
        trigger: MANAGED_UPDATE_POLICY_TRIGGER.to_string(),
        behavior: update_policy_behavior(context).to_string(),
        remote_source: context.remote_source.clone(),
        fallback_applied: context.fallback_applied,
    }
}

fn update_policy_behavior(context: &UpdatePolicyContext) -> &'static str {
    match context.mode {
        UpdatePolicyMode::Manual => "reconcile_on_install",
        UpdatePolicyMode::Notify if context.fallback_applied => {
            "notify_on_local_state_change_fallback"
        }
        UpdatePolicyMode::Notify => "notify_on_remote_or_local_change",
        UpdatePolicyMode::AutoSafe if context.fallback_applied => "local_auto_reconcile_fallback",
        UpdatePolicyMode::AutoSafe => "auto_reconcile_with_remote_checks",
    }
}

fn update_policy_runtime_message(
    context: &UpdatePolicyContext,
    managed_state: Option<&ManagedStateWriteResult>,
) -> Option<String> {
    match context.mode {
        UpdatePolicyMode::Manual => None,
        UpdatePolicyMode::Notify => Some(match managed_state.map(|state| state.status) {
            Some(ManagedStateWriteStatus::Created) => {
                "Update policy notify: codemod-managed state was created in this install (local fallback active)."
                    .to_string()
            }
            Some(ManagedStateWriteStatus::Updated) => {
                "Update policy notify: codemod-managed state changed in this install (local fallback active)."
                    .to_string()
            }
            Some(ManagedStateWriteStatus::Unchanged) => {
                "Update policy notify: no codemod-managed state change detected (local fallback active)."
                    .to_string()
            }
            None => "Update policy notify: managed state is unavailable; change notifications are limited.".to_string(),
        }),
        UpdatePolicyMode::AutoSafe => Some(format!(
            "Update policy auto-safe: applied local safe reconcile fallback (remote source hint: {}).",
            context.remote_source
        )),
    }
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
    command: &InstallCommand,
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
    use serde_json::Value;
    use std::path::PathBuf;

    #[test]
    fn install_output_json_includes_codemod_mcp_entry() {
        let update_policy = UpdatePolicyContext {
            mode: UpdatePolicyMode::Manual,
            remote_source: MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
            fallback_applied: false,
            warnings: Vec::new(),
        };
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
            Some(ManagedStateWriteResult {
                path: PathBuf::from("/tmp/.claude/codemod/managed-install-state.json"),
                status: crate::commands::harness_adapter::ManagedStateWriteStatus::Created,
            }),
            &update_policy,
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
        assert_eq!(
            output_json
                .get("managed_state")
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str),
            Some("created")
        );
        assert_eq!(
            output_json
                .get("update_policy")
                .and_then(|value| value.get("mode"))
                .and_then(Value::as_str),
            Some("manual")
        );
        assert_eq!(
            output_json
                .get("update_policy")
                .and_then(|value| value.get("behavior"))
                .and_then(Value::as_str),
            Some("reconcile_on_install")
        );
        assert_eq!(
            output_json
                .get("update_policy")
                .and_then(|value| value.get("remote_source"))
                .and_then(Value::as_str),
            Some("local_embedded_only")
        );
        assert_eq!(
            output_json
                .get("update_policy")
                .and_then(|value| value.get("fallback_applied"))
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            output_json
                .get("update_policy")
                .and_then(|value| value.get("trigger"))
                .and_then(Value::as_str),
            Some("agent_install")
        );
        assert!(codemod_mcp.get("path").and_then(Value::as_str).is_some());
        assert!(codemod_mcp.get("version").is_some_and(Value::is_null));
    }

    #[test]
    fn managed_components_include_discovery_guides_and_mcp_kind() {
        let installed = vec![
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
        ];
        let discovery_paths = vec![
            PathBuf::from("/tmp/AGENTS.md"),
            PathBuf::from("/tmp/CLAUDE.md"),
        ];

        let components = managed_components_from_install(&installed, &discovery_paths);
        assert_eq!(components.len(), 4);

        let mcp_component = components
            .iter()
            .find(|component| component.id == "codemod-mcp")
            .expect("expected codemod-mcp managed component");
        assert_eq!(mcp_component.kind, ManagedComponentKind::McpConfig);

        let discovery_component = components
            .iter()
            .find(|component| component.id == "discovery-guide:AGENTS.md")
            .expect("expected AGENTS.md discovery component");
        assert_eq!(
            discovery_component.kind,
            ManagedComponentKind::DiscoveryGuide
        );
    }

    #[test]
    fn parse_update_policy_mode_accepts_supported_values() {
        assert_eq!(
            parse_update_policy_mode("manual"),
            Some(UpdatePolicyMode::Manual)
        );
        assert_eq!(
            parse_update_policy_mode("notify"),
            Some(UpdatePolicyMode::Notify)
        );
        assert_eq!(
            parse_update_policy_mode("auto-safe"),
            Some(UpdatePolicyMode::AutoSafe)
        );
        assert_eq!(
            parse_update_policy_mode("auto_safe"),
            Some(UpdatePolicyMode::AutoSafe)
        );
        assert_eq!(
            parse_update_policy_mode("autosafe"),
            Some(UpdatePolicyMode::AutoSafe)
        );
        assert_eq!(parse_update_policy_mode(""), None);
        assert_eq!(parse_update_policy_mode("unknown"), None);
    }

    #[test]
    fn update_policy_runtime_message_notify_reflects_state_status() {
        let updated_state = ManagedStateWriteResult {
            path: PathBuf::from("/tmp/managed.json"),
            status: crate::commands::harness_adapter::ManagedStateWriteStatus::Updated,
        };
        let unchanged_state = ManagedStateWriteResult {
            path: PathBuf::from("/tmp/managed.json"),
            status: crate::commands::harness_adapter::ManagedStateWriteStatus::Unchanged,
        };
        let notify_context = UpdatePolicyContext {
            mode: UpdatePolicyMode::Notify,
            remote_source: "registry:https://app.codemod.com/".to_string(),
            fallback_applied: true,
            warnings: Vec::new(),
        };
        let autosafe_context = UpdatePolicyContext {
            mode: UpdatePolicyMode::AutoSafe,
            remote_source: "registry:https://app.codemod.com/".to_string(),
            fallback_applied: true,
            warnings: Vec::new(),
        };
        let manual_context = UpdatePolicyContext {
            mode: UpdatePolicyMode::Manual,
            remote_source: MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
            fallback_applied: false,
            warnings: Vec::new(),
        };

        assert!(
            update_policy_runtime_message(&notify_context, Some(&updated_state))
                .unwrap()
                .contains("local fallback")
        );
        assert!(
            update_policy_runtime_message(&notify_context, Some(&unchanged_state))
                .unwrap()
                .contains("no codemod-managed state change")
        );
        assert!(update_policy_runtime_message(&autosafe_context, None)
            .unwrap()
            .contains("remote source hint"));
        assert!(update_policy_runtime_message(&manual_context, None).is_none());
    }

    #[test]
    fn parse_update_remote_source_override_handles_local_registry_and_url() {
        let (local_source, local_warning) = parse_update_remote_source_override("local");
        assert_eq!(local_source, MANAGED_UPDATE_POLICY_LOCAL_SOURCE);
        assert!(local_warning.is_none());

        let (url_source, url_warning) =
            parse_update_remote_source_override("https://updates.codemod.com");
        assert_eq!(url_source, "url:https://updates.codemod.com/");
        assert!(url_warning.is_none());

        let (registry_source, registry_warning) = parse_update_remote_source_override("registry");
        if registry_warning.is_none() {
            assert!(registry_source.starts_with("registry:"));
        } else {
            assert_eq!(registry_source, MANAGED_UPDATE_POLICY_LOCAL_SOURCE);
        }
    }

    #[test]
    fn parse_update_remote_source_override_falls_back_on_invalid_values() {
        let (source, warning) = parse_update_remote_source_override("not a url");
        assert_eq!(source, MANAGED_UPDATE_POLICY_LOCAL_SOURCE);
        assert!(warning
            .as_deref()
            .is_some_and(|text| text.contains("Unsupported")));

        let (empty_source, empty_warning) = parse_update_remote_source_override("  ");
        assert_eq!(empty_source, MANAGED_UPDATE_POLICY_LOCAL_SOURCE);
        assert!(empty_warning
            .as_deref()
            .is_some_and(|text| text.contains("Empty")));
    }
}
