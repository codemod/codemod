use crate::commands::harness_adapter::{
    install_restart_hint, resolve_adapter, resolve_install_scope, upsert_skill_discovery_guides,
    Harness, HarnessAdapterError, InstallRequest, InstallScope, InstalledSkill, OutputFormat,
    SkillPackageInstallSpec,
};
use crate::engine::create_registry_client;
use crate::utils::manifest::CodemodManifest;
use crate::utils::skill_layout::{expected_authored_skill_file, find_authored_skill_dir};
use anyhow::Result;
use butterflow_core::registry::RegistryError;
use clap::error::ErrorKind;
use clap::Parser;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[cfg(test)]
use crate::utils::skill_layout::SKILL_FILE_NAME;

const DEFAULT_WORKFLOW_FILE_NAME: &str = "workflow.yaml";
const SKILL_PROVIDES_NAMES: [&str; 1] = ["skill"];
const WORKFLOW_PROVIDES_NAMES: [&str; 1] = ["workflow"];

#[derive(Parser, Debug)]
struct DirectSkillInstallCommand {
    /// Package identifier
    #[arg(value_name = "PACKAGE")]
    package_id: String,
    /// Install package skill behavior
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageBehaviorShape {
    WorkflowOnly,
    SkillOnly,
    Hybrid,
    Missing,
}

impl PackageBehaviorShape {
    fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowOnly => "workflow-only",
            Self::SkillOnly => "skill-only",
            Self::Hybrid => "hybrid",
            Self::Missing => "missing-behavior",
        }
    }

    fn supports_skill(self) -> bool {
        matches!(self, Self::SkillOnly | Self::Hybrid)
    }

    fn install_warning(self, package_id: &str) -> Option<String> {
        match self {
            Self::SkillOnly => Some(format!(
                "Detected skill-only package behavior for `{package_id}`. Installing this package as a harness skill."
            )),
            _ => None,
        }
    }
}

