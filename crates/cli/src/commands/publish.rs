use crate::utils::manifest::{CodemodManifest, RegistryConfig};
use crate::utils::rolldown_bundler::{RolldownBundler, RolldownBundlerConfig};
use anyhow::{anyhow, Result};
use butterflow_core::utils::validate_workflow;
use butterflow_core::Workflow;
use butterflow_models::step::StepAction;
use clap::Args;
use log::{debug, info, warn};
use regex::Regex;
use reqwest;
use serde::Deserialize;
use serde_yaml;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use walkdir::WalkDir;

use crate::auth::TokenStorage;
use crate::{TelemetrySenderMutex, CLI_VERSION};
use codemod_telemetry::send_event::BaseEvent;

#[derive(Args, Debug)]
pub struct Command {
    /// Path to codemod directory
    path: Option<PathBuf>,
    /// Explicit version override
    #[arg(long)]
    version: Option<String>,
    /// Target registry URL
    #[arg(long)]
    registry: Option<String>,
    /// Tag for the release
    #[arg(long)]
    tag: Option<String>,
    /// Access level (public, private)
    #[arg(long)]
    access: Option<String>,
    /// Validate and pack without uploading
    #[arg(long)]
    dry_run: bool,
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

pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    let package_path = args
        .path
        .as_ref()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve package path: {}", e))?;

    info!("Publishing codemod from: {}", package_path.display());

    // Load and validate manifest
    let mut manifest = load_manifest(&package_path)?;

    // Override version if specified
    if let Some(version) = &args.version {
        manifest.version = version.clone();
    }

    // Override access if specified
    if let Some(access) = &args.access {
        if manifest.registry.is_none() {
            manifest.registry = Some(RegistryConfig {
                access: Some(access.clone()),
                scope: None,
                visibility: None,
            });
        } else if let Some(ref mut registry) = manifest.registry {
            registry.access = Some(access.clone());
        }
    }

    // Validate package structure and get JS files to bundle
    let js_files_to_bundle = validate_package_structure(&package_path, &manifest)?;

    // Create package bundle with bundled JS files
    let bundle_path =
        create_package_bundle(&package_path, &manifest, &js_files_to_bundle, args.dry_run).await?;

    println!("bundle_path: {}", bundle_path.display());

    // Get registry configuration
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    let registry_url = args
        .registry
        .as_ref()
        .unwrap_or(&config.default_registry)
        .clone();

    // Check authentication
    let auth = storage
        .get_auth_for_registry(&registry_url)?
        .ok_or_else(|| {
            anyhow!(
                "Not authenticated with registry: {}. Run 'npx codemod@latest login' first.",
                registry_url
            )
        })?;

    // Upload package
    let response = upload_package(
        &registry_url,
        &bundle_path,
        &manifest,
        &auth.tokens.access_token,
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
                    ("codemodName".to_string(), manifest.name.clone()),
                    ("version".to_string(), manifest.version.clone()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;

    println!("✅ Package published successfully!");
    println!("📦 {}", format_package_name(&response.package));
    println!("🏷️  Version: {}", response.package.version);
    println!("📅 Published: {}", response.package.published_at);

    // Clean up temporary bundle
    if let Err(e) = fs::remove_file(&bundle_path) {
        warn!("Failed to clean up temporary bundle: {e}");
    }

    Ok(())
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
    let manifest: CodemodManifest = serde_yaml::from_str(&manifest_content)
        .map_err(|e| anyhow!("Failed to parse codemod.yaml: {}", e))?;

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
    // Check required files
    let workflow_path = package_path.join(&manifest.workflow);
    if !workflow_path.exists() {
        return Err(anyhow!(
            "Workflow file not found: {}",
            workflow_path.display()
        ));
    }

    // Validate workflow file
    let workflow_content = fs::read_to_string(&workflow_path)?;
    let workflow: Workflow = serde_yaml::from_str(&workflow_content)
        .map_err(|e| anyhow!("Invalid workflow YAML: {}", e))?;

    let validation_result = validate_workflow(&workflow, package_path);
    if let Err(e) = validation_result {
        return Err(anyhow!("Invalid workflow: {}", e));
    }

    // Find all JS AST grep steps that need bundling
    let js_files = find_js_files_in_workflow(&workflow, package_path)?;

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

    info!("Package validation successful");
    Ok(js_files)
}

async fn create_package_bundle(
    package_path: &Path,
    manifest: &CodemodManifest,
    js_files_to_bundle: &[String],
    dry_run: bool,
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

            // Check if this is a JS file that should be replaced with bundled version
            if let Some(bundled_code) = bundled_files.get(&relative_path_str) {
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

    // Move to a persistent location (both dry-run and regular publishing)
    let output_path = if dry_run {
        std::env::current_dir()?.join(&bundle_name)
    } else {
        // Create a temporary file in the system temp directory that won't be auto-cleaned
        let system_temp = std::env::temp_dir();
        system_temp.join(&bundle_name)
    };

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
            return Err(anyhow!("Version {} already exists.", manifest.version));
        } else if status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow!(
                "Access denied. You may not have permission to publish to this package."
            ));
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
