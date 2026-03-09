use crate::commands::harness_adapter::{
    install_restart_hint, mcs_install_requires_force, persist_managed_install_state,
    read_managed_install_state, resolve_adapter, resolve_install_scope,
    skill_discovery_guide_paths, upsert_periodic_update_trigger, upsert_skill_discovery_guides,
    Harness, HarnessAdapterError, InstallRequest, InstallScope, InstalledSkill,
    ManagedComponentKind, ManagedComponentSnapshot, OutputFormat, PeriodicUpdatePolicy,
    ResolvedAdapter, VerificationStatus,
};
use crate::commands::output::{
    exit_adapter_error, format_output_path, prompt_for_overwrite_confirmation,
};
use crate::{TelemetrySenderMutex, CLI_VERSION};
use anyhow::Result;
use clap::{Args, Subcommand};
use codemod_telemetry::send_event::BaseEvent;
use inquire::Select;
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
    /// Reconcile/apply managed updates; falls back to install when not installed yet
    Update(UpdateCommand),
    /// List installed codemod skills for a harness
    List(ListCommand),
}

#[derive(Args, Debug)]
struct InstallCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Disable interactive install wizard prompts
    #[arg(long)]
    no_interactive: bool,
    /// Install into current repo workspace
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Install into user-level skills path
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// Overwrite existing skill files
    #[arg(long)]
    force: bool,
    /// Internal mode switch used by `agent update`
    #[arg(skip)]
    update: bool,
    /// Managed update policy for this install and periodic harness checks
    #[arg(long, value_enum, default_value_t = UpdatePolicyMode::AutoSafe)]
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
    #[arg(long, value_enum, default_value_t = OutputFormat::Logs)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct UpdateCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Disable interactive install wizard prompts
    #[arg(long)]
    no_interactive: bool,
    /// Update within current repo workspace scope
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Update within user-level scope
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// If fallback install is needed, overwrite existing skill files
    #[arg(long)]
    force: bool,
    /// Managed update policy for this update execution
    #[arg(long, value_enum, default_value_t = UpdatePolicyMode::AutoSafe)]
    update_policy: UpdatePolicyMode,
    /// Remote source for managed update metadata: local, registry, or absolute URL
    #[arg(long, default_value = DEFAULT_UPDATE_SOURCE)]
    update_source: String,
    /// Require signed remote manifests for this update execution
    #[arg(long, conflicts_with = "allow_unsigned_manifest")]
    require_signed_manifest: bool,
    /// Allow unsigned remote manifests for this update execution
    #[arg(long, conflicts_with = "require_signed_manifest")]
    allow_unsigned_manifest: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Logs)]
    format: OutputFormat,
}

impl From<&UpdateCommand> for InstallCommand {
    fn from(value: &UpdateCommand) -> Self {
        Self {
            harness: value.harness,
            no_interactive: value.no_interactive,
            project: value.project,
            user: value.user,
            force: value.force,
            update: true,
            update_policy: value.update_policy,
            update_source: value.update_source.clone(),
            require_signed_manifest: value.require_signed_manifest,
            allow_unsigned_manifest: value.allow_unsigned_manifest,
            format: value.format,
        }
    }
}

#[derive(Args, Debug)]
struct ListCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Logs)]
    format: OutputFormat,
}

pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    match &args.action {
        AgentAction::Install(command) => {
            handle_install_like_action(
                command,
                &telemetry,
                "agentMcsInstalled",
                "codemod.agent.install",
                command.harness,
            )
            .await
        }
        AgentAction::Update(command) => {
            let install_like_command = InstallCommand::from(command);
            handle_install_like_action(
                &install_like_command,
                &telemetry,
                "agentMcsUpdated",
                "codemod.agent.update",
                command.harness,
            )
            .await
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

async fn handle_install_like_action(
    command: &InstallCommand,
    telemetry: &TelemetrySenderMutex,
    event_kind: &'static str,
    command_name: &'static str,
    requested_harness: Harness,
) -> Result<()> {
    let mut install_inputs = resolve_install_inputs(command).unwrap_or_else(|error| {
        exit_adapter_error(error, command.format);
    });
    let resolved_adapter = resolve_adapter(install_inputs.harness).unwrap_or_else(|error| {
        exit_adapter_error(error, command.format);
    });
    if install_inputs.interactive && !install_inputs.force && !command.update {
        let overwrite_required =
            mcs_install_requires_force(resolved_adapter.harness, install_inputs.scope)
                .unwrap_or_else(|error| exit_adapter_error(error, command.format));
        if overwrite_required {
            install_inputs.force = prompt_for_overwrite_confirmation()
                .unwrap_or_else(|error| exit_adapter_error(error, command.format));
        }
    }
    let update_policy = resolve_update_policy_context(&UpdatePolicyResolveOptions {
        mode: install_inputs.update_policy,
        remote_source: install_inputs.update_source.clone(),
        require_signed_manifest: install_inputs.require_signed_manifest,
    })
    .await
    .unwrap_or_else(|error| {
        exit_adapter_error(
            HarnessAdapterError::InstallFailed(format!("failed to resolve update policy: {error}")),
            command.format,
        )
    });
    let mut warnings = resolved_adapter.warnings.clone();
    let mut messages = Vec::new();
    warnings.extend(update_policy.warnings.iter().cloned());
    let (installed, managed_components) = if command.update {
        let mut managed_state =
            read_managed_install_state(resolved_adapter.harness, install_inputs.scope)
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
        if managed_state.is_none() && !install_inputs.scope_explicit {
            let alternate = alternate_scope(install_inputs.scope);
            let alternate_state = read_managed_install_state(resolved_adapter.harness, alternate)
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            if let Some(found_state) = alternate_state {
                messages.push(format!(
                    "No managed state found for `{}` scope; using existing `{}` scope state.",
                    install_inputs.scope.as_str(),
                    alternate.as_str()
                ));
                install_inputs.scope = alternate;
                managed_state = Some(found_state);
            }
        }
        if let Some(managed_state) = managed_state {
            messages.push(format!(
                "Loaded managed state from: {}",
                format_output_path(&managed_state.path)
            ));
            (Vec::new(), managed_state.components)
        } else {
            messages.push(
                "No managed install state found; running install fallback before update."
                    .to_string(),
            );
            run_install_flow(
                &resolved_adapter,
                &install_inputs,
                command.format,
                &mut messages,
                &mut warnings,
            )
        }
    } else {
        run_install_flow(
            &resolved_adapter,
            &install_inputs,
            command.format,
            &mut messages,
            &mut warnings,
        )
    };
    let component_decisions = build_component_reconcile_decisions(
        &update_policy,
        resolved_adapter.harness,
        &managed_components,
    );
    let auto_safe_apply =
        maybe_apply_auto_safe_updates(&update_policy, &component_decisions, &managed_components)
            .await;

    warnings.extend(auto_safe_apply.warnings.iter().cloned());
    if command.update {
        messages
            .push("Executed managed update reconciliation for existing components.".to_string());
    }
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
        messages.push(policy_runtime_message);
    }

    let restart_hint = if command.update {
        auto_safe_apply.result.as_ref().and_then(|result| {
            (result.applied > 0).then(|| install_restart_hint(resolved_adapter.harness))
        })
    } else {
        Some(install_restart_hint(resolved_adapter.harness))
    };
    let output = build_install_output(BuildInstallOutputInput {
        harness: resolved_adapter.harness,
        scope: install_inputs.scope,
        installed,
        managed_state,
        update_policy: &update_policy,
        component_decisions,
        auto_safe_apply: auto_safe_apply.result,
        notes: messages,
        warnings,
        restart_hint,
    });
    print_install_output(&output, command.format)?;
    send_agent_install_event(
        telemetry,
        event_kind,
        command_name,
        requested_harness,
        resolved_adapter.harness,
        &install_inputs,
        &output,
    )
    .await;
    Ok(())
}

async fn send_agent_install_event(
    telemetry: &TelemetrySenderMutex,
    event_kind: &'static str,
    command_name: &'static str,
    requested_harness: Harness,
    resolved_harness: Harness,
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
                kind: event_kind.to_string(),
                properties: HashMap::from([
                    ("commandName".to_string(), command_name.to_string()),
                    (
                        "requestedHarness".to_string(),
                        requested_harness.as_str().to_string(),
                    ),
                    (
                        "resolvedHarness".to_string(),
                        resolved_harness.as_str().to_string(),
                    ),
                    ("scope".to_string(), inputs.scope.as_str().to_string()),
                    ("interactive".to_string(), inputs.interactive.to_string()),
                    ("force".to_string(), inputs.force.to_string()),
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
                            .any(|entry| entry.name == "codemod")
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

fn run_install_flow(
    resolved_adapter: &ResolvedAdapter,
    install_inputs: &InstallInputs,
    format: OutputFormat,
    messages: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> (Vec<InstalledSkill>, Vec<ManagedComponentSnapshot>) {
    let request = InstallRequest {
        scope: install_inputs.scope,
        force: install_inputs.force,
    };
    let installed = resolved_adapter
        .adapter
        .install_skills(&request)
        .unwrap_or_else(|error| {
            exit_adapter_error(error, format);
        });
    let verification_checks = resolved_adapter
        .adapter
        .verify_skills()
        .unwrap_or_else(|error| {
            exit_adapter_error(error, format);
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
            format,
        );
    }

    match upsert_skill_discovery_guides(resolved_adapter.harness, install_inputs.scope) {
        Ok(updated_files) if !updated_files.is_empty() => messages.push(format!(
            "Updated discovery hints in: {}",
            updated_files
                .iter()
                .map(|path| format_output_path(path))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Ok(_) => {}
        Err(error) => warnings.push(format!(
            "Installed skills, but failed to update harness discovery hints: {error}"
        )),
    }

    let discovery_paths = match skill_discovery_guide_paths(
        resolved_adapter.harness,
        install_inputs.scope,
    ) {
        Ok(paths) => paths,
        Err(error) => {
            warnings.push(format!(
                "Installed skills, but failed to resolve harness discovery hint paths for managed-state tracking: {error}"
            ));
            Vec::new()
        }
    };

    let periodic_trigger = match upsert_periodic_update_trigger(
        resolved_adapter.harness,
        install_inputs.scope,
        periodic_policy_from_update_mode(install_inputs.update_policy),
    ) {
        Ok(result) => Some(result),
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
    let managed_components =
        managed_components_from_install(&installed, &discovery_paths, &periodic_trigger_paths);
    (installed, managed_components)
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
    scope_explicit: bool,
    force: bool,
    interactive: bool,
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

#[derive(Clone)]
struct ScopePromptOption {
    scope: InstallScope,
    label: String,
}

impl fmt::Display for ScopePromptOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label)
    }
}

fn resolve_install_inputs(
    command: &InstallCommand,
) -> std::result::Result<InstallInputs, HarnessAdapterError> {
    let interactive = !command.no_interactive
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    let scope_explicit = command.project || command.user;

    if !interactive {
        let scope = resolve_install_scope(command.project, command.user)?;
        return Ok(InstallInputs {
            harness: command.harness,
            scope,
            scope_explicit,
            force: command.force,
            interactive,
            update_policy: command.update_policy,
            update_source: command.update_source.clone(),
            require_signed_manifest: resolve_signed_manifest_override(
                command.require_signed_manifest,
                command.allow_unsigned_manifest,
            ),
        });
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
            HarnessPromptOption {
                harness: Harness::Codex,
                label: "codex",
            },
            HarnessPromptOption {
                harness: Harness::Antigravity,
                label: "antigravity",
            },
        ];
        let starting_cursor = detected_harness_for_interactive_prompt()
            .and_then(|detected| options.iter().position(|option| option.harness == detected))
            .unwrap_or(0);

        Select::new("Choose harness adapter:", options)
            .with_starting_cursor(starting_cursor)
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
        let user_scope_label = interactive_user_scope_label(harness);
        let options = vec![
            ScopePromptOption {
                scope: InstallScope::Project,
                label: "project (current workspace)".to_string(),
            },
            ScopePromptOption {
                scope: InstallScope::User,
                label: user_scope_label,
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

    Ok(InstallInputs {
        harness,
        scope,
        scope_explicit,
        force: command.force,
        interactive,
        update_policy: command.update_policy,
        update_source: command.update_source.clone(),
        require_signed_manifest: resolve_signed_manifest_override(
            command.require_signed_manifest,
            command.allow_unsigned_manifest,
        ),
    })
}

fn alternate_scope(scope: InstallScope) -> InstallScope {
    match scope {
        InstallScope::Project => InstallScope::User,
        InstallScope::User => InstallScope::Project,
    }
}

fn detected_harness_for_interactive_prompt() -> Option<Harness> {
    let resolved = resolve_adapter(Harness::Auto).ok()?;
    if resolved.warnings.is_empty() {
        Some(resolved.harness)
    } else {
        None
    }
}

fn scope_label_harness(harness: Harness) -> Harness {
    match harness {
        Harness::Auto => Harness::Claude,
        resolved => resolved,
    }
}

fn user_skills_root_hint_for_harness(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude | Harness::Auto => "~/.claude/skills",
        Harness::Goose => "~/.goose/skills",
        Harness::Opencode => "~/.opencode/skills",
        Harness::Cursor => "~/.cursor/skills",
        Harness::Codex => "~/.agents/skills",
        Harness::Antigravity => "~/.gemini/antigravity/skills",
    }
}

fn interactive_user_scope_label(harness: Harness) -> String {
    let label_harness = scope_label_harness(harness);
    format!(
        "user ({}: {})",
        label_harness.as_str(),
        user_skills_root_hint_for_harness(label_harness)
    )
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

fn periodic_policy_from_update_mode(mode: UpdatePolicyMode) -> PeriodicUpdatePolicy {
    match mode {
        UpdatePolicyMode::Manual => PeriodicUpdatePolicy::Manual,
        UpdatePolicyMode::Notify => PeriodicUpdatePolicy::Notify,
        UpdatePolicyMode::AutoSafe => PeriodicUpdatePolicy::AutoSafe,
    }
}

#[cfg(test)]
mod tests;
