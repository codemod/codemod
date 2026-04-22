use crate::utils::manifest::CodemodManifest;
use crate::utils::package_validation::{
    detect_package_behavior_shape, expected_workflow_path, validate_package_behavior_structure,
    validate_skill_behavior, PackageBehaviorShape,
};
use crate::utils::rolldown_bundler::{RolldownBundler, RolldownBundlerConfig};
use anyhow::{anyhow, Result};
use butterflow_core::utils::validate_workflow;
use butterflow_core::Workflow;
use butterflow_models::step::StepAction;
use clap::Args;
use console::style;
use log::{debug, info, warn};
use regex::Regex;
use reqwest;
use serde::Deserialize;
use serde_json::Value;
use serde_yaml;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use walkdir::WalkDir;

#[cfg(test)]
use crate::utils::package_validation::DEFAULT_WORKFLOW_FILE_NAME;
#[cfg(test)]
use crate::utils::skill_layout::expected_authored_skill_file;

use crate::auth::storage::StoredAuth;
use crate::auth::TokenStorage;
use crate::{TelemetrySenderMutex, CLI_VERSION};
use codemod_telemetry::send_event::BaseEvent;

#[derive(Args, Debug)]
pub struct Command {
    /// Path to codemod directory
    path: Option<PathBuf>,
}

#[derive(Deserialize, Debug)]
struct PublishResponse {
    success: bool,
    package: PublishedPackage,
}

#[derive(Deserialize, Debug)]
struct PublishedPackage {
    #[allow(dead_code)]
    id: String,
    name: String,
    version: String,
    scope: Option<String>,
    published_at: String,
}

enum PublishAuthSource {
    EnvironmentToken(String),
    StoredAuth(Box<StoredAuth>),
}

impl PublishAuthSource {
    fn access_token(&self) -> &str {
        match self {
            Self::EnvironmentToken(token) => token,
            Self::StoredAuth(auth) => &auth.tokens.access_token,
        }
    }
}

pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    let package_path = args
        .path
        .as_ref()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve package path: {}", e))?;

    info!("Publishing codemod from: {}", package_path.display());

    let manifest = load_manifest(&package_path)?;

    // Get registry configuration
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    let registry_url = config.default_registry.clone();
    let auth_source = resolve_publish_auth_source(&storage, &registry_url)?;
    let effective_manifest = resolve_effective_manifest(manifest)?;

    // Validate package structure and get JS files to bundle
    let js_files_to_bundle = validate_package_structure(&package_path, &effective_manifest)?;

    // Create package bundle with bundled JS files
    let bundle_path =
        create_package_bundle(&package_path, &effective_manifest, &js_files_to_bundle).await?;

    // Upload package
    let response = upload_package(
        &registry_url,
        &bundle_path,
        &effective_manifest,
        auth_source.access_token(),
    )
    .await?;

    if !response.success {
        return Err(anyhow!("Failed to publish package"));
    }

    telemetry
        .send_event(
            BaseEvent {
                kind: "codemodPublished".to_string(),
                properties: HashMap::from([
                    ("codemodName".to_string(), effective_manifest.name.clone()),
                    ("version".to_string(), effective_manifest.version.clone()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;

    println!(
        "{} Package published successfully!",
        style("✓").green().bold()
    );
    println!(
        "  {} {}",
        style("Package:").dim(),
        style(format_package_name(&response.package)).cyan()
    );
    println!(
        "  {} {}",
        style("Version:").dim(),
        style(&response.package.version).cyan()
    );
    println!(
        "  {} {}",
        style("Published:").dim(),
        style(&response.package.published_at).cyan()
    );

    // Clean up temporary bundle
    if let Err(e) = fs::remove_file(&bundle_path) {
        warn!("Failed to clean up temporary bundle: {e}");
    }

    Ok(())
}

fn resolve_effective_manifest(mut manifest: CodemodManifest) -> Result<CodemodManifest> {
    let author = manifest.author.trim().to_string();
    if author.is_empty() {
        return Err(anyhow!(
            "Package author is missing. Set 'author' explicitly in codemod.yaml."
        ));
    }

    manifest.author = author;
    Ok(manifest)
}

fn load_manifest(package_path: &Path) -> Result<CodemodManifest> {
    let manifest_path = package_path.join("codemod.yaml");

    if !manifest_path.exists() {
        return Err(anyhow!(
            "codemod.yaml not found in {}",
            package_path.display()
        ));
    }

    let manifest_content = fs::read_to_string(&manifest_path)?;
    let manifest: CodemodManifest = serde_yaml::from_str(&manifest_content).map_err(|e| {
        if e.to_string().contains("missing field `author`") {
            anyhow!("Package author is missing. Set 'author' explicitly in codemod.yaml.")
        } else {
            anyhow!("Failed to parse codemod.yaml: {}", e)
        }
    })?;

    debug!(
        "Loaded manifest for package: {} v{}",
        manifest.name, manifest.version
    );
    Ok(manifest)
}

/// Find all JS files used in JS AST grep steps
fn find_js_files_in_workflow(workflow: &Workflow, package_path: &Path) -> Result<Vec<String>> {
    let mut js_files = Vec::new();

    for node in &workflow.nodes {
        for step in &node.steps {
            if let StepAction::JSAstGrep(js_step) = &step.action {
                let js_file_path = package_path.join(&js_step.js_file);
                if !js_file_path.exists() {
                    return Err(anyhow!(
                        "JS file referenced in workflow not found: {}",
                        js_file_path.display()
                    ));
                }
                js_files.push(js_step.js_file.clone());
            }
        }
    }

    info!(
        "Found {} JS files to bundle: {:?}",
        js_files.len(),
        js_files
    );
    Ok(js_files)
}

/// Bundle a JavaScript file and return the bundled code
async fn bundle_js_file(package_path: &Path, js_file: &str) -> Result<String> {
    let js_file_path = package_path.join(js_file);

    debug!("Bundling JS file: {}", js_file_path.display());

    let config = RolldownBundlerConfig {
        entry_path: js_file_path.clone(),
        base_dir: Some(package_path.to_path_buf()),
        output_path: None, // Return code directly, don't write to file
        source_maps: false,
    };

    let bundler = RolldownBundler::new(config);
    let bundle_result = bundler
        .bundle()
        .await
        .map_err(|e| anyhow!("Failed to bundle {js_file}:\n{e}"))?;

    info!(
        "Successfully bundled {} ({} bytes)",
        js_file,
        bundle_result.code.len()
    );
    Ok(bundle_result.code)
}

fn validate_package_structure(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> Result<Vec<String>> {
    validate_package_behavior_structure(package_path, manifest)?;
    validate_common_package_metadata(package_path, manifest)?;

    let behavior_shape = detect_package_behavior_shape(package_path, manifest);
    if behavior_shape == PackageBehaviorShape::Missing {
        return Err(anyhow!(
            "Invalid package structure in {}: package must include executable workflow steps and/or skill installation steps with authored skill files.",
            package_path.display(),
        ));
    }

    let workflow_path = expected_workflow_path(package_path, manifest);
    if !workflow_path.exists() {
        return Err(anyhow!(
            "Workflow file not found: {}",
            workflow_path.display()
        ));
    }
    let js_files = validate_workflow_behavior(package_path, &workflow_path)?;

    if behavior_shape.includes_skill() || behavior_shape == PackageBehaviorShape::SkillOnly {
        validate_skill_behavior(package_path, manifest)?;
    }

    if !behavior_shape.includes_workflow() {
        info!("Skill-only package validation successful");
        info!(
            "Package validation successful ({})",
            behavior_shape.as_str()
        );
        return Ok(js_files);
    }

    info!(
        "Package validation successful ({})",
        behavior_shape.as_str()
    );
    Ok(js_files)
}

fn validate_common_package_metadata(package_path: &Path, manifest: &CodemodManifest) -> Result<()> {
    // Check optional files
    if let Some(readme) = &manifest.readme {
        let readme_path = package_path.join(readme);
        if !readme_path.exists() {
            warn!("README file not found: {}", readme_path.display());
        }
    }

    // Validate package name format
    if !is_valid_package_name(&manifest.name) {
        return Err(anyhow!("Invalid package name: {}. Must contain only lowercase letters, numbers, hyphens, and underscores.", manifest.name));
    }

    // Validate version format (semver)
    if !is_valid_semver(&manifest.version) {
        return Err(anyhow!(
            "Invalid version: {}. Must be valid semantic version (x.y.z).",
            manifest.version
        ));
    }

    // Check package size
    let package_size = calculate_package_size(package_path)?;
    const MAX_PACKAGE_SIZE: u64 = 50 * 1024 * 1024; // 50MB

    if package_size > MAX_PACKAGE_SIZE {
        return Err(anyhow!(
            "Package too large: {} bytes. Maximum allowed: {} bytes.",
            package_size,
            MAX_PACKAGE_SIZE
        ));
    }

    Ok(())
}

fn validate_workflow_behavior(package_path: &Path, workflow_path: &Path) -> Result<Vec<String>> {
    // Validate workflow file
    let workflow_content = fs::read_to_string(workflow_path)?;
    let workflow: Workflow = serde_yaml::from_str(&workflow_content)
        .map_err(|e| anyhow!("Invalid workflow YAML: {}", e))?;

    let validation_result = validate_workflow(&workflow, package_path);
    if let Err(e) = validation_result {
        return Err(anyhow!("Invalid workflow: {}", e));
    }

    // Find all JS AST grep steps that need bundling
    find_js_files_in_workflow(&workflow, package_path)
}

async fn create_package_bundle(
    package_path: &Path,
    manifest: &CodemodManifest,
    js_files_to_bundle: &[String],
) -> Result<PathBuf> {
    let temp_dir = TempDir::new()?;
    let bundle_name = format!(
        "{}-{}.tar.gz",
        manifest.name.replace("/", "__"),
        manifest.version
    )
    .to_string();
    let temp_bundle_path = temp_dir.path().join(&bundle_name);

    // Bundle JS files first and prepare replacements
    let mut bundled_files = HashMap::new();
    for js_file in js_files_to_bundle {
        bundled_files.insert(
            js_file.clone(),
            bundle_js_file(package_path, js_file).await?,
        );
    }

    // Create tar.gz archive
    let tar_gz = fs::File::create(&temp_bundle_path)?;
    let enc = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);

    // Add files to archive
    let mut file_count = 0;
    for entry in WalkDir::new(package_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path() != package_path) // Skip the root directory itself
        .filter(|e| should_include_file(e.path(), package_path))
    {
        if entry.file_type().is_file() {
            let relative_path = entry.path().strip_prefix(package_path)?;
            let relative_path_str = relative_path.to_string_lossy().to_string();

            debug!("Adding file to bundle: {}", relative_path.display());

            if relative_path_str == "codemod.yaml" {
                let rendered_manifest = serde_yaml::to_string(manifest)?;
                let mut header = tar::Header::new_gnu();
                header.set_path(relative_path)?;
                header.set_size(rendered_manifest.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append(&header, rendered_manifest.as_bytes())?;
                info!("Replaced codemod.yaml with effective manifest");
            } else if let Some(bundled_code) = bundled_files.get(&relative_path_str) {
                // Add bundled version instead of original
                let mut header = tar::Header::new_gnu();
                header.set_path(relative_path)?;
                header.set_size(bundled_code.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append(&header, bundled_code.as_bytes())?;
                info!(
                    "Replaced {} with bundled version ({} bytes)",
                    relative_path_str,
                    bundled_code.len()
                );
            } else {
                // Add original file
                tar.append_path_with_name(entry.path(), relative_path)?;
            }
            file_count += 1;
        }
    }

    info!("Added {file_count} files to bundle");

    // Finish the tar archive and flush the gzip encoder
    let enc = tar.into_inner()?;
    enc.finish()?;

    let bundle_size = fs::metadata(&temp_bundle_path)?.len();
    const MAX_BUNDLE_SIZE: u64 = 10 * 1024 * 1024; // 10MB compressed

    if bundle_size > MAX_BUNDLE_SIZE {
        return Err(anyhow!(
            "Compressed bundle too large: {} bytes. Maximum allowed: {} bytes.",
            bundle_size,
            MAX_BUNDLE_SIZE
        ));
    }

    info!("Created bundle: {bundle_name} ({bundle_size} bytes)");

    // Move to a persistent location in the system temp directory
    let system_temp = std::env::temp_dir();
    let output_path = system_temp.join(&bundle_name);

    fs::copy(&temp_bundle_path, &output_path)?;
    Ok(output_path)
}

fn should_include_file(file_path: &Path, package_root: &Path) -> bool {
    let relative_path = match file_path.strip_prefix(package_root) {
        Ok(path) => path,
        Err(_) => {
            debug!("Failed to strip prefix for: {}", file_path.display());
            return false;
        }
    };

    let path_str = relative_path.to_string_lossy();

    // Exclude common development/build artifacts
    const EXCLUDED_PATTERNS: &[&str] = &[
        ".git/",
        ".gitignore",
        "node_modules/",
        "target/",
        ".cargo/",
        "__pycache__/",
        "*.pyc",
        ".venv/",
        ".env",
        ".DS_Store",
        "Thumbs.db",
    ];

    for pattern in EXCLUDED_PATTERNS {
        if pattern.ends_with('/') {
            if path_str.starts_with(pattern) {
                debug!("Excluding directory: {path_str} (matches {pattern})");
                return false;
            }
        } else if pattern.contains('*') {
            // Simple glob matching
            if *pattern == "*.pyc" && path_str.ends_with(".pyc") {
                debug!("Excluding file: {path_str} (matches {pattern})");
                return false;
            }
        } else if path_str == *pattern {
            debug!("Excluding file: {path_str} (matches {pattern})");
            return false;
        }
    }

    debug!("Including file: {path_str}");
    true
}

async fn upload_package(
    registry_url: &str,
    bundle_path: &Path,
    manifest: &CodemodManifest,
    access_token: &str,
) -> Result<PublishResponse> {
    let client = reqwest::Client::new();

    let package_name = if let Some(registry) = &manifest.registry {
        if let Some(scope) = &registry.scope {
            format!("{}/{}", scope, manifest.name)
        } else {
            manifest.name.clone()
        }
    } else {
        manifest.name.clone()
    };

    let url = format!("{registry_url}/api/v1/registry/packages/{package_name}");

    // Read bundle file
    let bundle_data = fs::read(bundle_path)?;
    let manifest_json = serde_json::to_string(manifest)?;

    // Create multipart form
    let form = reqwest::multipart::Form::new()
        .part(
            "packageFile",
            reqwest::multipart::Part::bytes(bundle_data)
                .file_name(format!("{}-{}.tar.gz", manifest.name, manifest.version))
                .mime_str("application/gzip")?,
        )
        .text("manifest", manifest_json);

    debug!("Uploading to: {url}");

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "codemod-cli/1.0")
        .multipart(form)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::CONFLICT {
            return Err(anyhow!(format_publish_conflict_error(
                manifest,
                &error_text
            )));
        } else if status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow!(format_publish_forbidden_error(
                manifest,
                &error_text
            )));
        } else if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!(
                "Authentication failed. Please run 'npx codemod@latest login' again."
            ));
        }

        return Err(anyhow!("Upload failed ({}): {}", status, error_text));
    }

    let publish_response: PublishResponse = response.json().await?;
    Ok(publish_response)
}

