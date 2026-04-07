use crate::commands::harness_adapter::{
    core_skill_path_for_harness, install_restart_hint, mcp_config_path_for_harness,
    mcs_install_requires_force, persist_managed_install_state, read_codex_mcp_server_from_path,
    read_managed_install_state, resolve_adapter, resolve_install_scope, runtime_paths_for_execution,
    runtime_working_directory,
    skill_discovery_guide_paths,
    upsert_mcs_command_entrypoints_with_runtime, upsert_periodic_update_trigger,
    upsert_skill_discovery_guides_with_command_status, Harness, HarnessAdapterError,
    InstallRequest, InstallScope, InstalledSkill, ManagedComponentKind, ManagedComponentSnapshot,
    OutputFormat, PeriodicUpdatePolicy, ResolvedAdapter, VerificationStatus,
};
use crate::commands::output::{
    exit_adapter_error, format_output_path, prompt_for_overwrite_confirmation,
};
use crate::{TelemetrySenderMutex, CLI_VERSION};
use anyhow::Result;
use clap::{Args, Subcommand};
use codemod_telemetry::send_event::BaseEvent;
use inquire::Select;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command as TokioCommand;

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
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
#[command(
    about = "Install Codemod AI integrations, or run update/list subcommands",
    after_help = "Run `codemod ai` without a subcommand to install Codemod integrations."
)]
pub struct Command {
    #[command(subcommand)]
    action: Option<AiAction>,
    #[command(flatten)]
    install: InstallCommand,
}

#[derive(Subcommand, Debug)]
enum AiAction {
    /// Reconcile/apply managed updates; falls back to install when not installed yet
    Update(UpdateCommand),
    /// List installed codemod skills for a harness
    List(ListCommand),
    /// Verify that Codemod AI and MCP are actually visible and usable for the chosen harness/scope
    Doctor(DoctorCommand),
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
    /// Internal mode switch used by `ai update`
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

#[derive(Args, Debug)]
struct DoctorCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Codex)]
    harness: Harness,
    /// Inspect the current repo workspace scope
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Inspect the user-level scope
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// Run an active Codex probe to verify Codemod MCP becomes visible
    #[arg(long)]
    probe: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Logs)]
    format: OutputFormat,
}

#[derive(Serialize)]
struct DoctorOutput {
    ok: bool,
    harness: String,
    scope: String,
    skill_path: String,
    skill_exists: bool,
    mcp_config_path: Option<String>,
    mcp_config_exists: bool,
    mcp_entry_present: bool,
    configured_command: Option<String>,
    configured_args: Vec<String>,
    verification_checks: Vec<DoctorVerificationCheckOutput>,
    probe: Option<DoctorProbeOutput>,
    warnings: Vec<String>,
    notes: Vec<String>,
}

#[derive(Serialize)]
struct DoctorVerificationCheckOutput {
    skill: String,
    scope: Option<String>,
    status: String,
    reason: Option<String>,
}

#[derive(Serialize)]
struct DoctorProbeOutput {
    status: String,
    command: Vec<String>,
    output_tail: String,
}

pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    match &args.action {
        None => {
            let command = &args.install;
            handle_install_like_action(
                command,
                &telemetry,
                "aiMcsInstalled",
                "codemod.ai.install",
                command.harness,
            )
            .await
        }
        Some(AiAction::Update(command)) => {
            let install_like_command = InstallCommand::from(command);
            handle_install_like_action(
                &install_like_command,
                &telemetry,
                "aiMcsUpdated",
                "codemod.ai.update",
                command.harness,
            )
            .await
        }
        Some(AiAction::List(command)) => {
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
            send_ai_list_event(
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
        Some(AiAction::Doctor(command)) => handle_doctor_action(command, &telemetry).await,
    }
}

async fn handle_doctor_action(
    command: &DoctorCommand,
    telemetry: &TelemetrySenderMutex,
) -> Result<()> {
    let scope = resolve_install_scope(command.project, command.user).unwrap_or_else(|error| {
        exit_adapter_error(error, command.format);
    });
    let runtime_paths = runtime_paths_for_execution(None, None).unwrap_or_else(|error| {
        exit_adapter_error(error, command.format);
    });
    let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
        exit_adapter_error(error, command.format);
    });

    let skill_path =
        core_skill_path_for_harness(resolved_adapter.harness, scope, &runtime_paths).unwrap_or_else(
            |error| {
                exit_adapter_error(error, command.format);
            },
        );
    let skill_exists = skill_path.is_file();

    let verification_checks = resolved_adapter
        .adapter
        .verify_skills()
        .unwrap_or_else(|error| {
            exit_adapter_error(error, command.format);
        })
        .into_iter()
        .map(|check| DoctorVerificationCheckOutput {
            skill: check.skill,
            scope: check.scope.map(|scope| scope.as_str().to_string()),
            status: match check.status {
                VerificationStatus::Pass => "pass".to_string(),
                VerificationStatus::Fail => "fail".to_string(),
            },
            reason: check.reason,
        })
        .collect::<Vec<_>>();

    let harness_supports_mcp = !matches!(resolved_adapter.harness, Harness::Antigravity | Harness::Auto);

    let mcp_config_path = if harness_supports_mcp {
        Some(
            mcp_config_path_for_harness(resolved_adapter.harness, scope, &runtime_paths)
                .unwrap_or_else(|error| exit_adapter_error(error, command.format)),
        )
    } else {
        None
    };
    let mcp_config_exists = mcp_config_path.as_ref().is_some_and(|path| path.is_file());
    let (mcp_entry_present, configured_command, configured_args) = if resolved_adapter.harness
        == Harness::Codex
    {
        if let Some(path) = &mcp_config_path {
            match read_codex_mcp_server_from_path(path) {
                Ok(Some((command, args))) => (true, Some(command), args),
                Ok(None) => (false, None, Vec::new()),
                Err(error) => {
                    return Err(anyhow::Error::msg(error.to_string()));
                }
            }
        } else {
            (false, None, Vec::new())
        }
    } else {
        (mcp_config_exists, None, Vec::new())
    };

    let mut notes = Vec::new();
    let mut warnings = resolved_adapter.warnings.clone();

    if !skill_exists {
        warnings.push("Core codemod skill file is missing for the chosen scope.".to_string());
    }
    if harness_supports_mcp && !mcp_config_exists {
        warnings.push("Codemod MCP config file is missing for the chosen scope.".to_string());
    }
    if harness_supports_mcp && mcp_config_exists && !mcp_entry_present {
        warnings.push("Codemod MCP config exists but does not contain a usable `codemod` MCP entry.".to_string());
    }

    let probe = if command.probe && resolved_adapter.harness == Harness::Codex {
        Some(run_codex_probe(runtime_working_directory(&runtime_paths)).await)
    } else if command.probe {
        warnings.push("`--probe` is currently only supported for `--harness codex`.".to_string());
        None
    } else {
        None
    };

    if resolved_adapter.harness == Harness::Codex {
        notes.push(format!(
            "Before authoring codemods, confirm Codemod MCP becomes visible in Codex for `{}` scope.",
            scope.as_str()
        ));
    }

    let ok = skill_exists
        && verification_checks.iter().all(|check| check.status == "pass")
        && (!harness_supports_mcp || (mcp_config_exists && mcp_entry_present))
        && probe
            .as_ref()
            .map(|probe| probe.status == "probe_passed" || probe.status == "codex_unavailable")
            .unwrap_or(true);

    let output = DoctorOutput {
        ok,
        harness: resolved_adapter.harness.as_str().to_string(),
        scope: scope.as_str().to_string(),
        skill_path: format_output_path(&skill_path),
        skill_exists,
        mcp_config_path: mcp_config_path.as_ref().map(|path| format_output_path(path)),
        mcp_config_exists,
        mcp_entry_present,
        configured_command,
        configured_args,
        verification_checks,
        probe,
        warnings,
        notes,
    };

    match command.format {
        OutputFormat::Logs | OutputFormat::Table => {
            println!("Harness: {}", output.harness);
            println!("Scope: {}", output.scope);
            println!("Skill path: {}", output.skill_path);
            println!("Skill present: {}", output.skill_exists);
            if let Some(path) = &output.mcp_config_path {
                println!("MCP config: {}", path);
                println!("MCP config present: {}", output.mcp_config_exists);
                println!("Codemod MCP entry present: {}", output.mcp_entry_present);
            }
            if let Some(command) = &output.configured_command {
                let mut parts = vec![command.clone()];
                parts.extend(output.configured_args.iter().cloned());
                println!("Configured MCP command: {}", parts.join(" "));
            }
            if !output.verification_checks.is_empty() {
                println!("Verification checks:");
                for check in &output.verification_checks {
                    let scope = check.scope.as_deref().unwrap_or("unknown");
                    println!("  - {} [{}]: {}", check.skill, scope, check.status);
                    if let Some(reason) = &check.reason {
                        println!("    {}", reason);
                    }
                }
            }
            if let Some(probe) = &output.probe {
                println!("Probe status: {}", probe.status);
                println!("Probe command: {}", probe.command.join(" "));
                if !probe.output_tail.is_empty() {
                    println!("Probe output tail:\n{}", probe.output_tail);
                }
            }
            if !output.notes.is_empty() {
                println!("Notes:");
                for note in &output.notes {
                    println!("  - {note}");
                }
            }
            if !output.warnings.is_empty() {
                println!("Warnings:");
                for warning in &output.warnings {
                    println!("  - {warning}");
                }
            }
        }
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&output)?),
        OutputFormat::Yaml => println!("{}", serde_yaml::to_string(&output)?),
    }

    telemetry
        .send_event(
            BaseEvent {
                kind: "aiDoctorRan".to_string(),
                properties: HashMap::from([
                    ("harness".to_string(), output.harness.clone()),
                    ("scope".to_string(), output.scope.clone()),
                    ("probeRequested".to_string(), command.probe.to_string()),
                    ("ok".to_string(), output.ok.to_string()),
                    ("warningsCount".to_string(), output.warnings.len().to_string()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                ]),
            },
            None,
        )
        .await;

    Ok(())
}

