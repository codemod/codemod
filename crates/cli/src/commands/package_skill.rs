use crate::commands::harness_adapter::{
    resolve_adapter, resolve_install_scope, Harness, HarnessAdapterError, InstallRequest,
    InstallScope, InstalledSkill, OutputFormat, SkillPackageInstallSpec,
};
use crate::engine::create_registry_client;
use crate::utils::manifest::CodemodManifest;
use anyhow::Result;
use butterflow_core::registry::RegistryError;
use clap::Parser;
use serde::Serialize;
use std::fs;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[derive(Parser, Debug)]
struct DirectSkillInstallCommand {
    /// Package identifier
    #[arg(value_name = "PACKAGE")]
    package_id: String,
    /// Install package skill capability
    #[arg(long)]
    skill: bool,
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
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

#[derive(Serialize)]
struct PackageSkillInstallOutput {
    ok: bool,
    package_id: String,
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

pub async fn handle_direct_install(trailing_args: &[String]) -> Result<bool> {
    if trailing_args.is_empty() || !trailing_args.iter().any(|arg| arg == "--skill") {
        return Ok(false);
    }

    let parse_args = std::iter::once("codemod".to_string())
        .chain(trailing_args.iter().cloned())
        .collect::<Vec<_>>();

    let command =
        DirectSkillInstallCommand::try_parse_from(parse_args).map_err(anyhow::Error::from)?;

    if !command.skill {
        return Ok(false);
    }

    let scope = resolve_install_scope(command.project, command.user).unwrap_or_else(|error| {
        exit_adapter_error(error, command.format);
    });

    let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
        exit_adapter_error(error, command.format);
    });

    let request = InstallRequest {
        scope,
        force: command.force,
    };

    let package = resolve_skill_package_for_install(&command.package_id)
        .await
        .unwrap_or_else(|error| {
            exit_adapter_error(error, command.format);
        });

    let installed = resolved_adapter
        .adapter
        .install_package_skill(&package, &request)
        .unwrap_or_else(|error| {
            exit_adapter_error(error, command.format);
        });

    let output = build_install_output(
        &package.id,
        resolved_adapter.harness,
        scope,
        installed,
        resolved_adapter.warnings,
    );
    print_install_output(&output, command.format)?;

    Ok(true)
}

fn build_install_output(
    package_id: &str,
    harness: Harness,
    scope: InstallScope,
    installed: Vec<InstalledSkill>,
    warnings: Vec<String>,
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
        warnings,
    }
}

fn print_install_output(output: &PackageSkillInstallOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(output)?),
        OutputFormat::Yaml => println!("{}", serde_yaml::to_string(output)?),
        OutputFormat::Table => print_install_output_table(output),
    }

    Ok(())
}

fn print_install_output_table(output: &PackageSkillInstallOutput) {
    println!("Package: {}", output.package_id);
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

async fn resolve_skill_package_for_install(
    package_id: &str,
) -> std::result::Result<SkillPackageInstallSpec, HarnessAdapterError> {
    let resolved_package = resolve_skill_package(package_id).await?;
    Ok(SkillPackageInstallSpec {
        id: resolved_package.id,
        version: resolved_package.version,
        description: resolved_package.description,
    })
}

#[derive(Debug)]
struct ResolvedSkillPackage {
    id: String,
    version: String,
    description: String,
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
                "Install package skill capability for `{}`.",
                resolved_package.spec.name
            )
        });

    Ok(ResolvedSkillPackage {
        id: package_id,
        version: resolved_package.version,
        description,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