fn format_publish_conflict_error(manifest: &CodemodManifest, error_text: &str) -> String {
    let backend_message = extract_backend_error_message(error_text);
    if !backend_message.is_empty() {
        return format!(
            "Access denied. The registry refused this publish because {}",
            ensure_sentence(backend_message)
        );
    }

    format!(
        "Access denied. The registry refused this publish because version {} already exists for package '{}'.",
        manifest.version, manifest.name
    )
}

fn format_publish_forbidden_error(manifest: &CodemodManifest, error_text: &str) -> String {
    let backend_message = extract_backend_error_message(error_text);
    if is_publish_name_taken_error(&backend_message) {
        return format!(
            "Access denied. The registry refused this publish because package name '{}' is already taken. Choose a different package name or publish under a scope you own.",
            manifest.name
        );
    }

    if !backend_message.is_empty() {
        return format!(
            "Access denied. You may not have permission to publish to this package. Backend message: {}",
            backend_message
        );
    }

    format!(
        "Access denied. You may not have permission to publish to this package. If this is a new unscoped package, the package name may already be taken or may require trusted publisher configuration."
    )
}

fn ensure_sentence(message: String) -> String {
    let trimmed = message.trim().trim_end_matches('.');
    format!("{trimmed}.")
}

fn extract_backend_error_message(error_text: &str) -> String {
    let trimmed = error_text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Some(message) = value.get("message").and_then(Value::as_str) {
            return message.trim().to_string();
        }
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return error.trim().to_string();
        }
    }

    trimmed.to_string()
}

