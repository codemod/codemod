use bytes::BytesMut;
use log::{debug, info};
use reqwest;
use reqwest::header::CONTENT_LENGTH;
use serde::Deserialize;
use serde_json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
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

    #[error(
        "Downloaded data is not a valid gzip file and not a JSON redirect. Expected gzip magic bytes 1f 8b, got {magic}"
    )]
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
    #[serde(default)]
    dist_tags: HashMap<String, String>,
    versions: HashMap<String, PackageVersion>,
    #[serde(default)]
    access: Option<String>,
}

const PACKAGE_INFO_CACHE_CAPACITY: usize = 256;
type PackageInfoCacheCell = Arc<tokio::sync::OnceCell<Arc<PackageInfo>>>;
type PackageInfoCache = Arc<tokio::sync::Mutex<HashMap<String, PackageInfoCacheCell>>>;

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
    #[serde(default)]
    dry_run_only: bool,
}

#[derive(Debug, Clone)]
pub struct PackageSpec {
    pub scope: Option<String>,
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegistryPackageMetadata {
    pub registry_base_url: String,
    pub package_web_path: String,
    pub access: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub spec: PackageSpec,
    pub version: String,
    pub package_dir: PathBuf,
    pub dry_run_only: bool,
    pub registry_metadata: Option<RegistryPackageMetadata>,
}

#[derive(Clone)]
pub struct RegistryClient {
    pub config: RegistryConfig,
    pub auth_provider: Option<Arc<dyn AuthProvider>>,
    client: reqwest::Client,
    package_locks: Arc<tokio::sync::Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>>,
    package_info_cache: PackageInfoCache,
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
            package_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            package_info_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    async fn package_lock(&self, package_cache_dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.package_locks.lock().await;
        // Completed resolutions leave only weak entries behind. Prune them
        // opportunistically so a long-lived client does not retain every
        // package path it has ever seen.
        locks.retain(|_, lock| lock.strong_count() > 0);

        if let Some(lock) = locks.get(package_cache_dir).and_then(Weak::upgrade) {
            return lock;
        }

        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(package_cache_dir.to_path_buf(), Arc::downgrade(&lock));
        lock
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
        let package_info = self
            .get_package_info(registry, &package_spec, !force_download)
            .await?;

        if package_info.is_legacy {
            return Err(RegistryError::LegacyPackage {
                package: format_package_spec(&package_spec),
            });
        }

        let resolved_package_spec = PackageSpec {
            name: package_info.name.clone(),
            scope: package_info.scope.as_deref().map(normalize_scope),
            version: None,
        };

        // Determine version to use
        let version = determine_version(&package_spec, &package_info)?;

        // Get or create cache directory
        let package_cache_dir = self.get_package_cache_dir(&package_spec, &version)?;
        // Parallel bundle resolution can discover the same transitive package
        // through multiple branches. Serialize writes to that package's cache
        // directory so concurrent downloads cannot remove or partially copy
        // each other's extracted files.
        let package_lock = self.package_lock(&package_cache_dir).await;
        let _package_guard = package_lock.lock().await;

        // Check if package is cached and valid.
        let is_pro_package = package_info.access.as_deref() == Some("pro");
        let should_download = force_download
            || !is_package_cached(&package_cache_dir)?
            || is_pro_package
            || package_cache_dir.join(".dry_run_only").exists();

        let (package_dir, dry_run_only) = if should_download {
            info!("Downloading package: {source}@{version}");
            let (dir, dry_run_only) = self
                .download_and_extract_package(
                    registry,
                    &resolved_package_spec,
                    &version,
                    &package_cache_dir,
                    progress_bar,
                )
                .await?;

            if dry_run_only {
                let _ = std::fs::write(dir.join(".dry_run_only"), "");
            }

            (dir, dry_run_only)
        } else {
            debug!("Using cached package: {}", package_cache_dir.display());
            (package_cache_dir, false)
        };

        // Validate package structure
        validate_package_structure(&package_dir)?;

        let package_web_path = registry_package_web_path(&resolved_package_spec);

        Ok(ResolvedPackage {
            spec: package_spec,
            version,
            package_dir,
            dry_run_only,
            registry_metadata: Some(RegistryPackageMetadata {
                registry_base_url: registry.to_string(),
                package_web_path,
                access: package_info.access.clone(),
            }),
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
            dry_run_only: false,
            registry_metadata: None,
        })
    }

