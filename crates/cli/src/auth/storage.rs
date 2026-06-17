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
    #[serde(default)]
    pub anonymous_feedback: AnonymousFeedbackConfig,
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
            anonymous_feedback: AnonymousFeedbackConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnonymousFeedbackConfig {
    pub enabled: bool,
    pub consented_at: Option<String>,
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

    pub fn save_config(&self, config: &Config) -> Result<()> {
        let config_path = self.config_dir.join("config.json");
        let content =
            serde_json::to_string_pretty(config).context("Failed to serialize config file")?;

        fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config file: {config_path:?}"))?;

        Ok(())
    }

    pub fn enable_anonymous_feedback(&self) -> Result<Config> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let mut config = self.load_config_with_env(Some(&env))?;
        let consented_at = config
            .anonymous_feedback
            .consented_at
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        config.anonymous_feedback = AnonymousFeedbackConfig {
            enabled: true,
            consented_at: Some(consented_at),
        };
        self.save_config(&config)?;
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

#[cfg(test)]
mod tests {
    use super::TokenStorage;
    use std::fs;

    #[test]
    fn missing_feedback_config_defaults_to_disabled() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        fs::write(
            temp_dir.path().join("config.json"),
            r#"{
  "default_registry": "https://app.codemod.com",
  "registries": {}
}"#,
        )
        .expect("expected config write");

        let storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");
        let config = storage.load_config().expect("config");

        assert!(!config.anonymous_feedback.enabled);
        assert_eq!(config.anonymous_feedback.consented_at, None);
    }

    #[test]
    fn enable_anonymous_feedback_persists_consent() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");

        let config = storage
            .enable_anonymous_feedback()
            .expect("expected feedback consent write");
        let reloaded = storage.load_config().expect("expected config reload");

        assert!(config.anonymous_feedback.enabled);
        assert!(config.anonymous_feedback.consented_at.is_some());
        assert!(reloaded.anonymous_feedback.enabled);
        assert_eq!(
            reloaded.anonymous_feedback.consented_at,
            config.anonymous_feedback.consented_at
        );
    }

    #[test]
    fn enable_anonymous_feedback_preserves_existing_consent_date() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        fs::write(
            temp_dir.path().join("config.json"),
            r#"{
  "default_registry": "https://app.codemod.com",
  "registries": {},
  "anonymous_feedback": {
    "enabled": true,
    "consented_at": "2026-06-09T12:00:00Z"
  }
}"#,
        )
        .expect("expected config write");
        let storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");

        let config = storage
            .enable_anonymous_feedback()
            .expect("expected feedback consent write");

        assert!(config.anonymous_feedback.enabled);
        assert_eq!(
            config.anonymous_feedback.consented_at.as_deref(),
            Some("2026-06-09T12:00:00Z")
        );
    }
}
