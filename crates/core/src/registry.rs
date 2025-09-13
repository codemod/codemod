use bytes::BytesMut;
use log::{debug, info};
use reqwest;
use reqwest::header::CONTENT_LENGTH;
use serde::Deserialize;
use serde_json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use thiserror::Error;
use walkdir::WalkDir;

use crate::utils::get_cache_dir;

pub type ProgressBarCallback = Arc<Box<dyn Fn(u64, u64) + Send + Sync>>;

#[derive(Error, Debug)]
pub enum RegistryError {
    #[error("Package is legacy: {package}")]
    LegacyPackage { package: String },

    #[error("Local package path does not exist: {path}")]
    LocalPackageNotFound { path: String },

    #[error("Local package path is not a directory: {path}")]
    LocalPackageNotDirectory { path: String },

    #[error("Package not found: {package}")]
    PackageNotFound { package: String },

    #[error("Access denied to package: {package}. You may need to login.")]
    AccessDenied { package: String },

    #[error("Failed to fetch package info ({status}): {message}")]
    FetchPackageInfoFailed { status: u16, message: String },

    #[error("Invalid scoped package name: {name}")]
    InvalidScopedPackageName { name: String },

    #[error("Version {version} not found for package {package}")]
    VersionNotFound { version: String, package: String },

    #[error("No version specified and no latest version available for package {package}")]
    NoVersionAvailable { package: String },

    #[error("Failed to download package ({status}): {message}")]
    DownloadFailed { status: u16, message: String },

    #[error("Failed to download package from CDN ({status}): {message}")]
    CdnDownloadFailed { status: u16, message: String },

    #[error("Downloaded data is not a valid gzip file. Expected magic bytes 1f 8b, got {magic}")]
    InvalidGzipFile { magic: String },

    #[error("CDN file is not a valid gzip file. Expected magic bytes 1f 8b, got {magic}")]
    InvalidCdnGzipFile { magic: String },

    #[error("Downloaded data is not a valid gzip file and not a JSON redirect. Expected gzip magic bytes 1f 8b, got {magic}")]
    InvalidDownloadData { magic: String },

    #[error("Invalid package: missing {file} in {path}")]
    MissingPackageFile { file: String, path: String },