    async fn get_package_info(
        &self,
        registry_url: &str,
        spec: &PackageSpec,
        use_cache: bool,
    ) -> Result<Arc<PackageInfo>> {
        let exact_version = spec
            .version
            .as_deref()
            .filter(|version| semver::Version::parse(version).is_ok());

        if !use_cache || exact_version.is_none() {
            return self
                .fetch_package_info(registry_url, spec)
                .await
                .map(Arc::new);
        }

        let cache_key = format!(
            "{}|{}",
            registry_url.trim_end_matches('/'),
            format_package_spec(spec)
        );
        let cache_cell = self.package_info_cache_cell(cache_key).await;
        let package_info = cache_cell
            .get_or_try_init(|| async {
                self.fetch_package_info(registry_url, spec)
                    .await
                    .map(Arc::new)
            })
            .await?;

        Ok(Arc::clone(package_info))
    }

    async fn package_info_cache_cell(&self, cache_key: String) -> PackageInfoCacheCell {
        let mut cache = self.package_info_cache.lock().await;
        if let Some(cell) = cache.get(&cache_key) {
            return Arc::clone(cell);
        }

        if cache.len() >= PACKAGE_INFO_CACHE_CAPACITY {
            let completed_key = cache.iter().find_map(|(key, cell)| {
                if cell.get().is_some() {
                    Some(key.clone())
                } else {
                    None
                }
            });
            if let Some(completed_key) = completed_key {
                cache.remove(&completed_key);
            }
        }

        let cell = Arc::new(tokio::sync::OnceCell::new());
        // If every bounded entry is currently being fetched, leave this cell
        // uncached instead of evicting an in-flight request.
        if cache.len() < PACKAGE_INFO_CACHE_CAPACITY {
            cache.insert(cache_key, Arc::clone(&cell));
        }
        cell
    }

