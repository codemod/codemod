use crate::commands::harness_adapter::{
    install_package_skill_bundle_with_runtime, install_restart_hint,
    package_skill_install_requires_force_with_runtime, resolve_adapter_with_runtime,
    runtime_paths_for_execution, upsert_skill_discovery_guides_with_runtime, Harness,
    HarnessAdapterError, InstallRequest, InstallScope, InstalledSkill, OutputFormat,
    SkillPackageInstallSpec,
};
use crate::commands::output::{format_output_path, prompt_for_overwrite_confirmation};
use crate::engine::create_registry_client_with_env;
use crate::utils::manifest::CodemodManifest;
use crate::utils::package_validation::{
    authored_skill_file_candidate, detect_package_behavior_shape_with_manifest_hint,
    AuthoredSkillFileCandidate, PackageBehaviorShape,
};
use crate::utils::skill_layout::{
    expected_authored_skill_file, find_authored_skill_dir, resolve_configured_skill_file_path,
};
use crate::{TelemetrySenderMutex, CLI_VERSION};
use anyhow::Result;
use async_trait::async_trait;
use butterflow_core::config::{InstallSkillExecutionRequest, InstallSkillExecutor};
use butterflow_core::registry::RegistryError;
use butterflow_core::structured_log::OutputFormat as WorkflowOutputFormat;
use butterflow_models::step::{InstallSkillHarness, InstallSkillScope};
use codemod_telemetry::send_event::BaseEvent;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[cfg(test)]
use crate::utils::skill_layout::SKILL_FILE_NAME;

