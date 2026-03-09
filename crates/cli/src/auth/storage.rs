use anyhow::{Context, Result};
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

        fs::write(&auth_path, content)
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
