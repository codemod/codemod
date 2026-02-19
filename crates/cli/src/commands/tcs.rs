use crate::commands::harness_adapter::{
    resolve_adapter, resolve_install_scope, Harness, HarnessAdapterError, InstallRequest,
    InstallScope, InstalledSkill, OutputFormat, TcsInstallPackage,
};
use crate::engine::create_registry_client;
use crate::utils::manifest::CodemodManifest;
use anyhow::{bail, Result};
use butterflow_core::registry::RegistryError;
use clap::{Args, Subcommand};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: TcsAction,
}

#[derive(Subcommand, Debug)]
enum TcsAction {
    /// Install a task-specific codemod skill package
    Install(InstallCommand),
    /// Return metadata for a task-specific codemod
    Inspect(InspectCommand),
    /// Run a task-specific codemod explicitly
    Run(RunCommand),
}

#[derive(Args, Debug)]
struct InstallCommand {
    /// TCS package identifier
    #[arg(value_name = "TCS_ID")]
    tcs_id: String,
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

#[derive(Args, Debug)]
struct InspectCommand {
    /// TCS package identifier
    #[arg(value_name = "TCS_ID")]
    tcs_id: String,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct RunCommand {
    /// TCS package identifier
    #[arg(value_name = "TCS_ID")]
    tcs_id: String,
    /// Optional target path for transformation
    #[arg(long)]
    target: Option<PathBuf>,
    /// Run in dry-run mode
    #[arg(long)]
    dry_run: bool,
    /// Parameters passed to TCS runtime in key=value format
    #[arg(long = "param", value_name = "KEY=VALUE")]
    params: Vec<String>,
    /// Existing session identifier
    #[arg(long)]
    session: Option<String>,
    /// Directory used for run artifacts
    #[arg(long)]
    artifacts_dir: Option<PathBuf>,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

pub async fn handler(args: &Command) -> Result<()> {
    match &args.action {
        TcsAction::Install(command) => {
            let scope =
                resolve_install_scope(command.project, command.user).unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let _ = resolved_adapter.adapter.metadata();
            let request = InstallRequest {
                scope,
                force: command.force,
            };
            let resolved_tcs_package = resolve_tcs_package_for_install(&command.tcs_id)
                .await
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let installed = resolved_adapter
                .adapter
                .install_tcs_skill(&resolved_tcs_package, &request)
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });

            let output = build_install_output(
                &resolved_tcs_package.id,
                resolved_adapter.harness,
                scope,
                installed,
                resolved_adapter.warnings,
            );
            print_install_output(&output, command.format)?;
            Ok(())
        }
        TcsAction::Inspect(command) => {
            let resolved_tcs_package =
                resolve_tcs_package(&command.tcs_id)
                    .await
                    .unwrap_or_else(|error| {
                        exit_adapter_error(error, command.format);
                    });
            let output = build_inspect_output(&resolved_tcs_package);
            print_inspect_output(&output, command.format)?;
            Ok(())
        }
        TcsAction::Run(command) => {
            let _ = (
                &command.tcs_id,
                &command.target,
                command.dry_run,
                &command.params,
                &command.session,
                &command.artifacts_dir,
                command.format,
            );
            bail!("tcs run is not implemented yet")
        }
    }
}

#[derive(Serialize)]
struct TcsInstallOutput {
    ok: bool,
    tcs_id: String,
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
struct TcsInspectOutput {
    ok: bool,
    id: String,
    description: String,
    version: String,
    supports: TcsInspectSupports,
    constraints: TcsInspectConstraints,
    confidence_hints: Vec<String>,
}

#[derive(Serialize)]
struct TcsInspectSupports {
    languages: Vec<String>,
    frameworks: Vec<String>,
    version_ranges: Vec<String>,
}

#[derive(Serialize)]
struct TcsInspectConstraints {
    capabilities: Vec<String>,
    requires_tests: Option<bool>,
    min_test_coverage: Option<u32>,
}

#[derive(Tabled)]
struct TcsInspectRow {
    #[tabled(rename = "TCS")]
    id: String,
    #[tabled(rename = "Version")]
    version: String,
    #[tabled(rename = "Languages")]
    languages: String,
    #[tabled(rename = "Frameworks")]
    frameworks: String,
    #[tabled(rename = "Version Ranges")]
    version_ranges: String,
    #[tabled(rename = "Capabilities")]
    capabilities: String,
    #[tabled(rename = "Requires Tests")]
    requires_tests: String,
    #[tabled(rename = "Min Coverage")]
    min_test_coverage: String,
    #[tabled(rename = "Confidence Hints")]
    confidence_hints: String,
}

fn build_install_output(
    tcs_id: &str,
    harness: Harness,
    scope: InstallScope,
    installed: Vec<InstalledSkill>,
    warnings: Vec<String>,
) -> TcsInstallOutput {
    TcsInstallOutput {
        ok: true,
        tcs_id: tcs_id.to_string(),
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

fn print_install_output(output: &TcsInstallOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(output)?),
        OutputFormat::Yaml => println!("{}", serde_yaml::to_string(output)?),
        OutputFormat::Table => print_install_output_table(output),
    }

    Ok(())
}

fn print_install_output_table(output: &TcsInstallOutput) {
    println!("TCS: {}", output.tcs_id);
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

fn build_inspect_output(resolved_tcs_package: &ResolvedTcsPackage) -> TcsInspectOutput {
    let (languages, frameworks, version_ranges) = match &resolved_tcs_package.manifest {
        Some(manifest) => extract_supports(manifest),
        None => (Vec::new(), Vec::new(), Vec::new()),
    };
    let (capabilities, requires_tests, min_test_coverage) = match &resolved_tcs_package.manifest {
        Some(manifest) => extract_constraints(manifest),
        None => (Vec::new(), None, None),
    };
    let confidence_hints = match &resolved_tcs_package.manifest {
        Some(manifest) => build_confidence_hints(manifest),
        None => Vec::new(),
    };

    TcsInspectOutput {
        ok: true,
        id: resolved_tcs_package.id.clone(),
        description: resolved_tcs_package.description.clone(),
        version: resolved_tcs_package.version.clone(),
        supports: TcsInspectSupports {
            languages,
            frameworks,
            version_ranges,
        },
        constraints: TcsInspectConstraints {
            capabilities,
            requires_tests,
            min_test_coverage,
        },
        confidence_hints,
    }
}

fn print_inspect_output(output: &TcsInspectOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(output)?),
        OutputFormat::Yaml => println!("{}", serde_yaml::to_string(output)?),
        OutputFormat::Table => print_inspect_output_table(output),
    }

