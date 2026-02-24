use crate::commands::harness_adapter::{
    install_restart_hint, persist_managed_install_state, resolve_adapter, resolve_install_scope,
    skill_discovery_guide_paths, upsert_periodic_update_trigger, upsert_skill_discovery_guides,
    Harness, HarnessAdapterError, InstallRequest, InstallScope, InstalledSkill,
    ManagedComponentKind, ManagedComponentSnapshot, OutputFormat, PeriodicUpdatePolicy,
    VerificationStatus,
};
use crate::commands::output::{exit_adapter_error, format_output_path};
use crate::{TelemetrySenderMutex, CLI_VERSION};
use anyhow::Result;
use clap::{Args, Subcommand};
use codemod_telemetry::send_event::BaseEvent;
use inquire::{Confirm, Select};
use std::collections::HashMap;
use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;

mod update;

use update::auto_safe::maybe_apply_auto_safe_updates;
use update::output::{
    build_install_output, build_list_output, print_install_output, print_list_output,
    BuildInstallOutputInput,
};
use update::policy::{
    resolve_update_policy_context, UpdatePolicyResolveOptions, DEFAULT_UPDATE_SOURCE,
};
use update::reconcile::{build_component_reconcile_decisions, update_policy_runtime_message};
use update::types::{UpdatePolicyMode, MANAGED_UPDATE_POLICY_LOCAL_SOURCE};
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
    /// Periodic update policy used by installed harness hooks/plugins
    #[arg(long, value_enum, default_value_t = PeriodicUpdatePolicy::AutoSafe)]
    periodic_policy: PeriodicUpdatePolicy,
    /// Update policy for this install execution
    #[arg(long, value_enum, default_value_t = UpdatePolicyMode::Manual)]
    update_policy: UpdatePolicyMode,
    /// Remote source for managed update metadata: local, registry, or absolute URL
    #[arg(long, default_value = DEFAULT_UPDATE_SOURCE)]
    update_source: String,
    /// Require signed remote manifests for this install execution
    #[arg(long, conflicts_with = "allow_unsigned_manifest")]
    require_signed_manifest: bool,
    /// Allow unsigned remote manifests for this install execution
    #[arg(long, conflicts_with = "require_signed_manifest")]
    allow_unsigned_manifest: bool,
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

pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    match &args.action {
        AgentAction::Install(command) => {
            let install_inputs = resolve_install_inputs(command).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let resolved_adapter =
                resolve_adapter(install_inputs.harness).unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
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

            let update_policy = resolve_update_policy_context(&UpdatePolicyResolveOptions {
                mode: install_inputs.update_policy,
                remote_source: install_inputs.update_source.clone(),
                require_signed_manifest: install_inputs.require_signed_manifest,
            })
            .await
            .unwrap_or_else(|error| {
                exit_adapter_error(
                    HarnessAdapterError::InstallFailed(format!(
                        "failed to resolve update policy: {error}"
                    )),
                    command.format,
                )
            });
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

            let periodic_trigger = match upsert_periodic_update_trigger(
                resolved_adapter.harness,
                install_inputs.scope,
                install_inputs.periodic_policy,
            ) {
                Ok(result) => {
                    if !result.updated_paths.is_empty() {
                        warnings.push(format!(
                            "Updated periodic update trigger artifacts in: {}",
                            result
                                .updated_paths
                                .iter()
                                .map(|path| format_output_path(path))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    warnings.extend(result.notes.iter().cloned());
                    Some(result)
                }
                Err(error) => {
                    warnings.push(format!(
                        "Installed skills, but failed to upsert periodic update triggers: {error}"
                    ));
                    None
                }
            };

            let periodic_trigger_paths = periodic_trigger
                .as_ref()
                .map(|result| result.tracked_paths.clone())
                .unwrap_or_default();
            let managed_components = managed_components_from_install(
                &installed,
                &discovery_paths,
                &periodic_trigger_paths,
            );
            let component_decisions = build_component_reconcile_decisions(
                &update_policy,
                resolved_adapter.harness,
                &managed_components,
            );
            let auto_safe_apply = maybe_apply_auto_safe_updates(
                &update_policy,
                &component_decisions,
                &managed_components,
            )
            .await;

            warnings.extend(auto_safe_apply.warnings.iter().cloned());
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
            if let Some(policy_runtime_message) = update_policy_runtime_message(
                &update_policy,
                managed_state.as_ref(),
                auto_safe_apply.result.as_ref(),
            ) {
                warnings.push(policy_runtime_message);
            }

            warnings.push(install_restart_hint(resolved_adapter.harness));

            let output = build_install_output(BuildInstallOutputInput {
                harness: resolved_adapter.harness,
                scope: install_inputs.scope,
                installed,
                managed_state,
                update_policy: &update_policy,
                component_decisions,
                auto_safe_apply: auto_safe_apply.result,
                warnings,
            });
            print_install_output(&output, command.format)?;
            send_agent_install_event(
                &telemetry,
                command.harness,
                resolved_adapter.harness,
                command,
                &install_inputs,
                &output,
            )
            .await;
            Ok(())
        }
        AgentAction::List(command) => {
            let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
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
            send_agent_list_event(
                &telemetry,
                command.harness,
                resolved_adapter.harness,
                command.format,
                output.skills.len(),
                output.warnings.len(),
            )
            .await;
            Ok(())
        }
    }
}

async fn send_agent_install_event(
    telemetry: &TelemetrySenderMutex,
    requested_harness: Harness,
    resolved_harness: Harness,
    command: &InstallCommand,
    inputs: &InstallInputs,
    output: &update::output::InstallSkillsOutput,
) {
    let auto_safe = output.update_policy.auto_safe_apply.as_ref();
    let require_signed_manifest = match inputs.require_signed_manifest {
        Some(true) => "true",
        Some(false) => "false",
        None => "default",
    };
    let update_source_kind =
        if output.update_policy.remote_source == MANAGED_UPDATE_POLICY_LOCAL_SOURCE {
            "local"
        } else if output.update_policy.remote_source.starts_with("registry:") {
            "registry"
        } else if output.update_policy.remote_source.starts_with("url:") {
            "url"
        } else {
            "unknown"
        };

    telemetry
        .send_event(
            BaseEvent {
                kind: "agentMcsInstalled".to_string(),
                properties: HashMap::from([
                    (
                        "commandName".to_string(),
                        "codemod.agent.install".to_string(),
                    ),
                    (
                        "requestedHarness".to_string(),
                        requested_harness.as_str().to_string(),
                    ),
                    (
                        "resolvedHarness".to_string(),
                        resolved_harness.as_str().to_string(),
                    ),
                    ("scope".to_string(), inputs.scope.as_str().to_string()),
                    ("interactive".to_string(), command.interactive.to_string()),
                    ("force".to_string(), inputs.force.to_string()),
                    (
                        "periodicPolicy".to_string(),
                        inputs.periodic_policy.as_str().to_string(),
                    ),
                    (
                        "updatePolicy".to_string(),
                        inputs.update_policy.as_str().to_string(),
                    ),
                    (
                        "requireSignedManifest".to_string(),
                        require_signed_manifest.to_string(),
                    ),
                    (
                        "updateSourceKind".to_string(),
                        update_source_kind.to_string(),
                    ),
                    (
                        "remoteManifestAvailable".to_string(),
                        output.update_policy.remote_manifest.is_some().to_string(),
                    ),
                    (
                        "fallbackApplied".to_string(),
                        output.update_policy.fallback_applied.to_string(),
                    ),
                    (
                        "installedCount".to_string(),
                        output.installed.len().to_string(),
                    ),
                    (
                        "mcsInstalled".to_string(),
                        output
                            .installed
                            .iter()
                            .any(|entry| entry.name == "codemod-cli")
                            .to_string(),
                    ),
                    (
                        "mcpInstalled".to_string(),
                        output
                            .installed
                            .iter()
                            .any(|entry| entry.name == "codemod-mcp")
                            .to_string(),
                    ),
                    (
                        "autoSafeAttempted".to_string(),
                        auto_safe
                            .map(|result| result.attempted.to_string())
                            .unwrap_or_else(|| "0".to_string()),
                    ),
                    (
                        "autoSafeApplied".to_string(),
                        auto_safe
                            .map(|result| result.applied.to_string())
                            .unwrap_or_else(|| "0".to_string()),
                    ),
                    (
                        "autoSafeFailed".to_string(),
                        auto_safe
                            .map(|result| result.failed.to_string())
                            .unwrap_or_else(|| "0".to_string()),
                    ),
                    (
                        "warningsCount".to_string(),
                        output.warnings.len().to_string(),
                    ),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;
}

async fn send_agent_list_event(
    telemetry: &TelemetrySenderMutex,
    requested_harness: Harness,
    resolved_harness: Harness,
    format: OutputFormat,
    listed_count: usize,
    warnings_count: usize,
) {
    telemetry
        .send_event(
            BaseEvent {
                kind: "agentSkillsListed".to_string(),
                properties: HashMap::from([
                    ("commandName".to_string(), "codemod.agent.list".to_string()),
                    (
                        "requestedHarness".to_string(),
                        requested_harness.as_str().to_string(),
                    ),
                    (
                        "resolvedHarness".to_string(),
                        resolved_harness.as_str().to_string(),
                    ),
                    ("format".to_string(), format.as_str().to_string()),
                    ("listedCount".to_string(), listed_count.to_string()),
                    ("warningsCount".to_string(), warnings_count.to_string()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;
}

fn managed_components_from_install(
    installed: &[InstalledSkill],
    discovery_paths: &[PathBuf],
    periodic_trigger_paths: &[PathBuf],
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

    for trigger_path in periodic_trigger_paths {
        let component_id = trigger_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| format!("periodic-trigger:{name}"))
            .unwrap_or_else(|| format!("periodic-trigger:{}", trigger_path.to_string_lossy()));

        components.push(ManagedComponentSnapshot {
            id: component_id,
            kind: ManagedComponentKind::DiscoveryGuide,
            path: trigger_path.clone(),
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

#[derive(Clone)]
struct InstallInputs {
    harness: Harness,
    scope: InstallScope,
    force: bool,
    periodic_policy: PeriodicUpdatePolicy,
    update_policy: UpdatePolicyMode,
    update_source: String,
    require_signed_manifest: Option<bool>,
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
            periodic_policy: command.periodic_policy,
            update_policy: command.update_policy,
            update_source: command.update_source.clone(),
            require_signed_manifest: resolve_signed_manifest_override(
                command.require_signed_manifest,
                command.allow_unsigned_manifest,
            ),
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
        periodic_policy: command.periodic_policy,
        update_policy: command.update_policy,
        update_source: command.update_source.clone(),
        require_signed_manifest: resolve_signed_manifest_override(
            command.require_signed_manifest,
            command.allow_unsigned_manifest,
        ),
    })
}

fn resolve_signed_manifest_override(require_signed: bool, allow_unsigned: bool) -> Option<bool> {
    if require_signed {
        Some(true)
    } else if allow_unsigned {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
