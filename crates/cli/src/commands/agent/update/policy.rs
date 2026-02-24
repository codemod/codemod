use super::types::{
    ManagedUpdateManifest, RemoteManifestSnapshot, UpdatePolicyContext, UpdatePolicyMode,
    MANAGED_UPDATE_MANIFEST_CACHE_RELATIVE_DIR, MANAGED_UPDATE_MANIFEST_CACHE_TTL_SECS,
    MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR, MANAGED_UPDATE_MANIFEST_REQUEST_TIMEOUT_SECS,
    MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER, MANAGED_UPDATE_POLICY_LOCAL_SOURCE,
    MANAGED_UPDATE_REGISTRY_MANIFEST_PATH,
};
use crate::auth::TokenStorage;
use base64::Engine;
use butterflow_core::utils::get_cache_dir;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(in crate::commands::agent) const DEFAULT_UPDATE_SOURCE: &str = "registry";

#[derive(Clone, Debug)]
pub(in crate::commands::agent) struct UpdatePolicyResolveOptions {
    pub(in crate::commands::agent) mode: UpdatePolicyMode,
    pub(in crate::commands::agent) remote_source: String,
    pub(in crate::commands::agent) require_signed_manifest: Option<bool>,
}

pub(in crate::commands::agent) async fn resolve_update_policy_context(
    options: &UpdatePolicyResolveOptions,
) -> std::result::Result<UpdatePolicyContext, String> {
    let remote_source = parse_update_remote_source_value(&options.remote_source)?;
    let (authenticity_config, authenticity_warning) =
        resolve_manifest_authenticity_config(options.mode, options.require_signed_manifest);
    let cache_ttl = resolve_manifest_cache_ttl();
    let mut warnings = Vec::new();
    if let Some(warning) = authenticity_warning {
        warnings.push(warning);
    }

    let mut remote_manifest = None;
    let fallback_applied = options.mode != UpdatePolicyMode::Manual;
    if fallback_applied {
        if remote_source == MANAGED_UPDATE_POLICY_LOCAL_SOURCE {
            warnings.push(format!(
                "Update policy `{}` requested with local-only source; applying deterministic local fallback.",
                options.mode.as_str()
            ));
        } else {
            match fetch_remote_update_manifest_with_cache(
                &remote_source,
                cache_ttl,
                &authenticity_config,
            )
            .await
            {
                Ok(resolution) => {
                    warnings.extend(resolution.warnings);
                    remote_manifest = Some(resolution.snapshot);
                }
                Err(error) => warnings.push(format!(
                    "Remote update manifest lookup failed ({error}). Applying deterministic local fallback."
                )),
            }
        }
    }

    Ok(UpdatePolicyContext {
        mode: options.mode,
        remote_source,
        fallback_applied,
        remote_manifest,
        warnings,
    })
}

#[derive(Clone, Debug)]
struct RemoteManifestResolution {
    snapshot: RemoteManifestSnapshot,
    warnings: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CachedManifestEnvelope {
    remote_source: String,
    endpoint: String,
    fetched_at_epoch_secs: u64,
    #[serde(default)]
    authenticity_verified: bool,
    manifest: ManagedUpdateManifest,
}

#[derive(Clone, Debug)]
struct CachedManifestSnapshot {
    snapshot: RemoteManifestSnapshot,
    age_secs: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManifestAuthenticityMode {
    Optional,
    Required,
}

impl ManifestAuthenticityMode {
    fn is_required(self) -> bool {
        matches!(self, Self::Required)
    }
}

#[derive(Clone, Debug)]
struct ManifestAuthenticityConfig {
    mode: ManifestAuthenticityMode,
    public_key: Option<VerifyingKey>,
}

#[derive(Clone, Debug)]
struct FetchedRemoteManifest {
    snapshot: RemoteManifestSnapshot,
}

fn resolve_manifest_cache_ttl() -> Duration {
    Duration::from_secs(MANAGED_UPDATE_MANIFEST_CACHE_TTL_SECS)
}

fn resolve_manifest_authenticity_config(
    mode: UpdatePolicyMode,
    require_signed_manifest: Option<bool>,
) -> (ManifestAuthenticityConfig, Option<String>) {
    let requirement = match require_signed_manifest {
        Some(true) => ManifestAuthenticityMode::Required,
        Some(false) => ManifestAuthenticityMode::Optional,
        None if mode == UpdatePolicyMode::AutoSafe => ManifestAuthenticityMode::Required,
        None => ManifestAuthenticityMode::Optional,
    };

    match resolve_manifest_public_key() {
        Ok(public_key) => (
            ManifestAuthenticityConfig {
                mode: requirement,
                public_key,
            },
            None,
        ),
        Err(error) => (
            ManifestAuthenticityConfig {
                mode: requirement,
                public_key: None,
            },
            Some(error),
        ),
    }
}

fn resolve_manifest_public_key() -> std::result::Result<Option<VerifyingKey>, String> {
    let raw_value = match std::env::var(MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!(
                "Invalid {} value (non-unicode).",
                MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR
            ));
        }
    };

