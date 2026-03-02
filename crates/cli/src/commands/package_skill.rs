use crate::commands::harness_adapter::{
    install_restart_hint, resolve_adapter, resolve_install_scope, upsert_skill_discovery_guides,
    Harness, HarnessAdapterError, InstallRequest, InstallScope, InstalledSkill, OutputFormat,
    SkillPackageInstallSpec,
};
use crate::commands::output::{exit_adapter_error, format_output_path};
use crate::engine::create_registry_client;
use crate::utils::manifest::CodemodManifest;
use crate::utils::package_validation::{
    authored_skill_file_candidate, detect_package_behavior_shape_with_manifest_hint,
    PackageBehaviorShape,
};
use crate::utils::skill_layout::{expected_authored_skill_file, find_authored_skill_dir};
use crate::{TelemetrySenderMutex, CLI_VERSION};
use anyhow::Result;
use butterflow_core::registry::RegistryError;
use clap::Args;
use codemod_telemetry::send_event::BaseEvent;
use inquire::{Confirm, Select};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[cfg(test)]
use crate::utils::skill_layout::SKILL_FILE_NAME;

#[derive(Args, Debug)]
pub struct InternalInstallSkillStepCommand {
    /// Package identifier
    #[arg(value_name = "PACKAGE")]
    pub package_id: String,
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    pub harness: Harness,
    /// Disable interactive install wizard prompts
    #[arg(long)]
    pub no_interactive: bool,
    /// Install into current repo workspace
    #[arg(long, conflicts_with = "user")]
    pub project: bool,
    /// Install into user-level skills path
    #[arg(long, conflicts_with = "project")]
    pub user: bool,
    /// Overwrite existing skill files
    #[arg(long)]
    pub force: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Logs)]
    pub format: OutputFormat,
}