async fn run_codex_probe(probe_dir: &std::path::Path) -> DoctorProbeOutput {
    if std::process::Command::new("codex")
        .arg("--version")
        .output()
        .is_err()
    {
        return DoctorProbeOutput {
            status: "codex_unavailable".to_string(),
            command: vec!["codex".to_string()],
            output_tail: String::new(),
        };
    }

    let command = vec![
        "codex".to_string(),
        "exec".to_string(),
        "-C".to_string(),
        probe_dir.display().to_string(),
        "List the available tools by name only.".to_string(),
    ];

    let output = tokio::time::timeout(
        Duration::from_secs(30),
        TokioCommand::new("codex")
            .arg("exec")
            .arg("-C")
            .arg(probe_dir)
            .arg("List the available tools by name only.")
            .output(),
    )
    .await;

    match output {
        Ok(Ok(output)) => {
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let ready = combined.contains("mcp: codemod ready")
                || combined.contains("mcp startup: ready: codemod");
            DoctorProbeOutput {
                status: if ready {
                    "probe_passed".to_string()
                } else {
                    "probe_failed".to_string()
                },
                command,
                output_tail: tail_text(&combined, 60),
            }
        }
        Ok(Err(error)) => DoctorProbeOutput {
            status: "probe_failed".to_string(),
            command,
            output_tail: error.to_string(),
        },
        Err(_) => DoctorProbeOutput {
            status: "probe_failed".to_string(),
            command,
            output_tail: "Timed out after 30s".to_string(),
        },
    }
}