    let trimmed = raw_value.trim().trim_start_matches("ed25519:");
    if trimmed.is_empty() {
        return Err(format!(
            "Empty {} value.",
            MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR
        ));
    }
    let bytes = decode_base64_bytes(trimmed).map_err(|error| {
        format!(
            "Failed to decode {} as base64: {error}",
            MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR
        )
    })?;
    let key_bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        format!(
            "{} must decode to exactly 32 bytes",
            MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR
        )
    })?;
    let key = VerifyingKey::from_bytes(&key_bytes).map_err(|error| {
        format!(
            "Invalid {} value: {error}",
            MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR
        )
    })?;
    Ok(Some(key))
}

fn decode_base64_bytes(value: &str) -> std::result::Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(value))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(value))
        .map_err(|error| error.to_string())
}

fn verify_remote_manifest_authenticity(
    payload: &[u8],
    signature_header: Option<&str>,
    config: &ManifestAuthenticityConfig,
) -> std::result::Result<bool, String> {
    let signature = signature_header
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (config.mode, config.public_key.as_ref(), signature) {
        (ManifestAuthenticityMode::Required, None, _) => Err(format!(
            "manifest authenticity is required but {} is not configured",
            MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR
        )),
        (ManifestAuthenticityMode::Required, Some(_), None) => Err(format!(
            "manifest authenticity is required but response header `{}` is missing",
            MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER
        )),
        (_, None, Some(_)) => Err(format!(
            "response header `{}` was provided, but {} is not configured",
            MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER, MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR
        )),
        (_, Some(_), None) if config.mode.is_required() => Err(format!(
            "manifest authenticity is required but response header `{}` is missing",
            MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER
        )),
        (_, Some(_), None) => Ok(false),
        (_, None, None) => Ok(false),
        (_, Some(public_key), Some(signature_value)) => {
            let signature_bytes = decode_base64_bytes(signature_value).map_err(|error| {
                format!(
                    "failed to decode `{}` header as base64: {error}",
                    MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER
                )
            })?;
            let signature_array: [u8; 64] = signature_bytes.try_into().map_err(|_| {
                format!(
                    "`{}` header must decode to exactly 64 bytes",
                    MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER
                )
            })?;
            let signature = Signature::from_bytes(&signature_array);
            public_key.verify(payload, &signature).map_err(|error| {
                format!("remote manifest signature verification failed: {error}")
            })?;
            Ok(true)
        }
    }
}

async fn fetch_remote_update_manifest_with_cache(
    remote_source: &str,
    cache_ttl: Duration,
    authenticity_config: &ManifestAuthenticityConfig,
) -> std::result::Result<RemoteManifestResolution, String> {
    let mut warnings = Vec::new();

    let fresh_cache = match load_cached_remote_manifest(
        remote_source,
        cache_ttl,
        false,
        authenticity_config.mode,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            warnings.push(format!(
                "Could not read remote manifest cache ({error}); attempting network lookup."
            ));
            None
        }
    };
    if let Some(cached) = fresh_cache {
        warnings.push(format!(
            "Using cached remote update manifest from {} (age {}s, ttl {}s). Deterministic local install fallback remains active until remote apply is implemented.",
            cached.snapshot.source,
            cached.age_secs,
            cache_ttl.as_secs(),
        ));
        return Ok(RemoteManifestResolution {
            snapshot: cached.snapshot,
            warnings,
        });
    }

    let stale_cache = match load_cached_remote_manifest(
        remote_source,
        cache_ttl,
        true,
        authenticity_config.mode,
    ) {
        Ok(snapshot) => snapshot.filter(|cached| cached.age_secs > cache_ttl.as_secs()),
        Err(error) => {
            warnings.push(format!(
                "Could not read stale remote manifest cache ({error}); proceeding without cache fallback."
            ));
            None
        }
    };

    match fetch_remote_update_manifest(remote_source, authenticity_config).await {
        Ok(fetch) => {
            let snapshot = fetch.snapshot;
            let schema_version = snapshot.manifest.schema_version.clone();
            let component_count = snapshot.manifest.components.len();
            match save_cached_remote_manifest(remote_source, &snapshot) {
                Ok(()) => warnings.push(format!(
                    "Loaded remote update manifest from {} (schema {}, {} components) and refreshed local cache. Deterministic local install fallback remains active until remote apply is implemented.",
                    snapshot.source, schema_version, component_count
                )),
                Err(error) => warnings.push(format!(
                    "Loaded remote update manifest from {} (schema {}, {} components), but failed to refresh local cache ({error}). Deterministic local install fallback remains active until remote apply is implemented.",
                    snapshot.source, schema_version, component_count
                )),
            }
            Ok(RemoteManifestResolution { snapshot, warnings })
        }
        Err(fetch_error) => {
            if let Some(stale) = stale_cache {
                warnings.push(format!(
                    "Remote update manifest lookup failed ({fetch_error}); using stale cached manifest from {} (age {}s). Deterministic local install fallback remains active until remote apply is implemented.",
                    stale.snapshot.source, stale.age_secs
                ));
                return Ok(RemoteManifestResolution {
                    snapshot: stale.snapshot,
                    warnings,
                });
            }
            Err(fetch_error)
        }
    }
}