pub async fn handle_direct_install(trailing_args: &[String]) -> Result<bool> {
    if trailing_args.is_empty() || !trailing_args.iter().any(|arg| arg == "--skill") {
        return Ok(false);
    }

    let parse_args = std::iter::once("codemod".to_string())
        .chain(trailing_args.iter().cloned())
        .collect::<Vec<_>>();

    let command = match DirectSkillInstallCommand::try_parse_from(parse_args) {
        Ok(command) => command,
        Err(parse_error) => {
            if matches!(
                parse_error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                let _ = parse_error.print();
                return Ok(true);
            }
            return Err(parse_error.into());
        }
    };

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

    let (package, mut package_warnings) = resolve_skill_package_for_install(&command.package_id)
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

    let mut warnings = resolved_adapter.warnings;
    warnings.append(&mut package_warnings);
    match upsert_skill_discovery_guides(resolved_adapter.harness, scope) {
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
            "Installed skill, but failed to update AGENTS.md/CLAUDE.md discovery hints: {error}"
        )),
    }
    warnings.push(install_restart_hint(resolved_adapter.harness));

    let output = build_install_output(
        &package.id,
        resolved_adapter.harness,
        scope,
        installed,
        warnings,
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
) -> std::result::Result<(SkillPackageInstallSpec, Vec<String>), HarnessAdapterError> {
    let resolved_package = resolve_skill_package(package_id).await?;
    if !resolved_package.behavior_shape.supports_skill() {
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
            expected_authored_skill_file(&resolved_package.package_dir, &resolved_package.id)
                .display()
        ))
    })?;

    let mut warnings = Vec::new();
    if let Some(warning) = resolved_package
        .behavior_shape
        .install_warning(&resolved_package.id)
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
    let skill_source_dir =
        find_authored_skill_dir(&resolved_package.package_dir, Some(manifest_name));
    let behavior_shape = detect_package_behavior_shape(
        &resolved_package.package_dir,
        manifest.as_ref(),
        skill_source_dir.as_ref(),
    );

    Ok(ResolvedSkillPackage {
        id: package_id,
        version: resolved_package.version,
        description,
        package_dir: resolved_package.package_dir,
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

fn detect_package_behavior_shape(
    package_dir: &Path,
    manifest: Option<&CodemodManifest>,
    authored_skill_dir: Option<&PathBuf>,
) -> PackageBehaviorShape {
    let has_skill_file = authored_skill_dir.is_some()
        || find_authored_skill_dir(package_dir, manifest.map(|manifest| manifest.name.as_str()))
            .is_some();
    let has_workflow_file = has_workflow_file(package_dir, manifest);
    let declares_skill = manifest
        .is_some_and(|manifest| manifest_declares_provides(manifest, &SKILL_PROVIDES_NAMES));
    let declares_workflow = manifest
        .is_some_and(|manifest| manifest_declares_provides(manifest, &WORKFLOW_PROVIDES_NAMES));

    let supports_skill = has_skill_file || declares_skill;
    let supports_workflow = has_workflow_file || declares_workflow;

    match (supports_workflow, supports_skill) {
        (true, true) => PackageBehaviorShape::Hybrid,
        (true, false) => PackageBehaviorShape::WorkflowOnly,
        (false, true) => PackageBehaviorShape::SkillOnly,
        (false, false) => PackageBehaviorShape::Missing,
    }
}

fn has_workflow_file(package_dir: &Path, manifest: Option<&CodemodManifest>) -> bool {
    let has_default_workflow = package_dir.join(DEFAULT_WORKFLOW_FILE_NAME).is_file();
    let has_manifest_workflow = manifest
        .and_then(|manifest| manifest.workflow.as_deref())
        .map(str::trim)
        .filter(|workflow_path| !workflow_path.is_empty())
        .is_some_and(|workflow_path| package_dir.join(workflow_path).is_file());

    has_default_workflow || has_manifest_workflow
}

fn manifest_declares_provides(manifest: &CodemodManifest, expected: &[&str]) -> bool {
    manifest.provides.as_ref().is_some_and(|provides| {
        provides
            .iter()
            .map(|provide| normalize_provide_name(provide))
            .any(|provide| expected.contains(&provide.as_str()))
    })
}

fn normalize_provide_name(raw_provide: &str) -> String {
    raw_provide.trim().to_ascii_lowercase().replace('_', "-")
}

fn unsupported_skill_install_error(
    package_id: &str,
    package_dir: &Path,
    behavior_shape: PackageBehaviorShape,
) -> String {
    let expected_skill_file = expected_authored_skill_file(package_dir, package_id);

    match behavior_shape {
        PackageBehaviorShape::WorkflowOnly => format!(
            "Package `{package_id}` at {} does not provide skill behavior (detected `{}`). Install this package with `codemod run {package_id}` instead of `--skill`.",
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
    use std::fs;
    use tempfile::tempdir;

    fn manifest_with(workflow: &str, provides: Option<Vec<&str>>) -> CodemodManifest {
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
            workflow: Some(workflow.to_string()),
            targets: None,
            dependencies: None,
            keywords: None,
            category: None,
            readme: None,
            changelog: None,
            documentation: None,
            validation: None,
            provides: provides.map(|entries| entries.into_iter().map(str::to_string).collect()),
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
        assert_eq!(
            detect_package_behavior_shape(package_dir, None, None),
            PackageBehaviorShape::SkillOnly
        );

        fs::write(
            package_dir.join(DEFAULT_WORKFLOW_FILE_NAME),
            "version: \"1\"\n",
        )
        .unwrap();
        assert_eq!(
            detect_package_behavior_shape(package_dir, None, None),
            PackageBehaviorShape::Hybrid
        );
    }

    #[test]
    fn detect_package_behavior_shape_workflow_only_when_skill_missing() {
        let temp_dir = tempdir().unwrap();
        let package_dir = temp_dir.path();
        fs::write(
            package_dir.join(DEFAULT_WORKFLOW_FILE_NAME),
            "version: \"1\"\n",
        )
        .unwrap();

        assert_eq!(
            detect_package_behavior_shape(package_dir, None, None),
            PackageBehaviorShape::WorkflowOnly
        );
    }

    #[test]
    fn detect_package_behavior_shape_uses_manifest_provides_hints() {
        let temp_dir = tempdir().unwrap();
        let package_dir = temp_dir.path();
        fs::write(package_dir.join("custom-workflow.yaml"), "version: \"1\"\n").unwrap();
        let authored_skill_dir = package_dir.join("agents/skill").join("example");
        fs::create_dir_all(&authored_skill_dir).unwrap();
        fs::write(authored_skill_dir.join(SKILL_FILE_NAME), "# Skill\n").unwrap();

        let hybrid_manifest =
            manifest_with("custom-workflow.yaml", Some(vec!["Skill", "workflow"]));
        assert_eq!(
            detect_package_behavior_shape(package_dir, Some(&hybrid_manifest), None),
            PackageBehaviorShape::Hybrid
        );

        let skill_manifest = manifest_with("custom-workflow.yaml", Some(vec!["skill"]));
        fs::remove_file(package_dir.join("custom-workflow.yaml")).unwrap();
        assert_eq!(
            detect_package_behavior_shape(package_dir, Some(&skill_manifest), None),
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
        let warning = PackageBehaviorShape::SkillOnly
            .install_warning("@codemod/skill-only")
            .expect("skill-only should produce warning");
        assert!(warning.contains("skill-only package behavior"));
        assert!(warning.contains("@codemod/skill-only"));
    }
}