fn tail_text(value: &str, max_lines: usize) -> String {
    let mut lines = value.lines().collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    lines.join("\n")
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
    if let Some(warning) =
        goose_project_scope_command_warning(resolved_adapter.harness, install_inputs.scope)
    {
        warnings.push(warning);
    }
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
    if resolved_adapter.harness == Harness::Codex {
        let scope_flag = match install_inputs.scope {
            InstallScope::Project => "--project",
            InstallScope::User => "--user",
        };
        messages.push(format!(
            "Before authoring codemods, run `codemod ai doctor --harness codex {scope_flag} --probe` and confirm Codemod MCP is visible."
        ));
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
    send_ai_install_event(
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

async fn send_ai_install_event(
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

async fn send_ai_list_event(
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
                kind: "aiSkillsListed".to_string(),
                properties: HashMap::from([
                    ("commandName".to_string(), "codemod.ai.list".to_string()),
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
    command_paths: &[PathBuf],
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

    for command_path in command_paths {
        let component_id = command_path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(|name| format!("command:{name}"))
            .unwrap_or_else(|| format!("command:{}", command_path.to_string_lossy()));

        components.push(ManagedComponentSnapshot {
            id: component_id,
            kind: ManagedComponentKind::Command,
            path: command_path.clone(),
            version: Some(CLI_VERSION.to_string()),
        });
    }

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

    let command_paths = match runtime_paths_for_execution(None, None) {
        Ok(runtime_paths) => match upsert_mcs_command_entrypoints_with_runtime(
            resolved_adapter.harness,
            install_inputs.scope,
            install_inputs.force,
            &runtime_paths,
        ) {
            Ok(paths) => {
                if !paths.is_empty() {
                    messages.push(format!(
                        "Installed codemod creation command in: {}",
                        paths
                            .iter()
                            .map(|path| format_output_path(path))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                paths
            }
            Err(error) => {
                warnings.push(format!(
                    "Installed skills, but failed to install codemod creation command: {error}"
                ));
                Vec::new()
            }
        },
        Err(error) => {
            warnings.push(format!(
                "Installed skills, but failed to resolve runtime paths for command entrypoints: {error}"
            ));
            Vec::new()
        }
    };

    match upsert_skill_discovery_guides_with_command_status(
        resolved_adapter.harness,
        install_inputs.scope,
        !command_paths.is_empty(),
    ) {
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
    let managed_components = managed_components_from_install(
        &installed,
        &command_paths,
        &discovery_paths,
        &periodic_trigger_paths,
    );
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
        let (options, starting_cursor) = scope_prompt_options(harness);

        Select::new("Choose install scope:", options)
            .with_starting_cursor(starting_cursor)
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
    match label_harness {
        Harness::Goose => "user (goose: ~/.goose/skills + ~/.config/goose/config.yaml)".to_string(),
        _ => format!(
            "user ({}: {})",
            label_harness.as_str(),
            user_skills_root_hint_for_harness(label_harness)
        ),
    }
}

fn scope_prompt_options(harness: Harness) -> (Vec<ScopePromptOption>, usize) {
    if scope_label_harness(harness) == Harness::Goose {
        return (
            vec![
                ScopePromptOption {
                    scope: InstallScope::User,
                    label:
                        "user (recommended; enables Goose /codemod via ~/.config/goose/config.yaml)"
                            .to_string(),
                },
                ScopePromptOption {
                    scope: InstallScope::Project,
                    label: "project (current workspace; skills only, /codemod stays unavailable)"
                        .to_string(),
                },
            ],
            0,
        );
    }

    (
        vec![
            ScopePromptOption {
                scope: InstallScope::Project,
                label: "project (current workspace)".to_string(),
            },
            ScopePromptOption {
                scope: InstallScope::User,
                label: interactive_user_scope_label(harness),
            },
        ],
        0,
    )
}

fn goose_project_scope_command_warning(harness: Harness, scope: InstallScope) -> Option<String> {
    if harness == Harness::Goose && scope == InstallScope::Project {
        return Some(
            "Goose /codemod is only documented via user config (`~/.config/goose/config.yaml`); project scope installed skills only. Re-run with `--user` to enable the slash command.".to_string(),
        );
    }

    None
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