fn save_cached_remote_manifest(
    remote_source: &str,
    snapshot: &RemoteManifestSnapshot,
) -> std::result::Result<(), String> {
    let cache_base_dir = resolve_manifest_cache_base_dir()?;
    save_cached_remote_manifest_to_base(remote_source, snapshot, &cache_base_dir, now_epoch_secs())
}

fn save_cached_remote_manifest_to_base(
    remote_source: &str,
    snapshot: &RemoteManifestSnapshot,
    cache_base_dir: &Path,
    fetched_at_epoch_secs: u64,
) -> std::result::Result<(), String> {
    let cache_path = managed_update_manifest_cache_path_for_base(remote_source, cache_base_dir);
    let parent = cache_path.parent().ok_or_else(|| {
        format!(
            "failed to resolve parent directory for manifest cache path {}",
            cache_path.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create remote manifest cache directory {}: {error}",
            parent.display()
        )
    })?;

    let envelope = CachedManifestEnvelope {
        remote_source: remote_source.to_string(),
        endpoint: snapshot.source.clone(),
        fetched_at_epoch_secs,
        authenticity_verified: snapshot.authenticity_verified,
        manifest: snapshot.manifest.clone(),
    };
    let payload = serde_json::to_vec(&envelope)
        .map_err(|error| format!("failed to serialize remote manifest cache payload: {error}"))?;
    fs::write(&cache_path, payload).map_err(|error| {
        format!(
            "failed to write remote manifest cache file {}: {error}",
            cache_path.display()
        )
    })?;
    Ok(())
}

fn load_cached_remote_manifest(
    remote_source: &str,
    cache_ttl: Duration,
    allow_stale: bool,
    authenticity_mode: ManifestAuthenticityMode,
) -> std::result::Result<Option<CachedManifestSnapshot>, String> {
    let cache_base_dir = resolve_manifest_cache_base_dir()?;
    load_cached_remote_manifest_from_base(
        remote_source,
        cache_ttl,
        allow_stale,
        authenticity_mode,
        &cache_base_dir,
    )
}

fn load_cached_remote_manifest_from_base(
    remote_source: &str,
    cache_ttl: Duration,
    allow_stale: bool,
    authenticity_mode: ManifestAuthenticityMode,
    cache_base_dir: &Path,
) -> std::result::Result<Option<CachedManifestSnapshot>, String> {
    let cache_path = managed_update_manifest_cache_path_for_base(remote_source, cache_base_dir);
    if !cache_path.exists() {
        return Ok(None);
    }

    let payload = fs::read(&cache_path).map_err(|error| {
        format!(
            "failed to read remote manifest cache file {}: {error}",
            cache_path.display()
        )
    })?;
    let envelope: CachedManifestEnvelope = serde_json::from_slice(&payload).map_err(|error| {
        format!(
            "failed to parse remote manifest cache file {}: {error}",
            cache_path.display()
        )
    })?;
    if envelope.remote_source.trim() != remote_source.trim() {
        return Err(format!("cache source mismatch in {}", cache_path.display()));
    }
    if authenticity_mode.is_required() && !envelope.authenticity_verified {
        return Err(format!(
            "cached manifest at {} was not authenticity-verified",
            cache_path.display()
        ));
    }
    validate_remote_update_manifest(&envelope.manifest)?;

    let age_secs = now_epoch_secs().saturating_sub(envelope.fetched_at_epoch_secs);
    if !allow_stale && age_secs > cache_ttl.as_secs() {
        return Ok(None);
    }

    Ok(Some(CachedManifestSnapshot {
        snapshot: RemoteManifestSnapshot {
            source: envelope.endpoint,
            manifest: envelope.manifest,
            authenticity_verified: envelope.authenticity_verified,
        },
        age_secs,
    }))
}