fn is_publish_name_taken_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("already taken")
        || normalized.contains("already been taken")
        || normalized.contains("name is taken")
        || normalized.contains("package name already exists")
        || normalized.contains("package already exists")
}

fn calculate_package_size(package_path: &Path) -> Result<u64> {
    let mut total_size = 0;

    for entry in WalkDir::new(package_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| should_include_file(e.path(), package_path))
    {
        total_size += entry.metadata()?.len();
    }

    Ok(total_size)
}

fn is_valid_package_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 50 {
        return false;
    }

    // Pattern: /^(@[a-zA-Z0-9-_.]+\/)?[a-zA-Z0-9-_]+$/
    let re = Regex::new(r"^(@[a-zA-Z0-9\-_.]+/)?[a-zA-Z0-9\-_]+$").unwrap();
    re.is_match(name)
}

fn is_valid_semver(version: &str) -> bool {
    // Basic semver validation (x.y.z format)
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return false;
    }

    parts.iter().all(|part| {
        part.chars().all(|c| c.is_ascii_digit())
            && !part.is_empty()
            && (*part == "0" || !part.starts_with('0'))
    })
}

fn format_package_name(package: &PublishedPackage) -> String {
    if let Some(scope) = &package.scope {
        format!("{}/{}", scope, package.name)
    } else {
        package.name.clone()
    }
}