#[derive(Clone, Debug)]
pub struct PackageSkillInstallRequest {
    pub package_id: String,
    pub configured_path: Option<String>,
    pub harness: Harness,
    pub scope: InstallScope,
    pub force: bool,
    pub no_interactive: bool,
    pub format: OutputFormat,
    pub emit_output: bool,
    pub working_directory: Option<PathBuf>,
    pub environment: Option<HashMap<String, String>>,
    /// Local codemod bundle root; when set and `codemod.yaml` matches `package_id`, skip registry.
    pub bundle_path: Option<PathBuf>,
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

struct PackageSkillInstallTelemetryInput {
    requested_harness: Harness,
    resolved_harness: Harness,
    scope: InstallScope,
    force: bool,
    format: OutputFormat,
    package_id: String,
    package_version: String,
    installed_names: Vec<String>,
    warnings_count: usize,
}

struct PackageSkillInstallExecution {
    rendered_output: String,
    telemetry_input: PackageSkillInstallTelemetryInput,
}

struct CliInstallSkillExecutor {
    telemetry: TelemetrySenderMutex,
}

#[async_trait]
impl InstallSkillExecutor for CliInstallSkillExecutor {
    async fn execute(&self, request: InstallSkillExecutionRequest) -> Result<String> {
        let (format, emit_output) = workflow_install_output_behavior(request.output_format);
        let install_request = PackageSkillInstallRequest {
            package_id: request.install_skill.package,
            configured_path: request.install_skill.path,
            harness: harness_from_step(request.install_skill.harness),
            scope: scope_from_step(request.install_skill.scope),
            force: request.install_skill.force.unwrap_or(false),
            no_interactive: request.no_interactive,
            format,
            emit_output,
            working_directory: Some(request.target_path),
            environment: Some(request.env),
            bundle_path: request.bundle_path,
        };

        install_package_skill(&install_request, &self.telemetry).await
    }
}

pub fn create_install_skill_executor(
    telemetry: TelemetrySenderMutex,
) -> Arc<dyn InstallSkillExecutor> {
    Arc::new(CliInstallSkillExecutor { telemetry })
}

pub async fn install_from_run_prompt(
    package_id: &str,
    target_path: Option<PathBuf>,
    telemetry: &TelemetrySenderMutex,
) -> Result<()> {
    install_from_run_request(package_id, false, target_path, telemetry).await?;
    Ok(())
}

pub async fn install_from_run_request(
    package_id: &str,
    no_interactive: bool,
    target_path: Option<PathBuf>,
    telemetry: &TelemetrySenderMutex,
) -> Result<String> {
    let request = PackageSkillInstallRequest {
        package_id: package_id.to_string(),
        configured_path: None,
        harness: Harness::Auto,
        scope: InstallScope::Project,
        force: false,
        no_interactive,
        format: OutputFormat::Logs,
        emit_output: true,
        working_directory: target_path,
        environment: None,
        bundle_path: None,
    };
    install_package_skill(&request, telemetry).await
}

pub async fn install_package_skill(
    request: &PackageSkillInstallRequest,
    telemetry: &TelemetrySenderMutex,
) -> Result<String> {
    let interactive = !request.no_interactive
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    let execution = execute_install_package_skill(request, interactive)
        .await
        .map_err(anyhow::Error::from)?;
    if request.emit_output {
        emit_install_output(&execution.rendered_output);
    }
    send_package_skill_install_event(telemetry, &execution.telemetry_input).await;
    Ok(execution.rendered_output)
}

async fn execute_install_package_skill(
    request: &PackageSkillInstallRequest,
    interactive: bool,
) -> std::result::Result<PackageSkillInstallExecution, HarnessAdapterError> {
    let runtime_paths = runtime_paths_for_execution(
        request.working_directory.as_deref(),
        request.environment.as_ref(),
    )?;
    let resolved_adapter = resolve_adapter_with_runtime(request.harness, &runtime_paths)?;
    let (package, mut package_warnings) = resolve_skill_package_for_install(
        &request.package_id,
        request.configured_path.as_deref(),
        request.environment.as_ref(),
        request.bundle_path.as_deref(),
    )
    .await?;

    let mut force = request.force;
    if interactive && !force {
        let overwrite_required = package_skill_install_requires_force_with_runtime(
            resolved_adapter.harness,
            request.scope,
            &package,
            &runtime_paths,
        )?;
        if overwrite_required {
            force = prompt_for_overwrite_confirmation()?;
        }
    }

    let installed = install_package_skill_bundle_with_runtime(
        resolved_adapter.harness,
        &package,
        &InstallRequest {
            scope: request.scope,
            force,
        },
        &runtime_paths,
    )?;

    let mut warnings = resolved_adapter.warnings;
    let mut notes = Vec::new();
    warnings.append(&mut package_warnings);
    match upsert_skill_discovery_guides_with_runtime(
        resolved_adapter.harness,
        request.scope,
        &runtime_paths,
    ) {
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

    let package_id = package.id.clone();
    let package_version = package.version.clone();
    let output = build_install_output(
        &package_id,
        resolved_adapter.harness,
        request.scope,
        installed,
        notes,
        warnings,
        Some(install_restart_hint(resolved_adapter.harness)),
    );
    let rendered_output = render_install_output(&output, request.format)
        .map_err(|error| HarnessAdapterError::InstallFailed(error.to_string()))?;

    Ok(PackageSkillInstallExecution {
        rendered_output,
        telemetry_input: PackageSkillInstallTelemetryInput {
            requested_harness: request.harness,
            resolved_harness: resolved_adapter.harness,
            scope: request.scope,
            force,
            format: request.format,
            package_id,
            package_version,
            installed_names: output
                .installed
                .iter()
                .map(|entry| entry.name.clone())
                .collect(),
            warnings_count: output.warnings.len(),
        },
    })
}

fn harness_from_step(harness: Option<InstallSkillHarness>) -> Harness {
    match harness.unwrap_or(InstallSkillHarness::Auto) {
        InstallSkillHarness::Auto => Harness::Auto,
        InstallSkillHarness::Claude => Harness::Claude,
        InstallSkillHarness::Goose => Harness::Goose,
        InstallSkillHarness::Opencode => Harness::Opencode,
        InstallSkillHarness::Cursor => Harness::Cursor,
        InstallSkillHarness::Codex => Harness::Codex,
        InstallSkillHarness::Antigravity => Harness::Antigravity,
    }
}

fn scope_from_step(scope: Option<InstallSkillScope>) -> InstallScope {
    match scope.unwrap_or(InstallSkillScope::Project) {
        InstallSkillScope::Project => InstallScope::Project,
        InstallSkillScope::User => InstallScope::User,
    }
}

fn workflow_install_output_behavior(output_format: WorkflowOutputFormat) -> (OutputFormat, bool) {
    match output_format {
        WorkflowOutputFormat::Text => (OutputFormat::Logs, true),
        WorkflowOutputFormat::Jsonl => (OutputFormat::Logs, false),
    }
}

async fn send_package_skill_install_event(
    telemetry: &TelemetrySenderMutex,
    input: &PackageSkillInstallTelemetryInput,
) {
    let PackageSkillInstallTelemetryInput {
        requested_harness,
        resolved_harness,
        scope,
        force,
        format,
        package_id,
        package_version,
        installed_names,
        warnings_count,
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
                    ("packageId".to_string(), package_id.clone()),
                    ("packageVersion".to_string(), package_version.clone()),
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
                        installed_names.len().to_string(),
                    ),
                    ("installedNames".to_string(), installed_names.join(",")),
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

fn render_install_output(
    output: &PackageSkillInstallOutput,
    format: OutputFormat,
) -> Result<String> {
    match format {
        OutputFormat::Logs => Ok(render_install_output_logs(output)),
        OutputFormat::Json => Ok(serde_json::to_string_pretty(output)?),
        OutputFormat::Yaml => Ok(serde_yaml::to_string(output)?),
        OutputFormat::Table => Ok(render_install_output_table(output)),
    }
}

fn emit_install_output(rendered_output: &str) {
    if rendered_output.ends_with('\n') {
        print!("{rendered_output}");
    } else {
        println!("{rendered_output}");
    }
}

fn render_install_output_logs(output: &PackageSkillInstallOutput) -> String {
    let mut rendered = format!(
        "Installed package skill `{}` for `{}` ({})",
        output.package_id, output.harness, output.scope
    );
    rendered.push('\n');

    if output.installed.is_empty() {
        rendered.push_str("No skills were installed.\n");
    } else {
        rendered.push_str("Installed components:\n");
        for installed_skill in &output.installed {
            let version = installed_skill.version.as_deref().unwrap_or("n/a");
            rendered.push_str(&format!(
                "  - {}@{} -> {}",
                installed_skill.name, version, installed_skill.path
            ));
            rendered.push('\n');
        }
    }

    if !output.notes.is_empty() {
        rendered.push_str("Notes:\n");
        for note in &output.notes {
            rendered.push_str(&format!("  - {note}\n"));
        }
    }

    if !output.warnings.is_empty() {
        rendered.push_str("Warnings:\n");
        for warning in &output.warnings {
            rendered.push_str(&format!("  - {warning}\n"));
        }
    }

    if let Some(restart_hint) = &output.restart_hint {
        rendered.push_str(&format!("🎉 {restart_hint}\n"));
    }

    rendered
}

fn render_install_output_table(output: &PackageSkillInstallOutput) -> String {
    let mut rendered = format!(
        "Package: {}\nHarness: {}\nScope: {}\n",
        output.package_id, output.harness, output.scope
    );
    if output.installed.is_empty() {
        rendered.push_str("No skills were installed.\n");
        if let Some(restart_hint) = &output.restart_hint {
            rendered.push_str(&format!("🎉 {restart_hint}\n"));
        }
        return rendered;
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
    rendered.push_str(&table.to_string());
    rendered.push('\n');

    if !output.notes.is_empty() {
        rendered.push_str("Notes:\n");
        for note in &output.notes {
            rendered.push_str(&format!("  - {note}\n"));
        }
    }
    if !output.warnings.is_empty() {
        rendered.push_str("Warnings:\n");
        for warning in &output.warnings {
            rendered.push_str(&format!("  - {warning}\n"));
        }
    }
    if let Some(restart_hint) = &output.restart_hint {
        rendered.push_str(&format!("🎉 {restart_hint}\n"));
    }

    rendered
}

async fn resolve_skill_package_for_install(
    package_id: &str,
    configured_path: Option<&str>,
    environment: Option<&HashMap<String, String>>,
    local_bundle_root: Option<&Path>,
) -> std::result::Result<(SkillPackageInstallSpec, Vec<String>), HarnessAdapterError> {
    let resolved_package = resolve_skill_package(
        package_id,
        configured_path,
        environment,
        local_bundle_root,
    )
    .await?;
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

fn canonical_package_id_from_manifest(manifest: &CodemodManifest) -> String {
    format_registry_id(
        &manifest.registry.as_ref().and_then(|r| r.scope.clone()),
        &manifest.name,
    )
}

fn local_bundle_matches_install_request(
    requested_package_id: &str,
    manifest: &CodemodManifest,
) -> bool {
    if requested_package_id == manifest.name {
        return true;
    }
    let canonical = canonical_package_id_from_manifest(manifest);
    requested_package_id == canonical
}

fn finish_resolve_skill_package(
    package_dir: &Path,
    manifest: Option<&CodemodManifest>,
    canonical_package_id: String,
    manifest_name_fallback: &str,
    version: String,
    configured_path: Option<&str>,
) -> std::result::Result<ResolvedSkillPackage, HarnessAdapterError> {
    let description = manifest
        .as_ref()
        .map(|m| m.description.clone())
        .unwrap_or_else(|| {
            format!("Install package skill for `{manifest_name_fallback}`.")
        });
    let manifest_name = manifest
        .as_ref()
        .map(|m| m.name.as_str())
        .unwrap_or(manifest_name_fallback);
    let candidate = resolve_skill_install_candidate(
        package_dir,
        manifest,
        manifest_name,
        configured_path,
        &canonical_package_id,
    )?;
    let expected_skill_file = candidate.path;
    let has_explicit_skill_path = candidate.explicit;
    let skill_source_dir = if expected_skill_file.is_file() {
        expected_skill_file.parent().map(Path::to_path_buf)
    } else if !has_explicit_skill_path {
        find_authored_skill_dir(package_dir, Some(manifest_name))
    } else {
        None
    };
    let behavior_shape =
        detect_package_behavior_shape_with_manifest_hint(package_dir, manifest);

    Ok(ResolvedSkillPackage {
        id: canonical_package_id,
        version,
        description,
        package_dir: package_dir.to_path_buf(),
        expected_skill_file,
        skill_source_dir,
        behavior_shape,
    })
}

async fn resolve_skill_package(
    package_id: &str,
    configured_path: Option<&str>,
    environment: Option<&HashMap<String, String>>,
    local_bundle_root: Option<&Path>,
) -> std::result::Result<ResolvedSkillPackage, HarnessAdapterError> {
    if let Some(bundle_root) = local_bundle_root {
        let manifest_path = bundle_root.join("codemod.yaml");
        if let Some(manifest) = read_package_manifest(&manifest_path) {
            if local_bundle_matches_install_request(package_id, &manifest) {
                let canonical_id = canonical_package_id_from_manifest(&manifest);
                return finish_resolve_skill_package(
                    bundle_root,
                    Some(&manifest),
                    canonical_id,
                    manifest.name.as_str(),
                    manifest.version.clone(),
                    configured_path,
                );
            }
        }
    }

    let registry_client = create_registry_client_with_env(None, environment).map_err(|error| {
        HarnessAdapterError::SkillPackageInstallFailed(format!(
            "failed to initialize registry client: {error}"
        ))
    })?;
    let registry_url = registry_client.config.default_registry.clone();
    let resolved_package = registry_client
        .resolve_package(package_id, Some(&registry_url), false, None)
        .await
        .map_err(|error| map_registry_error_to_install_error(package_id, error))?;

    let canonical_package_id =
        format_registry_id(&resolved_package.spec.scope, &resolved_package.spec.name);
    let manifest_path = resolved_package.package_dir.join("codemod.yaml");
    let manifest = read_package_manifest(&manifest_path);

    finish_resolve_skill_package(
        &resolved_package.package_dir,
        manifest.as_ref(),
        canonical_package_id,
        resolved_package.spec.name.as_str(),
        resolved_package.version,
        configured_path,
    )
}

fn resolve_skill_install_candidate(
    package_dir: &Path,
    manifest: Option<&CodemodManifest>,
    manifest_name: &str,
    configured_path: Option<&str>,
    package_id: &str,
) -> std::result::Result<AuthoredSkillFileCandidate, HarnessAdapterError> {
    if let Some(path) = configured_path {
        let resolved_path =
            resolve_configured_skill_file_path(package_dir, path).ok_or_else(|| {
                HarnessAdapterError::SkillPackageInstallFailed(format!(
                    "invalid install-skill path `{path}` for package `{package_id}`"
                ))
            })?;
        return Ok(AuthoredSkillFileCandidate {
            path: resolved_path,
            explicit: true,
        });
    }

    authored_skill_file_candidate(package_dir, manifest, manifest_name)
        .map_err(|error| HarnessAdapterError::SkillPackageInstallFailed(error.to_string()))
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
    fn resolve_skill_install_candidate_prefers_explicit_install_skill_path() {
        let temp_dir = tempdir().unwrap();
        let package_dir = temp_dir.path();
        let manifest = manifest_with("workflow.yaml");

        let custom_skill_dir = package_dir.join("custom-skill");
        fs::create_dir_all(&custom_skill_dir).unwrap();
        fs::write(custom_skill_dir.join(SKILL_FILE_NAME), "# Custom skill\n").unwrap();

        let candidate = resolve_skill_install_candidate(
            package_dir,
            Some(&manifest),
            &manifest.name,
            Some("./custom-skill"),
            "@codemod/example",
        )
        .unwrap();

        assert!(candidate.explicit);
        assert_eq!(candidate.path, custom_skill_dir.join(SKILL_FILE_NAME));
    }

    #[test]
    fn workflow_install_output_behavior_emits_logs_for_text_runs() {
        assert_eq!(
            workflow_install_output_behavior(WorkflowOutputFormat::Text),
            (OutputFormat::Logs, true)
        );
    }

    #[test]
    fn workflow_install_output_behavior_suppresses_stdout_for_jsonl_runs() {
        assert_eq!(
            workflow_install_output_behavior(WorkflowOutputFormat::Jsonl),
            (OutputFormat::Logs, false)
        );
    }

    /// Minimal tree: `codemod.yaml`, `workflow.yaml` with install-skill, and `agents/skill/<name>/SKILL.md`.
    fn write_minimal_installable_skill_bundle(
        root: &Path,
        manifest_name: &str,
        install_skill_package_field: &str,
        registry_scope: Option<&str>,
    ) {
        let mut manifest_yaml = format!(
            r#"schema_version: "1"
name: {name}
version: "1.0.0"
description: test
author: test
workflow: workflow.yaml
"#,
            name = manifest_name
        );
        if let Some(scope) = registry_scope {
            manifest_yaml.push_str(&format!("registry:\n  scope: {scope}\n"));
        }
        fs::write(root.join("codemod.yaml"), manifest_yaml).unwrap();

        // Quote package so scoped ids (e.g. `@scope/name`) are valid YAML.
        let workflow = format!(
            r#"version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install skill
        install-skill:
          package: "{pkg}"
"#,
            pkg = install_skill_package_field.replace('\\', "\\\\").replace('"', "\\\"")
        );
        fs::write(root.join("workflow.yaml"), workflow).unwrap();

        let skill_name =
            crate::utils::skill_layout::derive_skill_name_from_package_name(install_skill_package_field);
        let skill_dir = root.join("agents/skill").join(skill_name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join(SKILL_FILE_NAME), "# Skill\n").unwrap();
    }

    #[test]
    fn local_bundle_matches_accepts_bare_name_and_canonical_scoped_id() {
        let mut m = manifest_with("workflow.yaml");
        m.name = "mypkg".to_string();
        assert!(local_bundle_matches_install_request("mypkg", &m));
        assert!(!local_bundle_matches_install_request("otherpkg", &m));

        m.registry = Some(crate::utils::manifest::RegistryConfig {
            access: None,
            scope: Some("codemod".to_string()),
            visibility: None,
        });
        assert!(local_bundle_matches_install_request("mypkg", &m));
        assert!(local_bundle_matches_install_request("@codemod/mypkg", &m));
        assert!(!local_bundle_matches_install_request("@codemod/other", &m));
    }

    #[test]
    fn canonical_package_id_from_manifest_matches_format_registry_id() {
        let mut m = manifest_with("workflow.yaml");
        m.name = "jest-to-vitest".to_string();
        m.registry = Some(crate::utils::manifest::RegistryConfig {
            access: None,
            scope: Some("codemod".to_string()),
            visibility: None,
        });
        assert_eq!(
            canonical_package_id_from_manifest(&m),
            "@codemod/jest-to-vitest"
        );

        m.registry = None;
        assert_eq!(canonical_package_id_from_manifest(&m), "jest-to-vitest");
    }

    #[tokio::test]
    async fn resolve_skill_package_uses_local_bundle_when_bare_name_matches() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        write_minimal_installable_skill_bundle(root, "localtestpkg", "localtestpkg", None);

        let resolved = resolve_skill_package("localtestpkg", None, None, Some(root))
            .await
            .expect("expected local bundle resolution");

        assert_eq!(resolved.package_dir, root);
        assert_eq!(resolved.id, "localtestpkg");
        assert_eq!(resolved.version, "1.0.0");
        assert!(resolved.behavior_shape.includes_skill());
    }

    #[tokio::test]
    async fn resolve_skill_package_uses_local_bundle_when_scoped_id_matches() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        write_minimal_installable_skill_bundle(root, "mypkg", "@codemod/mypkg", Some("codemod"));

        let resolved = resolve_skill_package("@codemod/mypkg", None, None, Some(root))
            .await
            .expect("expected local bundle resolution");

        assert_eq!(resolved.package_dir, root);
        assert_eq!(resolved.id, "@codemod/mypkg");
        assert_eq!(resolved.version, "1.0.0");
        assert!(resolved.behavior_shape.includes_skill());
    }
}