fn resolve_manifest_cache_base_dir() -> std::result::Result<PathBuf, String> {
    get_cache_dir().map_err(|error| format!("failed to resolve cache directory: {error}"))
}

fn managed_update_manifest_cache_path_for_base(
    remote_source: &str,
    cache_base_dir: &Path,
) -> PathBuf {
    let source_hash = sha256_hex(remote_source.trim());
    cache_base_dir
        .join(MANAGED_UPDATE_MANIFEST_CACHE_RELATIVE_DIR)
        .join(format!("{source_hash}.json"))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn fetch_remote_update_manifest(
    remote_source: &str,
    authenticity_config: &ManifestAuthenticityConfig,
) -> std::result::Result<FetchedRemoteManifest, String> {
    let endpoint = remote_manifest_endpoint(remote_source)?;
    let parsed_endpoint = url::Url::parse(&endpoint)
        .map_err(|error| format!("invalid manifest endpoint: {error}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(
            MANAGED_UPDATE_MANIFEST_REQUEST_TIMEOUT_SECS,
        ))
        .build()
        .map_err(|error| format!("failed to initialize HTTP client: {error}"))?;

    let request = maybe_attach_registry_auth(client.get(parsed_endpoint), remote_source);

    let response = request
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "HTTP {} while fetching remote manifest: {}",
            status, body
        ));
    }

    let signature_header = response
        .headers()
        .get(MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let payload = response
        .bytes()
        .await
        .map_err(|error| format!("failed to read remote manifest body: {error}"))?;
    let authenticity_verified = verify_remote_manifest_authenticity(
        payload.as_ref(),
        signature_header.as_deref(),
        authenticity_config,
    )?;
    let manifest: ManagedUpdateManifest = serde_json::from_slice(payload.as_ref())
        .map_err(|error| format!("failed to parse remote manifest JSON: {error}"))?;
    validate_remote_update_manifest(&manifest)?;

    Ok(FetchedRemoteManifest {
        snapshot: RemoteManifestSnapshot {
            source: endpoint,
            manifest,
            authenticity_verified,
        },
    })
}

pub(in crate::commands::agent) fn remote_manifest_endpoint(
    remote_source: &str,
) -> std::result::Result<String, String> {
    if let Some(url) = remote_source.strip_prefix("url:") {
        let normalized = url.trim();
        if normalized.is_empty() {
            return Err("remote source URL is empty".to_string());
        }
        return Ok(normalized.to_string());
    }

    if let Some(registry_url) = remote_source.strip_prefix("registry:") {
        let normalized = registry_url.trim();
        if normalized.is_empty() {
            return Err("registry source URL is empty".to_string());
        }
        return Ok(format!(
            "{}{}",
            normalized.trim_end_matches('/'),
            MANAGED_UPDATE_REGISTRY_MANIFEST_PATH
        ));
    }

    Err(format!(
        "unsupported remote source format `{remote_source}`"
    ))
}

