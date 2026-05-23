use anyhow::{bail, Context, Result};
use butterflow_core::registry::RegistryConfig;
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::auth::types::{AuthTokens, UserInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_registry: String,
    pub registries: HashMap<String, RegistryAuthConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self::with_default_registry(RegistryConfig::default().default_registry.to_string())
    }
}

impl Config {
    fn with_default_registry(registry_url: String) -> Self {
        let mut registries = HashMap::new();

        registries.insert(
            registry_url.to_string(),
            RegistryAuthConfig {
                auth_url: format!("{registry_url}/api/auth/oauth2/authorize"),
                token_url: format!("{registry_url}/api/auth/oauth2/token"),
                client_id: "LaqxmrfBSiCAGzVywTqUxGgqgKVdzaLg".to_string(),
                scopes: vec![
                    "read".to_string(),
                    "write".to_string(),
                    "publish".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
        );

        Self {
            default_registry: registry_url.to_string(),
            registries,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryAuthConfig {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAuth {
    pub tokens: AuthTokens,
    pub user: UserInfo,
    pub registry: String,
}

pub struct TokenStorage {
    config_dir: PathBuf,
}

impl TokenStorage {
    pub fn new() -> Result<Self> {
        let config_dir = config_dir()
            .context("Could not determine config directory")?
            .join("codemod");
        Self::with_config_dir(config_dir)
    }

    pub fn with_config_dir(config_dir: PathBuf) -> Result<Self> {
        // Create config directory if it doesn't exist
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir)
                .with_context(|| format!("Failed to create config directory: {config_dir:?}"))?;
        }

        Ok(Self { config_dir })
    }

    pub fn load_config(&self) -> Result<Config> {
        self.load_config_with_env(None)
    }

    pub fn load_config_with_env(&self, env: Option<&HashMap<String, String>>) -> Result<Config> {
        let config_path = self.config_dir.join("config.json");

        if !config_path.exists() {
            let default_registry = env
                .and_then(|vars| vars.get("CODEMOD_REGISTRY_URL"))
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string())
                .unwrap_or_else(|| RegistryConfig::default().default_registry);
            return Ok(Config::with_default_registry(default_registry));
        }

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {config_path:?}"))?;

        let config: Config =
            serde_json::from_str(&content).context("Failed to parse config file")?;

        Ok(config)
    }

    pub fn load_auth(&self, registry: &str) -> Result<Option<StoredAuth>> {
        let auth_path = self.get_auth_path(registry);

        if !auth_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&auth_path)
            .with_context(|| format!("Failed to read auth file: {auth_path:?}"))?;

        let auth: StoredAuth =
            serde_json::from_str(&content).context("Failed to parse auth file")?;

        Ok(Some(auth))
    }

    pub fn save_auth(&self, auth: &StoredAuth) -> Result<()> {
        let auth_path = self.get_auth_path(&auth.registry);

        // Create auth directory if it doesn't exist
        if let Some(parent) = auth_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create auth directory: {parent:?}"))?;
        }

        let content =
            serde_json::to_string_pretty(auth).context("Failed to serialize auth data")?;

        write_auth_file(&auth_path, content.as_bytes())
            .with_context(|| format!("Failed to write auth file: {auth_path:?}"))?;

        Ok(())
    }

    pub fn remove_auth(&self, registry: &str) -> Result<()> {
        let auth_path = self.get_auth_path(registry);

        if auth_path.exists() {
            fs::remove_file(&auth_path)
                .with_context(|| format!("Failed to remove auth file: {auth_path:?}"))?;
        }

        Ok(())
    }

    pub fn clear_cache(&self) -> Result<()> {
        let cache_dir = self.config_dir.join("cache");

        if cache_dir.exists() {
            fs::remove_dir_all(&cache_dir)
                .with_context(|| format!("Failed to remove cache directory: {cache_dir:?}"))?;
        }

        Ok(())
    }

    pub fn get_auth_for_registry(&self, registry: &str) -> Result<Option<StoredAuth>> {
        self.load_auth(registry)
    }

    fn get_auth_path(&self, registry: &str) -> PathBuf {
        let auth_dir = self.config_dir.join("auth");
        let filename = format!("{}.json", Self::sanitize_registry_name(registry));
        auth_dir.join(filename)
    }

    fn sanitize_registry_name(registry: &str) -> String {
        registry
            .replace("://", "_")
            .replace("/", "_")
            .replace(":", "_")
    }
}

#[cfg(unix)]
fn write_auth_file(path: &std::path::Path, content: &[u8]) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let restricted_permissions = fs::Permissions::from_mode(0o600);

    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            bail!("Auth path exists but is not a regular file: {path:?}");
        }
        fs::set_permissions(path, restricted_permissions.clone())?;
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content)?;
    file.flush()?;
    fs::set_permissions(path, restricted_permissions)?;

    Ok(())
}

#[cfg(not(unix))]
fn write_auth_file(path: &std::path::Path, content: &[u8]) -> Result<()> {
    fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::types::{AuthTokens, UserInfo};

    fn stored_auth(registry: &str) -> StoredAuth {
        StoredAuth {
            tokens: AuthTokens {
                access_token: "access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                expires_at: None,
                scope: vec!["read".to_string()],
                token_type: "Bearer".to_string(),
            },
            user: UserInfo {
                id: "user-id".to_string(),
                username: "user".to_string(),
                email: "user@example.com".to_string(),
                organizations: None,
            },
            registry: registry.to_string(),
        }
    }

    #[test]
    fn save_auth_round_trips_stored_auth() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = TokenStorage::with_config_dir(temp_dir.path().join("codemod")).unwrap();
        let auth = stored_auth("https://app.codemod.com");

        storage.save_auth(&auth).unwrap();

        let loaded = storage
            .load_auth("https://app.codemod.com")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.tokens.access_token, "access-token");
        assert_eq!(
            loaded.tokens.refresh_token.as_deref(),
            Some("refresh-token")
        );
        assert_eq!(loaded.user.email, "user@example.com");
        assert_eq!(loaded.registry, "https://app.codemod.com");
    }

    #[cfg(unix)]
    #[test]
    fn save_auth_creates_token_file_with_user_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let storage = TokenStorage::with_config_dir(temp_dir.path().join("codemod")).unwrap();
        let auth = stored_auth("https://app.codemod.com");

        storage.save_auth(&auth).unwrap();

        let auth_path = storage.get_auth_path("https://app.codemod.com");
        let mode = fs::metadata(auth_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn save_auth_restricts_existing_permissive_token_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let storage = TokenStorage::with_config_dir(temp_dir.path().join("codemod")).unwrap();
        let auth = stored_auth("https://app.codemod.com");
        let auth_path = storage.get_auth_path("https://app.codemod.com");
        fs::create_dir_all(auth_path.parent().unwrap()).unwrap();
        fs::write(&auth_path, "{}").unwrap();
        fs::set_permissions(&auth_path, fs::Permissions::from_mode(0o644)).unwrap();

        storage.save_auth(&auth).unwrap();

        let mode = fs::metadata(auth_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn save_auth_rejects_non_file_auth_path_without_changing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let storage = TokenStorage::with_config_dir(temp_dir.path().join("codemod")).unwrap();
        let auth = stored_auth("https://app.codemod.com");
        let auth_path = storage.get_auth_path("https://app.codemod.com");
        fs::create_dir_all(&auth_path).unwrap();
        fs::set_permissions(&auth_path, fs::Permissions::from_mode(0o755)).unwrap();

        let result = storage.save_auth(&auth);

        assert!(result.is_err());
        let mode = fs::metadata(auth_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }
}