fn resolve_publish_auth_source(
    storage: &TokenStorage,
    registry_url: &str,
) -> Result<PublishAuthSource> {
    if let Some(token) = normalize_env_auth_token(std::env::var("CODEMOD_AUTH_TOKEN").ok()) {
        debug!("Using auth token from CODEMOD_AUTH_TOKEN environment variable");
        return Ok(PublishAuthSource::EnvironmentToken(token));
    }

    let auth = storage
        .get_auth_for_registry(registry_url)?
        .ok_or_else(|| {
            anyhow!(
                "Not authenticated with registry: {}. Run 'npx codemod@latest login' first, or set CODEMOD_AUTH_TOKEN environment variable.",
                registry_url
            )
        })?;

    Ok(PublishAuthSource::StoredAuth(Box::new(auth)))
}

fn normalize_env_auth_token(token: Option<String>) -> Option<String> {
    token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_authored_skill_bundle(package_path: &Path, package_name: &str) {
        let skill_file = expected_authored_skill_file(package_path, package_name);
        fs::create_dir_all(skill_file.parent().unwrap().join("references")).unwrap();
        fs::write(
            &skill_file,
            r#"---
name: "example"
description: "description"
allowed-tools:
  - Bash(codemod *)
---
codemod-compatibility: skill-package-v1
codemod-skill-version: 0.1.0
"#,
        )
        .unwrap();
        fs::write(
            skill_file.parent().unwrap().join("references/index.md"),
            "- [Usage](./usage.md)\n",
        )
        .unwrap();
        fs::write(
            skill_file.parent().unwrap().join("references/usage.md"),
            "# Usage\n",
        )
        .unwrap();
    }

    fn create_invalid_authored_skill_bundle_missing_marker(
        package_path: &Path,
        package_name: &str,
    ) {
        let skill_file = expected_authored_skill_file(package_path, package_name);
        fs::create_dir_all(skill_file.parent().unwrap().join("references")).unwrap();
        fs::write(
            &skill_file,
            r#"---
name: "example"
description: "description"
allowed-tools:
  - Bash(codemod *)
---
codemod-skill-version: 0.1.0
"#,
        )
        .unwrap();
        fs::write(
            skill_file.parent().unwrap().join("references/index.md"),
            "- [Usage](./usage.md)\n",
        )
        .unwrap();
        fs::write(
            skill_file.parent().unwrap().join("references/usage.md"),
            "# Usage\n",
        )
        .unwrap();
    }

    fn manifest_with(workflow: &str, name: &str) -> CodemodManifest {
        CodemodManifest {
            schema_version: "1".to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: "description".to_string(),
            author: "author".to_string(),
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
    fn resolve_effective_manifest_preserves_explicit_author() {
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");

        let effective = resolve_effective_manifest(manifest).unwrap();

        assert_eq!(effective.author, "author".to_string());
    }

    #[test]
    fn resolve_effective_manifest_trims_required_author() {
        let mut manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");
        manifest.author = "  alice <alice@example.com>  ".to_string();

        let effective = resolve_effective_manifest(manifest).unwrap();

        assert_eq!(effective.author, "alice <alice@example.com>".to_string());
    }

    #[test]
    fn resolve_effective_manifest_errors_when_author_is_blank() {
        let mut manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");
        manifest.author = "   ".to_string();

        let error = resolve_effective_manifest(manifest).unwrap_err();
        assert!(error.to_string().contains("Package author is missing"));
    }

    #[test]
    fn environment_auth_token_is_trimmed_and_blank_values_are_ignored() {
        assert_eq!(
            normalize_env_auth_token(Some("  token-value\n".to_string())),
            Some("token-value".to_string())
        );
        assert_eq!(normalize_env_auth_token(Some("  \n".to_string())), None);
        assert_eq!(normalize_env_auth_token(None), None);
    }

    #[test]
    fn forbidden_publish_error_surfaces_taken_name() {
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "my-codemod");

        let message = format_publish_forbidden_error(
            &manifest,
            r#"{"error":"Forbidden","message":"Package name is already taken"}"#,
        );

        assert!(message.starts_with("Access denied."));
        assert!(message.contains("package name 'my-codemod' is already taken"));
    }

    #[test]
    fn conflict_publish_error_uses_access_denied_framing() {
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "my-codemod");

        let message = format_publish_conflict_error(
            &manifest,
            r#"{"error":"Conflict","message":"Version 0.1.0 already exists"}"#,
        );

        assert!(message.starts_with("Access denied."));
        assert!(message.contains("Version 0.1.0 already exists."));
    }

    #[test]
    fn forbidden_publish_error_includes_backend_message_when_available() {
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");

        let message = format_publish_forbidden_error(
            &manifest,
            r#"{"error":"Forbidden","message":"Only organization owners may publish here"}"#,
        );

        assert!(message.contains("Backend message: Only organization owners may publish here"));
    }

    #[test]
    fn extract_backend_error_message_reads_json_message() {
        assert_eq!(
            extract_backend_error_message(
                r#"{"error":"Forbidden","message":"Package name is already taken"}"#
            ),
            "Package name is already taken"
        );
        assert_eq!(
            extract_backend_error_message("plain text error"),
            "plain text error"
        );
        assert_eq!(extract_backend_error_message("   "), "");
    }

    #[test]
    fn skill_only_package_validates_with_install_skill_workflow() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");
        create_authored_skill_bundle(temp_dir.path(), &manifest.name);
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
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

        let validation = validate_package_structure(temp_dir.path(), &manifest);

        assert!(validation.is_ok());
        assert!(validation.unwrap().is_empty());
    }

    #[test]
    fn install_skill_workflow_requires_authored_skill_file() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
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

        let error = validate_package_structure(temp_dir.path(), &manifest).unwrap_err();
        assert!(error.to_string().contains("install-skill"));
    }

    #[test]
    fn workflow_file_is_required() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("workflow.yaml", "example");

        let error = validate_package_structure(temp_dir.path(), &manifest).unwrap_err();
        assert!(error.to_string().contains("Workflow file is missing"));
    }

    #[test]
    fn workflow_package_validates_when_workflow_exists() {
        let temp_dir = tempdir().unwrap();
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
            r#"
version: "1"
nodes:
  - id: setup
    name: Setup
    type: automatic
    steps:
      - id: init
        name: Initialize
        run: echo hello
"#,
        )
        .unwrap();
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");

        let validation = validate_package_structure(temp_dir.path(), &manifest);

        assert!(validation.is_ok());
    }

    #[test]
    fn package_without_executable_or_install_skill_behavior_is_rejected() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
            r#"