    async fn fetch_package_info(
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

        let mut request = self
            .client
            .get(&url)
            .header("x-supports-dry-run-only", "true");

        // Add authentication header if available
        if let Some(auth_provider) = &self.auth_provider
            && let Ok(Some(auth)) = auth_provider.get_auth_for_registry(registry_url)
        {
            request = request.header(
                "Authorization",
                format!("Bearer {}", auth.tokens.access_token),
            );
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
    ) -> Result<(PathBuf, bool)> {
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

        // Signal to the server that this CLI version can enforce dry-run-only mode
        let dry_run_headers: &[(&str, &str)] = &[("x-supports-dry-run-only", "true")];

        // Download the initial response (might be gzip data or JSON redirect)
        let package_data = self
            .download_from_url(
                &download_url,
                auth_token.as_deref(),
                Some(dry_run_headers),
                progress_bar.clone(),
            )
            .await?;

        // Check if this is a JSON redirect response
        if package_data.len() >= 2 {
            let magic = &package_data[0..2];
            if magic != [0x1f, 0x8b] {
                // Check if this is a JSON response with a download_url
                if let Ok(text) = std::str::from_utf8(&package_data)
                    && text.trim().starts_with('{')
                {
                    // Try to parse as JSON to get the actual download URL
                    if let Ok(download_response) = serde_json::from_str::<DownloadResponse>(text) {
                        debug!(
                            "Server returned dry_run_only: {}",
                            download_response.dry_run_only
                        );
                        debug!(
                            "Server returned download URL: {}",
                            download_response.download_url
                        );

                        let dry_run_only = download_response.dry_run_only;

                        // Download from the actual CDN URL
                        let actual_package_data = self
                            .download_from_url(
                                &download_response.download_url,
                                auth_token.as_deref(),
                                None,
                                progress_bar,
                            )
                            .await?;

                        // Check if this is a gzip file or uncompressed tar
                        if actual_package_data.len() >= 2 {
                            let actual_magic = &actual_package_data[0..2];
                            if actual_magic == [0x1f, 0x8b] {
                                // It's a gzip file, extract as usual
                                self.extract_package(&actual_package_data, cache_dir)
                                    .await
                                    .map_err(|e| RegistryError::InvalidCdnGzipFile {
                                        magic: e.to_string(),
                                    })?;
                            } else {
                                // It might be an uncompressed tar file, try to extract it directly
                                self.extract_uncompressed_tar(&actual_package_data, cache_dir)
                                    .await
                                    .map_err(|e| RegistryError::InvalidCdnGzipFile {
                                        magic: e.to_string(),
                                    })?;
                            }
                        } else {
                            return Err(RegistryError::InvalidCdnGzipFile {
                                magic: "empty file".to_string(),
                            });
                        }
                        info!("Package cached to: {}", cache_dir.display());
                        return Ok((cache_dir.to_path_buf(), dry_run_only));
                    }
                }

                // If we get here, it's not a valid gzip file and not a JSON redirect
                return Err(RegistryError::InvalidDownloadData {
                    magic: format!("{:02x} {:02x}", magic[0], magic[1]),
                });
            }
        }

        // If we get here, it's a direct gzip file (public package)
        self.extract_package(&package_data, cache_dir).await?;
        info!("Package cached to: {}", cache_dir.display());
        Ok((cache_dir.to_path_buf(), false))
    }

    async fn download_from_url(
        &self,
        url: &str,
        auth_token: Option<&str>,
        extra_headers: Option<&[(&str, &str)]>,
        progress_bar: Option<ProgressBarCallback>,
    ) -> Result<BytesMut> {
        // Get content length for progress tracking
        let mut head_request = self.client.head(url);
        if let Some(token) = auth_token {
            head_request = head_request.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(headers) = extra_headers {
            for (key, value) in headers {
                head_request = head_request.header(*key, *value);
            }
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
        if let Some(headers) = extra_headers {
            for (key, value) in headers {
                get_request = get_request.header(*key, *value);
            }
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
        let temp_dir = TempDir::new()?;
        let temp_path = temp_dir.path();

        let tar_gz = flate2::read::GzDecoder::new(package_data);
        let mut archive = tar::Archive::new(tar_gz);
        info!("Extracting to: {}", temp_path.display());
        archive.unpack(temp_path)?;

        if cache_dir.exists() {
            fs::remove_dir_all(cache_dir)?;
        }
        fs::create_dir_all(cache_dir.parent().unwrap())?;

        copy_dir_recursively(temp_path, cache_dir)?;
        Ok(())
    }

    async fn extract_uncompressed_tar(&self, package_data: &[u8], cache_dir: &Path) -> Result<()> {
        let temp_dir = TempDir::new()?;
        let temp_path = temp_dir.path();

        let mut archive = tar::Archive::new(package_data);
        info!("Extracting uncompressed tar to: {}", temp_path.display());
        archive.unpack(temp_path)?;

        if cache_dir.exists() {
            fs::remove_dir_all(cache_dir)?;
        }
        fs::create_dir_all(cache_dir.parent().unwrap())?;

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

fn normalize_scope(scope: &str) -> String {
    if scope.starts_with('@') {
        scope.to_string()
    } else {
        format!("@{scope}")
    }
}

pub fn registry_package_web_path(spec: &PackageSpec) -> String {
    if let Some(scope) = &spec.scope {
        format!("{scope}/{}", spec.name)
    } else {
        spec.name.clone()
    }
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
    if let Some(selector) = &spec.version {
        if package_info.versions.contains_key(selector) {
            return Ok(selector.clone());
        }

        if let Some(version) = package_info.dist_tags.get(selector)
            && package_info.versions.contains_key(version)
        {
            return Ok(version.clone());
        }

        if selector == "latest"
            && let Some(version) = package_info
                .latest_version
                .as_ref()
                .filter(|version| package_info.versions.contains_key(*version))
        {
            return Ok(version.clone());
        }

        return Err(RegistryError::VersionNotFound {
            version: selector.clone(),
            package: format_package_spec(spec),
        });
    }

    if let Some(version) = package_info
        .dist_tags
        .get("latest")
        .filter(|version| package_info.versions.contains_key(*version))
    {
        return Ok(version.clone());
    }

    if let Some(version) = package_info
        .latest_version
        .as_ref()
        .filter(|version| package_info.versions.contains_key(*version))
    {
        return Ok(version.clone());
    }

    package_info
        .dist_tags
        .get("codemod-builder")
        .filter(|version| package_info.versions.contains_key(*version))
        .cloned()
        .ok_or_else(|| RegistryError::NoVersionAvailable {
            package: format_package_spec(spec),
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::{Cursor, Write};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn package_locks_are_shared_while_live_and_pruned_after_use() {
        let cache_dir = tempfile::tempdir().expect("cache dir");
        let client = RegistryClient::new(
            RegistryConfig {
                default_registry: "https://example.invalid".to_string(),
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );

        let shared_path = cache_dir.path().join("shared");
        let first = client.package_lock(&shared_path).await;
        let second = client.package_lock(&shared_path).await;
        assert!(Arc::ptr_eq(&first, &second));
        drop(first);
        drop(second);

        for index in 0..100 {
            let lock = client
                .package_lock(&cache_dir.path().join(format!("package-{index}")))
                .await;
            drop(lock);
        }

        let live = client.package_lock(&cache_dir.path().join("live")).await;
        let locks = client.package_locks.lock().await;
        assert_eq!(locks.len(), 1);
        assert!(
            locks
                .get(&cache_dir.path().join("live"))
                .and_then(Weak::upgrade)
                .is_some()
        );
        drop(locks);
        drop(live);
    }

    #[test]
    fn parse_package_spec_accepts_builder_tag() {
        let spec = parse_package_spec("@codemod/example@codemod-builder").unwrap();

        assert_eq!(spec.scope.as_deref(), Some("@codemod"));
        assert_eq!(spec.name, "example");
        assert_eq!(spec.version.as_deref(), Some("codemod-builder"));
    }

    #[test]
    fn determine_version_resolves_builder_tag() {
        let spec = PackageSpec {
            scope: Some("@codemod".to_string()),
            name: "example".to_string(),
            version: Some("codemod-builder".to_string()),
        };
        let package_info = package_info_with_dist_tags();

        assert_eq!(
            determine_version(&spec, &package_info).unwrap(),
            "0.0.1-codemod-builder.1"
        );
    }

    #[test]
    fn determine_version_accepts_exact_prerelease() {
        let spec = PackageSpec {
            scope: Some("@codemod".to_string()),
            name: "example".to_string(),
            version: Some("0.0.1-codemod-builder.1".to_string()),
        };
        let package_info = package_info_with_dist_tags();

        assert_eq!(
            determine_version(&spec, &package_info).unwrap(),
            "0.0.1-codemod-builder.1"
        );
    }

    #[test]
    fn determine_version_defaults_to_latest_dist_tag() {
        let spec = PackageSpec {
            scope: Some("@codemod".to_string()),
            name: "example".to_string(),
            version: None,
        };
        let mut package_info = package_info_with_dist_tags();
        package_info.latest_version = Some("0.9.0".to_string());

        assert_eq!(determine_version(&spec, &package_info).unwrap(), "1.0.0");
    }

    #[test]
    fn determine_version_resolves_explicit_latest_from_legacy_projection() {
        let spec = PackageSpec {
            scope: Some("@codemod".to_string()),
            name: "example".to_string(),
            version: Some("latest".to_string()),
        };
        let mut package_info = package_info_with_dist_tags();
        package_info.dist_tags.clear();

        assert_eq!(determine_version(&spec, &package_info).unwrap(), "1.0.0");
    }

    #[test]
    fn determine_version_ignores_stale_latest_dist_tag() {
        let spec = PackageSpec {
            scope: Some("@codemod".to_string()),
            name: "example".to_string(),
            version: None,
        };
        let mut package_info = package_info_with_dist_tags();
        package_info
            .dist_tags
            .insert("latest".to_string(), "2.0.0".to_string());

        assert_eq!(determine_version(&spec, &package_info).unwrap(), "1.0.0");
    }

    #[test]
    fn determine_version_defaults_to_builder_tag_without_latest_release() {
        let spec = PackageSpec {
            scope: Some("@codemod".to_string()),
            name: "example".to_string(),
            version: None,
        };
        let mut package_info = package_info_with_dist_tags();
        package_info.dist_tags.remove("latest");
        package_info.latest_version = None;

        assert_eq!(
            determine_version(&spec, &package_info).unwrap(),
            "0.0.1-codemod-builder.1"
        );
    }

    #[tokio::test]
    async fn resolve_package_downloads_from_resolved_unscoped_package_info_path() {
        assert_resolve_package_downloads_from_resolved_path(
            "alias-package@1.0.0",
            "/api/v1/registry/packages/alias-package",
            "canonical-package",
            None,
            "/api/v1/registry/packages/canonical-package/download/1.0.0",
        )
        .await;
    }

    #[tokio::test]
    async fn resolve_package_downloads_from_resolved_scoped_package_info_path() {
        assert_resolve_package_downloads_from_resolved_path(
            "@alias/alias-package@1.0.0",
            "/api/v1/registry/packages/@alias/alias-package",
            "canonical-package",
            Some("@codemod"),
            "/api/v1/registry/packages/@codemod/canonical-package/download/1.0.0",
        )
        .await;
    }

    #[tokio::test]
    async fn resolve_package_normalizes_resolved_scope_without_at_prefix() {
        assert_resolve_package_downloads_from_resolved_path(
            "@alias/alias-package@1.0.0",
            "/api/v1/registry/packages/@alias/alias-package",
            "canonical-package",
            Some("codemod"),
            "/api/v1/registry/packages/@codemod/canonical-package/download/1.0.0",
        )
        .await;
    }

    #[tokio::test]
    async fn exact_version_package_info_is_singleflight_and_cached() {
        let source = "@codemod/example@1.0.0";
        let info_path = "/api/v1/registry/packages/@codemod/example";
        let download_path = "/api/v1/registry/packages/@codemod/example/download/1.0.0";
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (registry_url, server) = spawn_registry_server(
            info_path.to_string(),
            package_info_response("example", Some("@codemod")),
            download_path.to_string(),
            package_archive(),
            Arc::clone(&requests),
        )
        .await;
        let cache_dir = tempfile::tempdir().expect("cache dir");
        let client = RegistryClient::new(
            RegistryConfig {
                default_registry: registry_url.clone(),
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );

        let (first, second) = tokio::join!(
            client.resolve_package(source, Some(&registry_url), false, None),
            client.resolve_package(source, Some(&registry_url), false, None),
        );
        first.expect("first resolution");
        second.expect("concurrent resolution");
        client
            .resolve_package(source, Some(&registry_url), false, None)
            .await
            .expect("cached resolution");

        let package_info_requests = requests
            .lock()
            .expect("requests lock")
            .iter()
            .filter(|(method, path)| method == "GET" && path == info_path)
            .count();
        assert_eq!(package_info_requests, 1);
        server.abort();
    }

    #[tokio::test]
    async fn moving_version_package_info_is_not_cached() {
        let source = "@codemod/example@latest";
        let info_path = "/api/v1/registry/packages/@codemod/example";
        let download_path = "/api/v1/registry/packages/@codemod/example/download/1.0.0";
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (registry_url, server) = spawn_registry_server(
            info_path.to_string(),
            package_info_response("example", Some("@codemod")),
            download_path.to_string(),
            package_archive(),
            Arc::clone(&requests),
        )
        .await;
        let cache_dir = tempfile::tempdir().expect("cache dir");
        let client = RegistryClient::new(
            RegistryConfig {
                default_registry: registry_url.clone(),
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );

        client
            .resolve_package(source, Some(&registry_url), false, None)
            .await
            .expect("first resolution");
        client
            .resolve_package(source, Some(&registry_url), false, None)
            .await
            .expect("second resolution");

        let package_info_requests = requests
            .lock()
            .expect("requests lock")
            .iter()
            .filter(|(method, path)| method == "GET" && path == info_path)
            .count();
        assert_eq!(package_info_requests, 2);
        server.abort();
    }

    #[tokio::test]
    async fn forced_resolutions_bypass_package_info_cache() {
        let source = "@codemod/example@1.0.0";
        let info_path = "/api/v1/registry/packages/@codemod/example";
        let download_path = "/api/v1/registry/packages/@codemod/example/download/1.0.0";
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (registry_url, server) = spawn_registry_server(
            info_path.to_string(),
            package_info_response("example", Some("@codemod")),
            download_path.to_string(),
            package_archive(),
            Arc::clone(&requests),
        )
        .await;
        let cache_dir = tempfile::tempdir().expect("cache dir");
        let client = RegistryClient::new(
            RegistryConfig {
                default_registry: registry_url.clone(),
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );

        client
            .resolve_package(source, Some(&registry_url), true, None)
            .await
            .expect("first forced resolution");
        client
            .resolve_package(source, Some(&registry_url), true, None)
            .await
            .expect("second forced resolution");

        let package_info_requests = requests
            .lock()
            .expect("requests lock")
            .iter()
            .filter(|(method, path)| method == "GET" && path == info_path)
            .count();
        assert_eq!(package_info_requests, 2);
        server.abort();
    }

    #[tokio::test]
    async fn package_info_cache_stays_bounded() {
        let cache_dir = tempfile::tempdir().expect("cache dir");
        let client = RegistryClient::new(
            RegistryConfig {
                default_registry: "https://example.com".to_string(),
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );

        for index in 0..=PACKAGE_INFO_CACHE_CAPACITY {
            let cell = client
                .package_info_cache_cell(format!("package-{index}"))
                .await;
            cell.set(Arc::new(package_info_with_dist_tags()))
                .expect("cache cell should be empty");
        }

        assert_eq!(
            client.package_info_cache.lock().await.len(),
            PACKAGE_INFO_CACHE_CAPACITY
        );
    }

    async fn assert_resolve_package_downloads_from_resolved_path(
        source: &str,
        info_path: &str,
        resolved_name: &str,
        resolved_scope: Option<&str>,
        expected_download_path: &str,
    ) {
        let archive = package_archive();
        let package_info = package_info_response(resolved_name, resolved_scope);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (registry_url, server) = spawn_registry_server(
            info_path.to_string(),
            package_info,
            expected_download_path.to_string(),
            archive,
            Arc::clone(&requests),
        )
        .await;

        let cache_dir = tempfile::tempdir().expect("cache dir");
        let client = RegistryClient::new(
            RegistryConfig {
                default_registry: registry_url.clone(),
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );

        let resolved = client
            .resolve_package(source, Some(&registry_url), true, None)
            .await
            .expect("package should resolve and download");

        assert!(resolved.package_dir.join("codemod.yaml").exists());
        assert!(resolved.package_dir.join("workflow.yaml").exists());

        let requests = requests.lock().expect("requests lock");
        assert!(
            requests
                .iter()
                .any(|(method, path)| method == "HEAD" && path == expected_download_path),
            "expected HEAD request to resolved download path {expected_download_path}, got {requests:?}"
        );
        assert!(
            requests
                .iter()
                .any(|(method, path)| method == "GET" && path == expected_download_path),
            "expected GET request to resolved download path {expected_download_path}, got {requests:?}"
        );
        assert!(
            !requests
                .iter()
                .any(|(_, path)| path.contains("alias-package/download")),
            "download should not use parsed input alias path, got {requests:?}"
        );

        server.abort();
    }

    async fn spawn_registry_server(
        info_path: String,
        package_info: String,
        download_path: String,
        archive: Vec<u8>,
        requests: Arc<Mutex<Vec<(String, String)>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test registry");
        let addr = listener.local_addr().expect("test registry address");

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };

                let info_path = info_path.clone();
                let package_info = package_info.clone();
                let download_path = download_path.clone();
                let archive = archive.clone();
                let requests = Arc::clone(&requests);

                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let Ok(size) = stream
                        .readable()
                        .await
                        .and_then(|_| stream.try_read(&mut buffer))
                    else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    let Some(request_line) = request.lines().next() else {
                        return;
                    };
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts.next().unwrap_or_default().to_string();

                    requests
                        .lock()
                        .expect("requests lock")
                        .push((method.clone(), path.clone()));

                    let (status, content_type, body) = if method == "GET" && path == info_path {
                        ("200 OK", "application/json", package_info.into_bytes())
                    } else if path == download_path && method == "HEAD" {
                        ("200 OK", "application/gzip", Vec::new())
                    } else if path == download_path && method == "GET" {
                        ("200 OK", "application/gzip", archive)
                    } else {
                        ("404 Not Found", "text/plain", b"not found".to_vec())
                    };

                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );

                    let _ = stream.writable().await;
                    let _ = stream.try_write(response.as_bytes());
                    if method != "HEAD" && !body.is_empty() {
                        let _ = stream.writable().await;
                        let _ = stream.try_write(&body);
                    }
                });
            }
        });

        (format!("http://{addr}"), handle)
    }

    fn package_info_response(name: &str, scope: Option<&str>) -> String {
        serde_json::json!({
            "id": "pkg_1",
            "name": name,
            "scope": scope,
            "is_legacy": false,
            "latest_version": "1.0.0",
            "versions": {
                "1.0.0": {
                    "version": "1.0.0",
                    "description": null,
                    "checksum": "sha256:test",
                    "size": 1
                }
            }
        })
        .to_string()
    }

    fn package_info_with_dist_tags() -> PackageInfo {
        serde_json::from_value(serde_json::json!({
            "id": "pkg_1",
            "name": "example",
            "scope": "@codemod",
            "is_legacy": false,
            "latest_version": "1.0.0",
            "dist_tags": {
                "latest": "1.0.0",
                "codemod-builder": "0.0.1-codemod-builder.1"
            },
            "versions": {
                "1.0.0": {
                    "version": "1.0.0",
                    "description": null,
                    "checksum": "sha256:latest",
                    "size": 1
                },
                "0.0.1-codemod-builder.1": {
                    "version": "0.0.1-codemod-builder.1",
                    "description": null,
                    "checksum": "sha256:builder",
                    "size": 1
                }
            }
        }))
        .unwrap()
    }

    fn package_archive() -> Vec<u8> {
        let mut tar_buffer = Vec::new();
        {
            let mut archive = tar::Builder::new(&mut tar_buffer);
            append_tar_file(&mut archive, "codemod.yaml", b"name: test\n");
            append_tar_file(&mut archive, "workflow.yaml", b"version: '1'\n");
            archive.finish().expect("finish tar");
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_buffer).expect("write gzip");
        encoder.finish().expect("finish gzip")
    }

    fn append_tar_file(archive: &mut tar::Builder<&mut Vec<u8>>, path: &str, contents: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).expect("set tar path");
        header.set_size(contents.len() as u64);
        header.set_cksum();
        archive
            .append(&header, Cursor::new(contents))
            .expect("append tar file");
    }
}