pub(in crate::commands::agent) fn registry_source_base_url(remote_source: &str) -> Option<String> {
    remote_source
        .strip_prefix("registry:")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(in crate::commands::agent) fn validate_remote_update_manifest(
    manifest: &ManagedUpdateManifest,
) -> std::result::Result<(), String> {
    if manifest.schema_version.trim().is_empty() {
        return Err("schema_version is required".to_string());
    }
    if let Some(generated_at) = manifest.generated_at.as_deref() {
        if generated_at.trim().is_empty() {
            return Err("generated_at cannot be empty when present".to_string());
        }
    }
    if manifest.components.is_empty() {
        return Err("components must contain at least one entry".to_string());
    }

    let mut ids = HashSet::new();
    for component in &manifest.components {
        if component.id.trim().is_empty() {
            return Err("component id is required".to_string());
        }
        if !ids.insert(component.id.clone()) {
            return Err(format!("duplicate component id `{}`", component.id));
        }
        if component.kind.trim().is_empty() {
            return Err(format!("component `{}` has empty kind", component.id));
        }
        if component.version.trim().is_empty() {
            return Err(format!("component `{}` has empty version", component.id));
        }
        if !is_valid_sha256_hex(&component.checksum_sha256) {
            return Err(format!(
                "component `{}` has invalid checksum_sha256",
                component.id
            ));
        }
        if url::Url::parse(component.source_url.trim()).is_err() {
            return Err(format!(
                "component `{}` has invalid source_url",
                component.id
            ));
        }
        if let Some(min_cli_version) = component.min_cli_version.as_deref() {
            if min_cli_version.trim().is_empty() {
                return Err(format!(
                    "component `{}` has empty min_cli_version",
                    component.id
                ));
            }
        }
        if let Some(max_cli_version) = component.max_cli_version.as_deref() {
            if max_cli_version.trim().is_empty() {
                return Err(format!(
                    "component `{}` has empty max_cli_version",
                    component.id
                ));
            }
        }
        if let Some(harnesses) = &component.harnesses {
            if harnesses.is_empty() {
                return Err(format!(
                    "component `{}` has empty harnesses list",
                    component.id
                ));
            }
            if harnesses.iter().any(|harness| harness.trim().is_empty()) {
                return Err(format!(
                    "component `{}` has blank harness entry",
                    component.id
                ));
            }
        }
    }

    Ok(())
}

pub(in crate::commands::agent) fn is_valid_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

pub(in crate::commands::agent) fn parse_update_remote_source_value(
    raw_value: &str,
) -> std::result::Result<String, String> {
    let normalized = raw_value.trim();
    if normalized.is_empty() {
        return Err(
            "update source cannot be empty; use `local`, `registry`, or an absolute URL"
                .to_string(),
        );
    }

    let normalized_lower = normalized.to_ascii_lowercase();
    if normalized_lower == "local" {
        return Ok(MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string());
    }

    if normalized_lower == "registry" {
        return resolve_default_registry_source().map_err(|error| {
            format!(
                "could not resolve registry update source: {error}. Use `--update-source <absolute-url>` or configure default registry."
            )
        });
    }

    if normalized_lower.starts_with("registry:") || normalized_lower.starts_with("url:") {
        return Err(format!(
            "unsupported update source `{normalized}`; use `local`, `registry`, or an absolute URL"
        ));
    }

    match url::Url::parse(normalized) {
        Ok(parsed) if parsed.scheme() == "http" || parsed.scheme() == "https" => {
            Ok(format!("url:{parsed}"))
        }
        Ok(_) => Err(format!(
            "unsupported update source `{normalized}`; only http/https URLs are supported"
        )),
        Err(error) => Err(format!(
            "unsupported update source `{normalized}` ({error}); use `local`, `registry`, or an absolute URL"
        )),
    }
}

pub(in crate::commands::agent) fn resolve_default_registry_source(
) -> std::result::Result<String, String> {
    let storage = TokenStorage::new()
        .map_err(|error| format!("failed to initialize token storage: {error}"))?;
    let config = storage
        .load_config()
        .map_err(|error| format!("failed to load CLI config: {error}"))?;
    let registry_url = config.default_registry.trim();
    if registry_url.is_empty() {
        return Err("default registry is empty".to_string());
    }

    let parsed = url::Url::parse(registry_url)
        .map_err(|error| format!("invalid default registry URL `{registry_url}`: {error}"))?;
    Ok(format!("registry:{parsed}"))
}

pub(in crate::commands::agent) fn maybe_attach_registry_auth(
    mut request: reqwest::RequestBuilder,
    remote_source: &str,
) -> reqwest::RequestBuilder {
    if let Some(registry_url) = registry_source_base_url(remote_source) {
        if let Ok(storage) = TokenStorage::new() {
            if let Ok(Some(auth)) = storage.get_auth_for_registry(&registry_url) {
                request = request.bearer_auth(auth.tokens.access_token);
            }
        }
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::agent::update::types::ManagedUpdateManifestComponent;
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    fn sample_remote_snapshot(version: &str) -> RemoteManifestSnapshot {
        RemoteManifestSnapshot {
            source: "https://app.codemod.com/api/v1/agent/managed-components/manifest".to_string(),
            authenticity_verified: true,
            manifest: ManagedUpdateManifest {
                schema_version: "1".to_string(),
                generated_at: None,
                components: vec![ManagedUpdateManifestComponent {
                    id: "codemod".to_string(),
                    kind: "skill".to_string(),
                    version: version.to_string(),
                    checksum_sha256:
                        "d8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                            .to_string(),
                    source_url: "https://updates.codemod.com/codemod.tar.gz".to_string(),
                    min_cli_version: None,
                    max_cli_version: None,
                    harnesses: None,
                }],
            },
        }
    }

    #[test]
    fn manifest_cache_roundtrip_loads_fresh_snapshot() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let remote_source = "registry:https://app.codemod.com/";
        let snapshot = sample_remote_snapshot("1.2.3");

        save_cached_remote_manifest_to_base(
            remote_source,
            &snapshot,
            temp_dir.path(),
            now_epoch_secs(),
        )
        .expect("expected cache write");

        let cached = load_cached_remote_manifest_from_base(
            remote_source,
            Duration::from_secs(300),
            false,
            ManifestAuthenticityMode::Optional,
            temp_dir.path(),
        )
        .expect("expected cache read")
        .expect("expected fresh snapshot");

        assert_eq!(cached.snapshot.source, snapshot.source);
        assert_eq!(
            cached.snapshot.manifest.components[0].version,
            snapshot.manifest.components[0].version
        );
    }

    #[test]
    fn manifest_cache_respects_ttl_for_fresh_reads() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let remote_source = "registry:https://app.codemod.com/";
        let snapshot = sample_remote_snapshot("1.2.3");

        save_cached_remote_manifest_to_base(
            remote_source,
            &snapshot,
            temp_dir.path(),
            now_epoch_secs().saturating_sub(300),
        )
        .expect("expected cache write");

        let cached = load_cached_remote_manifest_from_base(
            remote_source,
            Duration::from_secs(60),
            false,
            ManifestAuthenticityMode::Optional,
            temp_dir.path(),
        )
        .expect("expected cache read");
        assert!(cached.is_none());
    }

    #[test]
    fn manifest_cache_can_return_stale_snapshot_when_allowed() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let remote_source = "registry:https://app.codemod.com/";
        let snapshot = sample_remote_snapshot("1.2.3");

        save_cached_remote_manifest_to_base(
            remote_source,
            &snapshot,
            temp_dir.path(),
            now_epoch_secs().saturating_sub(300),
        )
        .expect("expected cache write");

        let cached = load_cached_remote_manifest_from_base(
            remote_source,
            Duration::from_secs(60),
            true,
            ManifestAuthenticityMode::Optional,
            temp_dir.path(),
        )
        .expect("expected cache read")
        .expect("expected stale snapshot");

        assert!(cached.age_secs >= 300);
        assert_eq!(cached.snapshot.source, snapshot.source);
    }

    #[test]
    fn verify_remote_manifest_authenticity_optional_accepts_unsigned_payload() {
        let config = ManifestAuthenticityConfig {
            mode: ManifestAuthenticityMode::Optional,
            public_key: None,
        };
        let verified = verify_remote_manifest_authenticity(b"{}", None, &config)
            .expect("expected unsigned optional verification path");
        assert!(!verified);
    }

    #[test]
    fn verify_remote_manifest_authenticity_required_rejects_missing_signature_or_key() {
        let signing_key = SigningKey::from_bytes(&[3_u8; 32]);
        let public_key = signing_key.verifying_key();
        let required_without_key = ManifestAuthenticityConfig {
            mode: ManifestAuthenticityMode::Required,
            public_key: None,
        };
        let missing_key_error =
            verify_remote_manifest_authenticity(b"{}", None, &required_without_key).unwrap_err();
        assert!(missing_key_error.contains("not configured"));

        let required_with_key = ManifestAuthenticityConfig {
            mode: ManifestAuthenticityMode::Required,
            public_key: Some(public_key),
        };
        let missing_signature_error =
            verify_remote_manifest_authenticity(b"{}", None, &required_with_key).unwrap_err();
        assert!(missing_signature_error.contains("missing"));
    }

    #[test]
    fn verify_remote_manifest_authenticity_accepts_valid_signed_payload() {
        let signing_key = SigningKey::from_bytes(&[5_u8; 32]);
        let payload = br#"{"schemaVersion":"1","components":[]}"#;
        let signature = signing_key.sign(payload);
        let signature_header =
            base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        let config = ManifestAuthenticityConfig {
            mode: ManifestAuthenticityMode::Required,
            public_key: Some(signing_key.verifying_key()),
        };

        let verified =
            verify_remote_manifest_authenticity(payload, Some(&signature_header), &config)
                .expect("expected signature verification");
        assert!(verified);
    }
}