version: "1"
nodes: []
"#,
        )
        .unwrap();

        let error = validate_package_structure(temp_dir.path(), &manifest).unwrap_err();
        assert!(error.to_string().contains("Invalid package structure"));
    }

    #[test]
    fn expected_workflow_path_uses_manifest_value_when_set() {
        let manifest = manifest_with("custom-workflow.yaml", "example");
        let path = expected_workflow_path(Path::new("/tmp/test"), &manifest);
        assert_eq!(path, Path::new("/tmp/test").join("custom-workflow.yaml"));
    }

    #[test]
    fn detect_behavior_shape_identifies_workflow_and_skill_packages() {
        let temp_dir = tempdir().unwrap();
        create_authored_skill_bundle(temp_dir.path(), "example");
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
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
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");

        let shape = detect_package_behavior_shape(temp_dir.path(), &manifest);
        assert_eq!(shape, PackageBehaviorShape::WorkflowAndSkill);
    }

    #[test]
    fn invalid_package_name_fails_validation() {
        let temp_dir = tempdir().unwrap();
        create_authored_skill_bundle(temp_dir.path(), "Invalid Name");
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "Invalid Name");
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
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
          package: "@codemod/invalid-name"
"#,
        )
        .unwrap();

        let error = validate_package_structure(temp_dir.path(), &manifest).unwrap_err();
        assert!(error.to_string().contains("Invalid package name"));
    }

    #[test]
    fn skill_publish_fails_when_skill_markers_are_missing() {
        let temp_dir = tempdir().unwrap();
        create_invalid_authored_skill_bundle_missing_marker(temp_dir.path(), "example");
        let manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
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

        let error = validate_package_structure(temp_dir.path(), &manifest).unwrap_err();
        assert!(error.to_string().contains("missing compatibility marker"));
    }

    #[test]
    fn bundled_package_contains_effective_manifest() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let temp_dir = tempdir().unwrap();
        let package_path = temp_dir.path();
        let mut manifest = manifest_with(DEFAULT_WORKFLOW_FILE_NAME, "example");
        manifest.author = "alice <alice@example.com>".to_string();

        fs::write(
            package_path.join("codemod.yaml"),
            r#"schema_version: "1.0"
name: "example"
version: "1.0.0"
description: "description"
workflow: "workflow.yaml"
"#,
        )
        .unwrap();
        fs::write(
            package_path.join(DEFAULT_WORKFLOW_FILE_NAME),
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

        let bundle_path = runtime
            .block_on(create_package_bundle(package_path, &manifest, &[]))
            .unwrap();

        let archive_file = fs::File::open(&bundle_path).unwrap();
        let decoder = flate2::read::GzDecoder::new(archive_file);
        let mut archive = tar::Archive::new(decoder);
        let mut bundled_manifest = None;

        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap() == Path::new("codemod.yaml") {
                let mut content = String::new();
                use std::io::Read;
                entry.read_to_string(&mut content).unwrap();
                bundled_manifest = Some(content);
                break;
            }
        }

        let bundled_manifest = bundled_manifest.expect("bundled codemod.yaml should exist");
        assert!(bundled_manifest.contains("author: alice <alice@example.com>"));

        fs::remove_file(bundle_path).unwrap();
    }
}