    #[error("HTTP request failed")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parsing failed")]
    JsonError(#[from] serde_json::Error),

    #[error("File I/O error")]
    IoError(#[from] std::io::Error),

    #[error("Walkdir error")]
    WalkdirError(#[from] walkdir::Error),

    #[error("Auth provider error")]
    AuthProviderError(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, RegistryError>;

#[derive(Debug, Clone)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegistryAuth {
    pub tokens: AuthTokens,
}

#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub default_registry: String,
    pub cache_dir: PathBuf,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        let registry_url =
            std::env::var("CODEMOD_REGISTRY_URL").unwrap_or("https://app.codemod.com".to_string());

        Self {
            default_registry: registry_url,
            cache_dir: get_cache_dir().unwrap(),
        }
    }
}

#[derive(Deserialize, Debug)]
struct PackageInfo {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    scope: Option<String>,
    is_legacy: bool,
    latest_version: Option<String>,
    versions: HashMap<String, PackageVersion>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct PackageVersion {
    version: String,
    description: Option<String>,
    checksum: String,
    size: u32,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct DownloadResponse {
    download_url: String,
    expires_at: String,
}

#[derive(Debug, Clone)]
pub struct PackageSpec {
    pub scope: Option<String>,
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub spec: PackageSpec,
    pub version: String,
    pub package_dir: PathBuf,
}

#[derive(Clone)]
pub struct RegistryClient {
    pub config: RegistryConfig,
    pub auth_provider: Option<Arc<dyn AuthProvider>>,
    client: reqwest::Client,
}

pub trait AuthProvider: Send + Sync {
    fn get_auth_for_registry(&self, registry_url: &str) -> Result<Option<RegistryAuth>>;
}

impl RegistryClient {
    pub fn new(config: RegistryConfig, auth_provider: Option<Arc<dyn AuthProvider>>) -> Self {
        Self {
            config,
            auth_provider,
            client: reqwest::Client::new(),
        }
    }

    pub async fn resolve_package(
        &self,
        source: &str,
        registry_url: Option<&str>,
        force_download: bool,
        progress_bar: Option<ProgressBarCallback>,
    ) -> Result<ResolvedPackage> {
        // Check if it's a local path
        if source.starts_with("./") || source.starts_with("../") || source.starts_with("/") {
            return self.resolve_local_package(source);
        }

        // It's a registry package
        let registry = registry_url.unwrap_or(&self.config.default_registry);
        let package_spec = parse_package_spec(source)?;

        info!(
            "Resolving package: {} from registry: {}",
            format_package_spec(&package_spec),
            registry
        );

        // Get package information
        let package_info = self.get_package_info(registry, &package_spec).await?;

        if package_info.is_legacy {
            return Err(RegistryError::LegacyPackage {
                package: format_package_spec(&package_spec),
            });
        }

        // Determine version to use
        let version = determine_version(&package_spec, &package_info)?;

        // Get or create cache directory
        let package_cache_dir = self.get_package_cache_dir(&package_spec, &version)?;

        println!("package_cache_dir: {}", package_cache_dir.display());

        // Check if package is cached and valid
        let package_dir = if force_download || !is_package_cached(&package_cache_dir)? {
            info!("Downloading package: {source}@{version}");
            self.download_and_extract_package(
                registry,
                &package_spec,
                &version,
                &package_cache_dir,
                progress_bar,
            )
            .await?
        } else {
            debug!("Using cached package: {}", package_cache_dir.display());
            package_cache_dir
        };

        // Validate package structure
        validate_package_structure(&package_dir)?;

        Ok(ResolvedPackage {
            spec: package_spec,
            version,
            package_dir,
        })
    }

    fn resolve_local_package(&self, source: &str) -> Result<ResolvedPackage> {
        let path = PathBuf::from(source);

        if !path.exists() {
            return Err(RegistryError::LocalPackageNotFound {
                path: source.to_string(),
            });
        }

        if !path.is_dir() {
            return Err(RegistryError::LocalPackageNotDirectory {
                path: source.to_string(),
            });
        }

        // Validate package structure
        validate_package_structure(&path)?;

        // Extract name from path for spec
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("local-package")
            .to_string();

        Ok(ResolvedPackage {
            spec: PackageSpec {
                scope: None,
                name,
                version: Some("local".to_string()),
            },
            version: "local".to_string(),
            package_dir: path,
        })
    }

    async fn get_package_info(
        &self,
        registry_url: &str,
        spec: &PackageSpec,
    ) -> Result<PackageInfo> {
        let package_path = if let Some(scope) = &spec.scope {
            format!("{}/{}", scope, spec.name)
        } else {
            spec.name.clone()
        };

        let url = format!("{registry_url}/api/v1/registry/packages/{package_path}");
        debug!("Fetching package info from: {url}");

        let mut request = self.client.get(&url);

        // Add authentication header if available
        if let Some(auth_provider) = &self.auth_provider {
            if let Ok(Some(auth)) = auth_provider.get_auth_for_registry(registry_url) {
                request = request.header(
                    "Authorization",
                    format!("Bearer {}", auth.tokens.access_token),
                );
            }
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(RegistryError::PackageNotFound {
                    package: format_package_spec(spec),
                });
            } else if status == reqwest::StatusCode::FORBIDDEN {
                return Err(RegistryError::AccessDenied {
                    package: format_package_spec(spec),
                });
            }

            let error_text = response.text().await.unwrap_or_default();
            return Err(RegistryError::FetchPackageInfoFailed {
                status: status.into(),
                message: error_text,
            });
        }

        let package_info: PackageInfo = response.json().await?;
        Ok(package_info)
    }

    fn get_package_cache_dir(&self, spec: &PackageSpec, version: &str) -> Result<PathBuf> {
        let package_dir = if let Some(scope) = &spec.scope {
            self.config
                .cache_dir
                .join(scope)
                .join(&spec.name)
                .join(version)
        } else {
            self.config
                .cache_dir
                .join("global")
                .join(&spec.name)
                .join(version)
        };

        fs::create_dir_all(&package_dir)?;
        Ok(package_dir)
    }

    async fn download_and_extract_package(
        &self,
        registry_url: &str,
        spec: &PackageSpec,
        version: &str,
        cache_dir: &Path,
        progress_bar: Option<ProgressBarCallback>,
    ) -> Result<PathBuf> {
        let package_path = if let Some(scope) = &spec.scope {
            format!("{}/{}", scope, spec.name)
        } else {
            spec.name.clone()
        };

        let download_url =
            format!("{registry_url}/api/v1/registry/packages/{package_path}/download/{version}");

        debug!("Downloading package from: {download_url}");

        // Get auth token if available
        let auth_token = if let Some(auth_provider) = &self.auth_provider {
            auth_provider
                .get_auth_for_registry(registry_url)
                .ok()
                .flatten()
                .map(|auth| auth.tokens.access_token)
        } else {
            None
        };

        // Download the initial response (might be gzip data or JSON redirect)
        let package_data = self
            .download_from_url(&download_url, auth_token.as_deref(), progress_bar.clone())
            .await?;

        // Check if this is a JSON redirect response
        if package_data.len() >= 2 {
            let magic = &package_data[0..2];
            if magic != [0x1f, 0x8b] {
                // Check if this is a JSON response with a download_url
                if let Ok(text) = std::str::from_utf8(&package_data) {
                    if text.trim().starts_with('{') {
                        // Try to parse as JSON to get the actual download URL
                        if let Ok(download_response) =
                            serde_json::from_str::<DownloadResponse>(text)
                        {
                            debug!(
                                "Server returned download URL: {}",
                                download_response.download_url
                            );

                            // Download from the actual CDN URL
                            let actual_package_data = self
                                .download_from_url(
                                    &download_response.download_url,
                                    auth_token.as_deref(),
                                    progress_bar,
                                )
                                .await?;

                            // Verify this is actually a gzip file
                            if actual_package_data.len() >= 2 {
                                let actual_magic = &actual_package_data[0..2];
                                if actual_magic != [0x1f, 0x8b] {
                                    return Err(RegistryError::InvalidCdnGzipFile {
                                        magic: format!(
                                            "{:02x} {:02x}",
                                            actual_magic[0], actual_magic[1]
                                        ),
                                    });
                                }
                            }

                            self.extract_package(&actual_package_data, cache_dir)
                                .await?;
                            info!("Package cached to: {}", cache_dir.display());
                            return Ok(cache_dir.to_path_buf());
                        }
                    }
                }

                // If we get here, it's not a valid gzip file and not a JSON redirect
                return Err(RegistryError::InvalidDownloadData {
                    magic: format!("{:02x} {:02x}", magic[0], magic[1]),
                });
            }
        }

        // If we get here, it's a direct gzip file
        self.extract_package(&package_data, cache_dir).await?;
        info!("Package cached to: {}", cache_dir.display());
        Ok(cache_dir.to_path_buf())
    }

    async fn download_from_url(
        &self,
        url: &str,
        auth_token: Option<&str>,
        progress_bar: Option<ProgressBarCallback>,
    ) -> Result<BytesMut> {
        // Get content length for progress tracking
        let mut head_request = self.client.head(url);
        if let Some(token) = auth_token {
            head_request = head_request.header("Authorization", format!("Bearer {token}"));
        }

        let head_response =
            head_request
                .send()
                .await
                .map_err(|e| RegistryError::DownloadFailed {
                    status: e
                        .status()
                        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
                        .into(),
                    message: e.to_string(),
                })?;

        let total_size = head_response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|val| val.to_str().ok()?.parse().ok())
            .unwrap_or(0);

        // Download the actual data
        let mut get_request = self.client.get(url);
        if let Some(token) = auth_token {
            get_request = get_request.header("Authorization", format!("Bearer {token}"));
        }

        let response = get_request
            .send()
            .await
            .map_err(|e| RegistryError::DownloadFailed {
                status: e
                    .status()
                    .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
                    .into(),
                message: e.to_string(),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(RegistryError::DownloadFailed {
                status: status.into(),
                message: error_text,
            });
        }

        let mut package_data = BytesMut::new();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| RegistryError::DownloadFailed {
                status: e
                    .status()
                    .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
                    .into(),
                message: e.to_string(),
            })?;
        package_data.extend_from_slice(&bytes);
        let downloaded = bytes.len() as u64;

        if let Some(callback) = &progress_bar {
            callback(downloaded, total_size);
        }

        Ok(package_data)
    }

    async fn extract_package(&self, package_data: &[u8], cache_dir: &Path) -> Result<()> {
        // Extract to temporary directory first
        let temp_dir = TempDir::new()?;
        let temp_path = temp_dir.path();

        // Extract tar.gz
        let tar_gz = flate2::read::GzDecoder::new(package_data);
        let mut archive = tar::Archive::new(tar_gz);
        info!("Extracting to: {}", temp_path.display());
        archive.unpack(temp_path)?;

        // Move to cache directory
        if cache_dir.exists() {
            fs::remove_dir_all(cache_dir)?;
        }
        fs::create_dir_all(cache_dir.parent().unwrap())?;

        // Copy to cache directory
        copy_dir_recursively(temp_path, cache_dir)?;
        Ok(())
    }
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new(RegistryConfig::default(), None)
    }
}

pub fn parse_package_spec(package: &str) -> Result<PackageSpec> {
    // Scoped packages are prefixed with @
    let (scope, rest) = if package.starts_with('@') {
        let parts: Vec<&str> = package.splitn(2, '/').collect();
        if parts.len() == 2 {
            (Some(parts[0].to_string()), parts[1])
        } else {
            return Err(RegistryError::InvalidScopedPackageName {
                name: package.to_string(),
            });
        }
    } else {
        (None, package)
    };

    // Get version
    let (name, version) = if rest.contains('@') {
        let parts: Vec<&str> = rest.rsplitn(2, '@').collect();
        if parts.len() == 2 {
            (parts[1].to_string(), Some(parts[0].to_string()))
        } else {
            (rest.to_string(), None)
        }
    } else {
        (rest.to_string(), None)
    };

    Ok(PackageSpec {
        scope,
        name,
        version,
    })
}

pub fn format_package_spec(spec: &PackageSpec) -> String {
    let name = if let Some(scope) = &spec.scope {
        format!("{}/{}", scope, spec.name)
    } else {
        spec.name.clone()
    };

    if let Some(version) = &spec.version {
        format!("{name}@{version}")
    } else {
        name
    }
}

fn determine_version(spec: &PackageSpec, package_info: &PackageInfo) -> Result<String> {
    if let Some(version) = &spec.version {
        if package_info.versions.contains_key(version) {
            Ok(version.clone())
        } else {
            Err(RegistryError::VersionNotFound {
                version: version.clone(),
                package: format_package_spec(spec),
            })
        }
    } else if let Some(latest) = &package_info.latest_version {
        Ok(latest.clone())
    } else {
        Err(RegistryError::NoVersionAvailable {
            package: format_package_spec(spec),
        })
    }
}

fn is_package_cached(package_dir: &Path) -> Result<bool> {
    if !package_dir.exists() {
        return Ok(false);
    }

    // Check for required files
    let codemod_yaml = package_dir.join("codemod.yaml");
    let workflow_yaml = package_dir.join("workflow.yaml");

    Ok(codemod_yaml.exists() && workflow_yaml.exists())
}

fn validate_package_structure(package_dir: &Path) -> Result<()> {
    let codemod_yaml = package_dir.join("codemod.yaml");
    let workflow_yaml = package_dir.join("workflow.yaml");

    if !codemod_yaml.exists() {
        return Err(RegistryError::MissingPackageFile {
            file: "codemod.yaml".to_string(),
            path: package_dir.display().to_string(),
        });
    }

    if !workflow_yaml.exists() {
        return Err(RegistryError::MissingPackageFile {
            file: "workflow.yaml".to_string(),
            path: package_dir.display().to_string(),
        });
    }

    debug!("Package structure validated");
    Ok(())
}

fn copy_dir_recursively(src: &Path, dst: &Path) -> Result<()> {
    info!(
        "Copying directory recursively from: {} to: {}",
        src.display(),
        dst.display()
    );
    for entry in WalkDir::new(src) {
        let entry = entry?;
        let path = entry.path();
        let relative_path = path.strip_prefix(src).unwrap();
        let dst_path = dst.join(relative_path);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dst_path)?;
        } else {
            fs::copy(path, &dst_path)?;
        }
    }
    Ok(())
}