    Ok(())
}

fn print_inspect_output_table(output: &TcsInspectOutput) {
    println!("TCS: {}", output.id);
    println!("Description: {}", output.description);

    let row = TcsInspectRow {
        id: output.id.clone(),
        version: output.version.clone(),
        languages: join_or_dash(&output.supports.languages),
        frameworks: join_or_dash(&output.supports.frameworks),
        version_ranges: join_or_dash(&output.supports.version_ranges),
        capabilities: join_or_dash(&output.constraints.capabilities),
        requires_tests: output
            .constraints
            .requires_tests
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        min_test_coverage: output
            .constraints
            .min_test_coverage
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        confidence_hints: join_or_dash(&output.confidence_hints),
    };

    let mut table = Table::new(vec![row]);
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

async fn resolve_tcs_package_for_install(
    tcs_id: &str,
) -> std::result::Result<TcsInstallPackage, HarnessAdapterError> {
    let resolved_tcs_package = resolve_tcs_package(tcs_id).await?;
    Ok(TcsInstallPackage {
        id: resolved_tcs_package.id,
        version: resolved_tcs_package.version,
        description: resolved_tcs_package.description,
    })
}

#[derive(Debug)]
struct ResolvedTcsPackage {
    id: String,
    version: String,
    description: String,
    manifest: Option<CodemodManifest>,
}

async fn resolve_tcs_package(
    tcs_id: &str,
) -> std::result::Result<ResolvedTcsPackage, HarnessAdapterError> {
    let registry_client = create_registry_client(None).map_err(|error| {
        HarnessAdapterError::TcsInstallFailed(format!(
            "failed to initialize registry client: {error}"
        ))
    })?;
    let registry_url = registry_client.config.default_registry.clone();
    let resolved_package = registry_client
        .resolve_package(tcs_id, Some(&registry_url), false, None)
        .await
        .map_err(|error| map_registry_error_to_tcs_error(tcs_id, error))?;

    let package_id = format_registry_id(&resolved_package.spec.scope, &resolved_package.spec.name);
    let manifest_path = resolved_package.package_dir.join("codemod.yaml");
    let manifest = read_tcs_manifest(&manifest_path);
    let description = manifest
        .as_ref()
        .map(|parsed_manifest| parsed_manifest.description.clone())
        .unwrap_or_else(|| {
            format!(
                "Inspect task-specific codemod package `{}`.",
                resolved_package.spec.name
            )
        });

    Ok(ResolvedTcsPackage {
        id: package_id,
        version: resolved_package.version,
        description,
        manifest,
    })
}

fn map_registry_error_to_tcs_error(tcs_id: &str, error: RegistryError) -> HarnessAdapterError {
    match error {
        RegistryError::PackageNotFound { .. }
        | RegistryError::VersionNotFound { .. }
        | RegistryError::NoVersionAvailable { .. }
        | RegistryError::InvalidScopedPackageName { .. } => {
            HarnessAdapterError::TcsNotFound(tcs_id.to_string())
        }
        other_error => HarnessAdapterError::TcsInstallFailed(format!(
            "failed to resolve package `{tcs_id}`: {other_error}"
        )),
    }
}

fn read_tcs_manifest(manifest_path: &std::path::Path) -> Option<CodemodManifest> {
    if !manifest_path.exists() {
        return None;
    }

    let manifest_content = fs::read_to_string(manifest_path).ok()?;
    serde_yaml::from_str(&manifest_content).ok()
}

fn extract_supports(manifest: &CodemodManifest) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut languages = manifest
        .targets
        .as_ref()
        .and_then(|targets| targets.languages.clone())
        .unwrap_or_default();
    languages.sort();
    languages.dedup();

    let mut frameworks = manifest
        .targets
        .as_ref()
        .and_then(|targets| targets.frameworks.clone())
        .unwrap_or_default();
    frameworks.sort();
    frameworks.dedup();

    let mut version_ranges = manifest
        .targets
        .as_ref()
        .and_then(|targets| targets.versions.as_ref())
        .map(|version_map| {
            version_map
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    version_ranges.sort();

    (languages, frameworks, version_ranges)
}

fn extract_constraints(manifest: &CodemodManifest) -> (Vec<String>, Option<bool>, Option<u32>) {
    let mut capabilities = manifest.capabilities.clone().unwrap_or_default();
    capabilities.sort();
    capabilities.dedup();

    let requires_tests = manifest
        .validation
        .as_ref()
        .and_then(|validation| validation.require_tests);
    let min_test_coverage = manifest
        .validation
        .as_ref()
        .and_then(|validation| validation.min_test_coverage);

    (capabilities, requires_tests, min_test_coverage)
}

fn build_confidence_hints(manifest: &CodemodManifest) -> Vec<String> {
    let mut hints = Vec::new();

    if let Some(targets) = &manifest.targets {
        if targets.languages.as_ref().map(|items| !items.is_empty()) == Some(true)
            || targets.frameworks.as_ref().map(|items| !items.is_empty()) == Some(true)
            || targets.versions.as_ref().map(|items| !items.is_empty()) == Some(true)
        {
            hints.push("target metadata declared".to_string());
        }
    }

    if let Some(validation) = &manifest.validation {
        if validation.strict == Some(true) {
            hints.push("strict validation enabled".to_string());
        }
        if validation.require_tests == Some(true) {
            hints.push("test requirement declared".to_string());
        }
        if validation.min_test_coverage.is_some() {
            hints.push("minimum test coverage declared".to_string());
        }
    }

    if manifest
        .capabilities
        .as_ref()
        .map(|capabilities| !capabilities.is_empty())
        == Some(true)
    {
        hints.push("capability requirements declared".to_string());
    }

    hints
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(", ")
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::manifest::{TargetConfig, ValidationConfig};
    use std::collections::HashMap;

    fn sample_manifest() -> CodemodManifest {
        CodemodManifest {
            schema_version: "1.0.0".to_string(),
            name: "jest-to-vitest".to_string(),
            version: "1.2.3".to_string(),
            description: "Migrate Jest tests to Vitest".to_string(),
            author: "codemod".to_string(),
            license: None,
            copyright: None,
            repository: None,
            homepage: None,
            bugs: None,
            registry: None,
            workflow: "workflow.yaml".to_string(),
            targets: Some(TargetConfig {
                languages: Some(vec!["typescript".to_string(), "javascript".to_string()]),
                frameworks: Some(vec!["react".to_string()]),
                versions: Some(HashMap::from([
                    ("jest".to_string(), ">=27".to_string()),
                    ("vitest".to_string(), ">=1".to_string()),
                ])),
            }),
            dependencies: None,
            keywords: None,
            category: None,
            readme: None,
            changelog: None,
            documentation: None,
            validation: Some(ValidationConfig {
                strict: Some(true),
                require_tests: Some(true),
                min_test_coverage: Some(85),
            }),
            capabilities: Some(vec!["fetch".to_string(), "fs".to_string()]),
        }
    }

    #[test]
    fn map_registry_error_converts_not_found_to_tcs_not_found() {
        let mapped = map_registry_error_to_tcs_error(
            "jest-to-vitest",
            RegistryError::PackageNotFound {
                package: "jest-to-vitest".to_string(),
            },
        );
        assert!(matches!(mapped, HarnessAdapterError::TcsNotFound(id) if id == "jest-to-vitest"));
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
    fn extract_supports_returns_sorted_support_fields() {
        let manifest = sample_manifest();
        let (languages, frameworks, version_ranges) = extract_supports(&manifest);

        assert_eq!(
            languages,
            vec!["javascript".to_string(), "typescript".to_string()]
        );
        assert_eq!(frameworks, vec!["react".to_string()]);
        assert_eq!(
            version_ranges,
            vec!["jest=>=27".to_string(), "vitest=>=1".to_string()]
        );
    }

    #[test]
    fn build_inspect_output_includes_constraints_and_confidence_hints() {
        let manifest = sample_manifest();
        let resolved_tcs_package = ResolvedTcsPackage {
            id: "@codemod/jest-to-vitest".to_string(),
            version: "1.2.3".to_string(),
            description: "Migrate Jest tests to Vitest".to_string(),
            manifest: Some(manifest),
        };

        let output = build_inspect_output(&resolved_tcs_package);

        assert!(output.ok);
        assert_eq!(output.id, "@codemod/jest-to-vitest");
        assert_eq!(output.version, "1.2.3");
        assert_eq!(
            output.constraints.capabilities,
            vec!["fetch".to_string(), "fs".to_string()]
        );
        assert_eq!(output.constraints.requires_tests, Some(true));
        assert_eq!(output.constraints.min_test_coverage, Some(85));
        assert!(output
            .confidence_hints
            .contains(&"strict validation enabled".to_string()));
        assert!(output
            .confidence_hints
            .contains(&"target metadata declared".to_string()));
    }

    #[test]
    fn build_inspect_output_handles_missing_manifest() {
        let resolved_tcs_package = ResolvedTcsPackage {
            id: "jest-to-vitest".to_string(),
            version: "1.2.3".to_string(),
            description: "Migrate Jest tests to Vitest".to_string(),
            manifest: None,
        };

        let output = build_inspect_output(&resolved_tcs_package);
        assert!(output.supports.languages.is_empty());
        assert!(output.supports.frameworks.is_empty());
        assert!(output.supports.version_ranges.is_empty());
        assert!(output.constraints.capabilities.is_empty());
        assert!(output.confidence_hints.is_empty());
    }

    #[test]
    fn inspect_output_json_contract_has_required_top_level_fields() {
        let resolved_tcs_package = ResolvedTcsPackage {
            id: "jest-to-vitest".to_string(),
            version: "1.2.3".to_string(),
            description: "Migrate Jest tests to Vitest".to_string(),
            manifest: None,
        };
        let output = build_inspect_output(&resolved_tcs_package);
        let json = serde_json::to_value(output).unwrap();

        assert_eq!(json.get("ok").and_then(|value| value.as_bool()), Some(true));
        assert_eq!(
            json.get("id").and_then(|value| value.as_str()),
            Some("jest-to-vitest")
        );
        assert!(json.get("supports").is_some());
        assert!(json.get("constraints").is_some());
        assert!(json.get("confidence_hints").is_some());
    }
}