#[derive(Serialize)]
struct PackageSkillInstallOutput {
    ok: bool,
    package_id: String,
    harness: String,
    scope: String,
    installed: Vec<InstalledSkillOutput>,
    notes: Vec<String>,
    warnings: Vec<String>,
    restart_hint: Option<String>,
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

#[derive(Clone, Copy)]
struct SkillInstallInputs {
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

pub async fn handle_internal_install_step(
    command: &InternalInstallSkillStepCommand,
    telemetry: &TelemetrySenderMutex,
) -> Result<()> {
    install_skill_command(command, telemetry).await?;
    Ok(())
}

pub async fn install_from_run_prompt(
    package_id: &str,
    telemetry: &TelemetrySenderMutex,
) -> Result<()> {
    install_from_run_request(package_id, false, telemetry).await
}

pub async fn install_from_run_request(
    package_id: &str,
    no_interactive: bool,
    telemetry: &TelemetrySenderMutex,
) -> Result<()> {
    let command = InternalInstallSkillStepCommand {
        package_id: package_id.to_string(),
        harness: Harness::Auto,
        no_interactive,
        project: true,
        user: false,
        force: false,
        format: OutputFormat::Logs,
    };
    install_skill_command(&command, telemetry).await
}

async fn install_skill_command(
    command: &InternalInstallSkillStepCommand,
    telemetry: &TelemetrySenderMutex,
) -> Result<()> {
    let command_format = command.format;

    let install_inputs = resolve_install_inputs(command).unwrap_or_else(|error| {
        exit_adapter_error(error, command_format);
    });

    let resolved_adapter = resolve_adapter(install_inputs.harness).unwrap_or_else(|error| {
        exit_adapter_error(error, command_format);
    });

    let request = InstallRequest {
        scope: install_inputs.scope,
        force: install_inputs.force,
    };

    let (package, mut package_warnings) = resolve_skill_package_for_install(&command.package_id)
        .await
        .unwrap_or_else(|error| {
            exit_adapter_error(error, command_format);
        });

    let installed = resolved_adapter
        .adapter
        .install_package_skill(&package, &request)
        .unwrap_or_else(|error| {
            exit_adapter_error(error, command_format);
        });

    let mut warnings = resolved_adapter.warnings;
    let mut notes = Vec::new();
    warnings.append(&mut package_warnings);
    match upsert_skill_discovery_guides(resolved_adapter.harness, install_inputs.scope) {
        Ok(updated_files) if !updated_files.is_empty() => notes.push(format!(
            "Updated discovery hints in: {}",
            updated_files
                .iter()
                .map(|path| format_output_path(path))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Ok(_) => {}
        Err(error) => warnings.push(format!(
            "Installed skill, but failed to update harness discovery hints: {error}"
        )),
    }
    let restart_hint = Some(install_restart_hint(resolved_adapter.harness));

    let output = build_install_output(
        &package.id,
        resolved_adapter.harness,
        install_inputs.scope,
        installed,
        notes,
        warnings,
        restart_hint,
    );
    print_install_output(&output, command_format)?;
    send_package_skill_install_event(
        telemetry,
        &PackageSkillInstallTelemetryInput {
            requested_harness: command.harness,
            resolved_harness: resolved_adapter.harness,
            scope: install_inputs.scope,
            force: install_inputs.force,
            format: command_format,
            package: &package,
            output: &output,
        },
    )
    .await;

    Ok(())
}

fn resolve_install_inputs(
    command: &InternalInstallSkillStepCommand,
) -> std::result::Result<SkillInstallInputs, HarnessAdapterError> {
    let interactive = !command.no_interactive
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();

    if !interactive {
        let scope = resolve_install_scope(command.project, command.user)?;
        return Ok(SkillInstallInputs {
            harness: command.harness,
            scope,
            force: command.force,
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
        let options = vec![
            ScopePromptOption {
                scope: InstallScope::Project,
                label: "project (current workspace)".to_string(),
            },
            ScopePromptOption {
                scope: InstallScope::User,
                label: interactive_user_scope_label(harness),
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

    Ok(SkillInstallInputs {
        harness,
        scope,
        force,
    })
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

struct PackageSkillInstallTelemetryInput<'a> {
    requested_harness: Harness,
    resolved_harness: Harness,
    scope: InstallScope,
    force: bool,
    format: OutputFormat,
    package: &'a SkillPackageInstallSpec,
    output: &'a PackageSkillInstallOutput,
}

async fn send_package_skill_install_event(
    telemetry: &TelemetrySenderMutex,
    input: &PackageSkillInstallTelemetryInput<'_>,
) {
    let PackageSkillInstallTelemetryInput {
        requested_harness,
        resolved_harness,
        scope,
        force,
        format,
        package,
        output,
    } = input;

    telemetry
        .send_event(
            BaseEvent {
                kind: "packageSkillInstalled".to_string(),
                properties: HashMap::from([
                    (
                        "commandName".to_string(),
                        "codemod.packageSkill.install".to_string(),
                    ),
                    ("packageId".to_string(), package.id.clone()),
                    ("packageVersion".to_string(), package.version.clone()),
                    (
                        "requestedHarness".to_string(),
                        requested_harness.as_str().to_string(),
                    ),
                    (
                        "resolvedHarness".to_string(),
                        resolved_harness.as_str().to_string(),
                    ),
                    ("scope".to_string(), scope.as_str().to_string()),
                    ("force".to_string(), force.to_string()),
                    ("format".to_string(), format.as_str().to_string()),
                    (
                        "installedCount".to_string(),
                        output.installed.len().to_string(),
                    ),
                    (
                        "installedNames".to_string(),
                        output
                            .installed
                            .iter()
                            .map(|entry| entry.name.clone())
                            .collect::<Vec<_>>()
                            .join(","),
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

fn build_install_output(
    package_id: &str,
    harness: Harness,
    scope: InstallScope,
    installed: Vec<InstalledSkill>,
    notes: Vec<String>,
    warnings: Vec<String>,
    restart_hint: Option<String>,
) -> PackageSkillInstallOutput {
    PackageSkillInstallOutput {
        ok: true,
        package_id: package_id.to_string(),
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
        notes,
        warnings,
        restart_hint,
    }
}

fn print_install_output(output: &PackageSkillInstallOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Logs => print_install_output_logs(output),
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(output)?),
        OutputFormat::Yaml => println!("{}", serde_yaml::to_string(output)?),
        OutputFormat::Table => print_install_output_table(output),
    }

    Ok(())
}

fn print_install_output_logs(output: &PackageSkillInstallOutput) {
    println!(
        "Installed package skill `{}` for `{}` ({})",
        output.package_id, output.harness, output.scope
    );

    if output.installed.is_empty() {
        println!("No skills were installed.");
    } else {
        println!("Installed components:");
        for installed_skill in &output.installed {
            let version = installed_skill.version.as_deref().unwrap_or("n/a");
            println!(
                "  - {}@{} -> {}",
                installed_skill.name, version, installed_skill.path
            );
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

    if let Some(restart_hint) = &output.restart_hint {
        println!("🎉 {restart_hint}");
    }
}

fn print_install_output_table(output: &PackageSkillInstallOutput) {
    println!("Package: {}", output.package_id);
    println!("Harness: {}", output.harness);
    println!("Scope: {}", output.scope);
    if output.installed.is_empty() {
        println!("No skills were installed.");
        if let Some(restart_hint) = &output.restart_hint {
            println!("🎉 {restart_hint}");
        }
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
                .unwrap_or_else(|| "n/a".to_string()),
            path: installed_skill.path.clone(),
        })
        .collect::<Vec<_>>();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");

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
    if let Some(restart_hint) = &output.restart_hint {
        println!("🎉 {restart_hint}");
    }
}

async fn resolve_skill_package_for_install(
    package_id: &str,
) -> std::result::Result<(SkillPackageInstallSpec, Vec<String>), HarnessAdapterError> {
    let resolved_package = resolve_skill_package(package_id).await?;
    if !resolved_package.behavior_shape.includes_skill() {
        return Err(HarnessAdapterError::SkillPackageInstallFailed(
            unsupported_skill_install_error(
                &resolved_package.id,
                &resolved_package.package_dir,
                resolved_package.behavior_shape,
            ),
        ));
    }

    let skill_source_dir = resolved_package.skill_source_dir.ok_or_else(|| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "Package `{}` declares skill behavior but authored skill files were not found under `{}`.",
            resolved_package.id,
            resolved_package.expected_skill_file.display()
        ))
    })?;

    let mut warnings = Vec::new();
    if let Some(warning) =
        install_warning_for_shape(resolved_package.behavior_shape, &resolved_package.id)
    {
        warnings.push(warning);
    }

    Ok((
        SkillPackageInstallSpec {
            id: resolved_package.id,
            version: resolved_package.version,
            description: resolved_package.description,
            source_dir: skill_source_dir,
        },
        warnings,
    ))
}

#[derive(Debug)]
struct ResolvedSkillPackage {
    id: String,
    version: String,
    description: String,
    package_dir: std::path::PathBuf,
    expected_skill_file: PathBuf,
    skill_source_dir: Option<PathBuf>,
    behavior_shape: PackageBehaviorShape,
}

async fn resolve_skill_package(
    package_id: &str,
) -> std::result::Result<ResolvedSkillPackage, HarnessAdapterError> {
    let registry_client = create_registry_client(None).map_err(|error| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "failed to initialize registry client: {error}"
        ))
    })?;
    let registry_url = registry_client.config.default_registry.clone();
    let resolved_package = registry_client
        .resolve_package(package_id, Some(&registry_url), false, None)
        .await
        .map_err(|error| map_registry_error_to_install_error(package_id, error))?;

    let package_id = format_registry_id(&resolved_package.spec.scope, &resolved_package.spec.name);
    let manifest_path = resolved_package.package_dir.join("codemod.yaml");
    let manifest = read_package_manifest(&manifest_path);
    let description = manifest
        .as_ref()
        .map(|manifest| manifest.description.clone())
        .unwrap_or_else(|| {
            format!(
                "Install package skill for `{}`.",
                resolved_package.spec.name
            )
        });
    let manifest_name = manifest
        .as_ref()
        .map(|manifest| manifest.name.as_str())
        .unwrap_or(resolved_package.spec.name.as_str());
    let candidate = authored_skill_file_candidate(
        &resolved_package.package_dir,
        manifest.as_ref(),
        manifest_name,
    )
    .map_err(|error| HarnessAdapterError::SkillPackageInstallFailed(error.to_string()))?;
    let expected_skill_file = candidate.path;
    let has_explicit_skill_path = candidate.explicit;
    let skill_source_dir = if expected_skill_file.is_file() {
        expected_skill_file.parent().map(Path::to_path_buf)
    } else if !has_explicit_skill_path {
        find_authored_skill_dir(&resolved_package.package_dir, Some(manifest_name))
    } else {
        None
    };
    let behavior_shape = detect_package_behavior_shape_with_manifest_hint(
        &resolved_package.package_dir,
        manifest.as_ref(),
    );

    Ok(ResolvedSkillPackage {
        id: package_id,
        version: resolved_package.version,
        description,
        package_dir: resolved_package.package_dir,
        expected_skill_file,
        skill_source_dir,
        behavior_shape,
    })
}

fn map_registry_error_to_install_error(
    package_id: &str,
    error: RegistryError,
) -> HarnessAdapterError {
    match error {
        RegistryError::PackageNotFound { .. }
        | RegistryError::VersionNotFound { .. }
        | RegistryError::NoVersionAvailable { .. }
        | RegistryError::InvalidScopedPackageName { .. } => {
            HarnessAdapterError::SkillPackageNotFound(package_id.to_string())
        }
        other_error => HarnessAdapterError::SkillPackageInstallFailed(format!(
            "failed to resolve package `{package_id}`: {other_error}"
        )),
    }
}

fn read_package_manifest(manifest_path: &std::path::Path) -> Option<CodemodManifest> {
    if !manifest_path.exists() {
        return None;
    }

    let manifest_content = fs::read_to_string(manifest_path).ok()?;
    serde_yaml::from_str(&manifest_content).ok()
}

fn format_registry_id(scope: &Option<String>, name: &str) -> String {
    match scope {
        Some(scope_name) => {
            if scope_name.starts_with('@') {
                format!("{scope_name}/{name}")
            } else {
                format!("@{scope_name}/{name}")
            }
        }
        None => name.to_string(),
    }
}

fn install_warning_for_shape(
    behavior_shape: PackageBehaviorShape,
    package_id: &str,
) -> Option<String> {
    match behavior_shape {
        PackageBehaviorShape::SkillOnly => Some(format!(
            "Detected skill-only package behavior for `{package_id}`. Installing this package as a harness skill."
        )),
        _ => None,
    }
}

fn unsupported_skill_install_error(
    package_id: &str,
    package_dir: &Path,
    behavior_shape: PackageBehaviorShape,
) -> String {
    let expected_skill_file = expected_authored_skill_file(package_dir, package_id);

    match behavior_shape {
        PackageBehaviorShape::WorkflowOnly => format!(
            "Package `{package_id}` at {} does not provide skill behavior (detected `{}`). Run it with `codemod run {package_id}`.",
            package_dir.display(),
            behavior_shape.as_str(),
        ),
        PackageBehaviorShape::Missing => format!(
            "Package `{package_id}` at {} does not provide workflow or skill behavior (detected `{}`). Add `{}` for skill installs or `workflow.yaml` for executable runs.",
            package_dir.display(),
            expected_skill_file.display(),
            behavior_shape.as_str(),
        ),
        _ => format!(
            "Package `{package_id}` cannot be installed as a skill (detected behavior `{}`).",
            behavior_shape.as_str()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn manifest_with(workflow: &str) -> CodemodManifest {
        CodemodManifest {
            schema_version: "1".to_string(),
            name: "example".to_string(),
            version: "1.0.0".to_string(),
            description: "example".to_string(),
            author: "codemod".to_string(),
            license: None,
            copyright: None,
            repository: None,
            homepage: None,
            bugs: None,
            registry: None,
            workflow: workflow.to_string(),
            targets: None,
            dependencies: None,
            keywords: None,
            category: None,
            readme: None,
            changelog: None,
            documentation: None,
            validation: None,
            capabilities: None,
        }
    }

    #[test]
    fn format_registry_id_supports_scoped_and_unscoped() {
        let scoped_id = format_registry_id(&Some("codemod".to_string()), "jest-to-vitest");
        assert_eq!(scoped_id, "@codemod/jest-to-vitest");

        let scoped_id_with_at = format_registry_id(&Some("@codemod".to_string()), "jest-to-vitest");
        assert_eq!(scoped_id_with_at, "@codemod/jest-to-vitest");

        let unscoped_id = format_registry_id(&None, "jest-to-vitest");
        assert_eq!(unscoped_id, "jest-to-vitest");
    }

    #[test]
    fn map_registry_error_converts_not_found_to_package_not_found() {
        let mapped = map_registry_error_to_install_error(
            "jest-to-vitest",
            RegistryError::PackageNotFound {
                package: "jest-to-vitest".to_string(),
            },
        );
        assert!(matches!(
            mapped,
            HarnessAdapterError::SkillPackageNotFound(id) if id == "jest-to-vitest"
        ));
    }

    #[test]
    fn detect_package_behavior_shape_from_files() {
        let temp_dir = tempdir().unwrap();
        let package_dir = temp_dir.path();

        let authored_skill_dir = package_dir.join("agents/skill").join("example");
        fs::create_dir_all(&authored_skill_dir).unwrap();
        fs::write(authored_skill_dir.join(SKILL_FILE_NAME), "# Skill\n").unwrap();
        fs::write(
            package_dir.join("workflow.yaml"),
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
          package: "@codemod/example"
"#,
        )
        .unwrap();
        assert_eq!(
            detect_package_behavior_shape_with_manifest_hint(package_dir, None),
            PackageBehaviorShape::SkillOnly
        );

        fs::write(
            package_dir.join("workflow.yaml"),
            r#"
version: "1"
nodes:
  - id: run
    name: Run
    type: automatic
    steps:
      - id: run
        name: Run
        run: echo hello
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install skill
        install-skill:
          package: "@codemod/example"
"#,
        )
        .unwrap();
        assert_eq!(
            detect_package_behavior_shape_with_manifest_hint(package_dir, None),
            PackageBehaviorShape::WorkflowAndSkill
        );
    }

    #[test]
    fn detect_package_behavior_shape_workflow_only_when_skill_missing() {
        let temp_dir = tempdir().unwrap();
        let package_dir = temp_dir.path();
        fs::write(
            package_dir.join("workflow.yaml"),
            r#"
version: "1"
nodes:
  - id: run
    name: Run
    type: automatic
    steps:
      - id: run
        name: Run
        run: echo hello
"#,
        )
        .unwrap();

        assert_eq!(
            detect_package_behavior_shape_with_manifest_hint(package_dir, None),
            PackageBehaviorShape::WorkflowOnly
        );
    }

    #[test]
    fn detect_package_behavior_shape_uses_manifest_workflow_and_layout() {
        let temp_dir = tempdir().unwrap();
        let package_dir = temp_dir.path();
        fs::write(
            package_dir.join("custom-workflow.yaml"),
            r#"
version: "1"
nodes:
  - id: run
    name: Run
    type: automatic
    steps:
      - id: run
        name: Run
        run: echo hello
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install skill
        install-skill:
          package: "@codemod/example"
"#,
        )
        .unwrap();
        let authored_skill_dir = package_dir.join("agents/skill").join("example");
        fs::create_dir_all(&authored_skill_dir).unwrap();
        fs::write(authored_skill_dir.join(SKILL_FILE_NAME), "# Skill\n").unwrap();

        let workflow_and_skill_manifest = manifest_with("custom-workflow.yaml");
        assert_eq!(
            detect_package_behavior_shape_with_manifest_hint(
                package_dir,
                Some(&workflow_and_skill_manifest),
            ),
            PackageBehaviorShape::WorkflowAndSkill
        );

        let skill_manifest = manifest_with("");
        fs::remove_file(package_dir.join("custom-workflow.yaml")).unwrap();
        fs::write(
            package_dir.join("workflow.yaml"),
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
          package: "@codemod/example"
"#,
        )
        .unwrap();
        assert_eq!(
            detect_package_behavior_shape_with_manifest_hint(package_dir, Some(&skill_manifest)),
            PackageBehaviorShape::SkillOnly
        );
    }

    #[test]
    fn unsupported_skill_install_error_recommends_run_for_workflow_packages() {
        let error = unsupported_skill_install_error(
            "@codemod/jest-to-vitest",
            Path::new("/tmp/pkg"),
            PackageBehaviorShape::WorkflowOnly,
        );
        assert!(error.contains("codemod run @codemod/jest-to-vitest"));
        assert!(error.contains("workflow-only"));
    }

    #[test]
    fn unsupported_skill_install_error_is_actionable_for_missing_behavior() {
        let error = unsupported_skill_install_error(
            "@codemod/unknown",
            Path::new("/tmp/pkg"),
            PackageBehaviorShape::Missing,
        );
        assert!(error.contains("does not provide workflow or skill behavior"));
        assert!(error.contains("agents/skill/unknown/SKILL.md"));
        assert!(error.contains("workflow.yaml"));
    }

    #[test]
    fn install_warning_is_emitted_for_skill_only_packages() {
        let warning =
            install_warning_for_shape(PackageBehaviorShape::SkillOnly, "@codemod/skill-only")
                .expect("skill-only should produce warning");
        assert!(warning.contains("skill-only package behavior"));
        assert!(warning.contains("@codemod/skill-only"));
    }

    #[test]
    fn interactive_user_scope_label_defaults_auto_to_claude_path() {
        assert_eq!(
            interactive_user_scope_label(Harness::Auto),
            "user (claude: ~/.claude/skills)"
        );
    }

    #[test]
    fn interactive_user_scope_label_uses_explicit_harness_path() {
        assert_eq!(
            interactive_user_scope_label(Harness::Cursor),
            "user (cursor: ~/.cursor/skills)"
        );
    }
}
