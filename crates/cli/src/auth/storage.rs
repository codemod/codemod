use anyhow::{Context, Result};
use butterflow_core::registry::RegistryConfig;
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::auth::types::{AuthTokens, UserInfo};

/// `config.json` in the codemod config directory. Other parts of the CLI
/// keep their own keys in the same file (`agent` and `decisionModel`, read by
/// `user_settings` and by `codemod/assess`), so keys this struct does not name
/// are kept as they are when it is saved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    // Defaulted to empty, not to `Config::default()`: the file may hold only
    // run preferences when nobody has logged in yet, and `load_config_with_env`
    // then fills in the registry settings, `CODEMOD_REGISTRY_URL` included.
    #[serde(default)]
    pub default_registry: String,
    #[serde(default)]
    pub registries: HashMap<String, RegistryAuthConfig>,
    pub anonymous_feedback: AnonymousFeedbackConfig,
    /// Every other key (run preferences, see `user_settings`).
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
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
            other: serde_json::Map::new(),
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
            fs::create_dir_all(&config_dir).with_context(|| {
                format!(
                    "Failed to create config directory: {}",
                    config_dir.display()
                )
            })?;
        }

        Ok(Self { config_dir })
    }

    pub fn load_config(&self) -> Result<Config> {
        self.load_config_with_env(None)
    }

    pub fn load_config_with_env(&self, env: Option<&HashMap<String, String>>) -> Result<Config> {
        let config_path = self.config_dir.join("config.json");

        let default_registry = || {
            env.and_then(|vars| vars.get("CODEMOD_REGISTRY_URL"))
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string())
                .unwrap_or_else(|| RegistryConfig::default().default_registry)
        };
        if !config_path.exists() {
            return Ok(Config::with_default_registry(default_registry()));
        }

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

        let mut config: Config =
            serde_json::from_str(&content).context("Failed to parse config file")?;
        if config.default_registry.trim().is_empty() {
            // Only preferences were saved: the registry settings are the
            // defaults, as if there were no file.
            let defaults = Config::with_default_registry(default_registry());
            config.default_registry = defaults.default_registry;
            config.registries.extend(defaults.registries);
        }

        Ok(config)
    }

    pub fn save_config(&self, config: &Config) -> Result<()> {
        let config_path = self.config_dir.join("config.json");
        let content =
            serde_json::to_string_pretty(config).context("Failed to serialize config file")?;

        fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

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
            .with_context(|| format!("Failed to read auth file: {}", auth_path.display()))?;

        let auth: StoredAuth =
            serde_json::from_str(&content).context("Failed to parse auth file")?;

        Ok(Some(auth))
    }

    pub fn save_auth(&self, auth: &StoredAuth) -> Result<()> {
        let auth_path = self.get_auth_path(&auth.registry);

        // Create auth directory if it doesn't exist
        if let Some(parent) = auth_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create auth directory: {}", parent.display())
            })?;
        }

        let content =
            serde_json::to_string_pretty(auth).context("Failed to serialize auth data")?;

        write_auth_file(&auth_path, content.as_bytes())
            .with_context(|| format!("Failed to write auth file: {}", auth_path.display()))?;

        Ok(())
    }

    pub fn remove_auth(&self, registry: &str) -> Result<()> {
        let auth_path = self.get_auth_path(registry);

        if auth_path.exists() {
            fs::remove_file(&auth_path)
                .with_context(|| format!("Failed to remove auth file: {}", auth_path.display()))?;
        }

        Ok(())
    }

    pub fn clear_cache(&self) -> Result<()> {
        let cache_dir = self.config_dir.join("cache");

        if cache_dir.exists() {
            fs::remove_dir_all(&cache_dir).with_context(|| {
                format!("Failed to remove cache directory: {}", cache_dir.display())
            })?;
        }

        Ok(())
    }

    pub fn get_auth_for_registry(&self, registry: &str) -> Result<Option<StoredAuth>> {
        self.load_auth(registry)
    }

    pub fn get_or_create_anonymous_telemetry_id(&self) -> Result<String> {
        let telemetry_id_path = self.config_dir.join("telemetry_id");
        match fs::read_to_string(&telemetry_id_path) {
            Ok(stored_id) => {
                let stored_id = stored_id.trim();
                if !stored_id.is_empty() {
                    return Ok(stored_id.to_string());
                }
                anyhow::bail!(
                    "Stored telemetry id is empty: {}",
                    telemetry_id_path.display()
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to read telemetry id: {}",
                        telemetry_id_path.display()
                    )
                });
            }
        }

        let telemetry_id = uuid::Uuid::new_v4().to_string();
        let mut temporary_id = tempfile::NamedTempFile::new_in(&self.config_dir)
            .context("Failed to create temporary telemetry id file")?;
        temporary_id
            .write_all(telemetry_id.as_bytes())
            .context("Failed to write temporary telemetry id")?;
        temporary_id
            .as_file()
            .sync_all()
            .context("Failed to persist temporary telemetry id")?;

        match temporary_id.persist_noclobber(&telemetry_id_path) {
            Ok(_) => Ok(telemetry_id),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stored_id = fs::read_to_string(&telemetry_id_path).with_context(|| {
                    format!(
                        "Failed to read concurrently created telemetry id: {}",
                        telemetry_id_path.display()
                    )
                })?;
                let stored_id = stored_id.trim();
                if stored_id.is_empty() {
                    anyhow::bail!(
                        "Concurrently created telemetry id is empty: {}",
                        telemetry_id_path.display()
                    );
                }
                Ok(stored_id.to_string())
            }
            Err(error) => Err(error.error).with_context(|| {
                format!(
                    "Failed to persist telemetry id: {}",
                    telemetry_id_path.display()
                )
            }),
        }
    }

    /// Email last delivered to PostHog as a person `$set` for `distinct_id`.
    /// Stored in plain text next to the auth file (which already holds the
    /// email). Missing or unreadable cache files are treated as "not sent yet".
    pub fn load_last_sent_person_email(&self, distinct_id: &str) -> Result<Option<String>> {
        let path = self.person_traits_path(distinct_id);
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let email = contents.trim();
                if email.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(email.to_string()))
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                log::debug!("Ignoring unreadable person traits file {path:?}: {error}");
                Ok(None)
            }
        }
    }

    pub fn save_last_sent_person_email(&self, distinct_id: &str, email: &str) -> Result<()> {
        let dir = self.person_traits_dir();
        fs::create_dir_all(&dir).with_context(|| {
            format!(
                "Failed to create person traits directory: {}",
                dir.display()
            )
        })?;
        let path = self.person_traits_path(distinct_id);
        fs::write(&path, email)
            .with_context(|| format!("Failed to write person traits file: {}", path.display()))?;
        Ok(())
    }

    fn person_traits_dir(&self) -> PathBuf {
        self.config_dir.join("person_traits")
    }

    fn person_traits_path(&self, distinct_id: &str) -> PathBuf {
        self.person_traits_dir()
            .join(Self::sanitize_registry_name(distinct_id))
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

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;

    if !file.metadata()?.file_type().is_file() {
        anyhow::bail!(
            "Auth path exists but is not a regular file: {}",
            path.display()
        );
    }

    file.set_len(0)?;
    file.write_all(content)?;
    file.flush()?;
    file.set_permissions(restricted_permissions)?;

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
    use std::collections::HashSet;
    use std::fs;
    use std::sync::{Arc, Barrier};

    #[test]
    fn empty_telemetry_id_displays_its_path_without_debug_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let storage = TokenStorage {
            config_dir: dir.path().to_path_buf(),
        };
        let path = dir.path().join("telemetry_id");
        fs::write(&path, "").unwrap();

        let error = storage.get_or_create_anonymous_telemetry_id().unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Stored telemetry id is empty: {}", path.display())
        );
    }

    #[test]
    fn the_decision_model_agent_and_unknown_keys_survive_a_save() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        // As another tool may write it: without the registry settings.
        fs::write(
            temp_dir.path().join("config.json"),
            r#"{ "decisionModel": { "provider": "openai", "model": "gpt-6-luna" }, "agent": "claude-code", "launcher": { "lastRun": 1 } }"#,
        )
        .expect("expected config write");
        let storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");
        let config = storage.load_config().expect("config");
        assert!(!config.default_registry.is_empty());
        storage.enable_anonymous_feedback().expect("save");
        let path = temp_dir.path().join("config.json");
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["decisionModel"]["model"], "gpt-6-luna");
        assert_eq!(saved["agent"], "claude-code");
        assert_eq!(saved["launcher"]["lastRun"], 1);
        assert_eq!(saved["anonymous_feedback"]["enabled"], true);
        // Both readers of the preferences still see them after the save.
        let settings = crate::user_settings::SettingsFile::at(path.clone()).load();
        assert_eq!(settings.agent.as_deref(), Some("claude-code"));
        assert_eq!(
            settings.decision_model.map(|model| model.id()).as_deref(),
            Some("openai/gpt-6-luna")
        );
        let preference = codemod_workflow::assess::config::user_preference(&path).unwrap();
        assert_eq!(preference.provider.as_deref(), Some("openai"));
        assert_eq!(preference.model.as_deref(), Some("gpt-6-luna"));
    }

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
    fn a_file_with_only_run_preferences_reads_as_defaults_and_keeps_them() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        fs::write(
            temp_dir.path().join("config.json"),
            r#"{ "agent": "codex", "decisionModel": { "provider": "systemone", "model": "jev-latest" } }"#,
        )
        .expect("expected config write");
        let storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");

        let config = storage.load_config().expect("config");
        assert_eq!(config.default_registry, Config::default().default_registry);
        assert!(config.registries.contains_key(&config.default_registry));

        storage.save_config(&config).expect("save");
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(temp_dir.path().join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["agent"], "codex");
        assert_eq!(saved["decisionModel"]["model"], "jev-latest");
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

    #[test]
    fn anonymous_telemetry_id_is_stable_across_cli_invocations() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let first_storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");
        let first_id = first_storage
            .get_or_create_anonymous_telemetry_id()
            .expect("expected telemetry id");

        let second_storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");
        let second_id = second_storage
            .get_or_create_anonymous_telemetry_id()
            .expect("expected persisted telemetry id");

        assert_eq!(first_id, second_id);
        assert!(uuid::Uuid::parse_str(&first_id).is_ok());
    }

    #[test]
    fn concurrent_anonymous_telemetry_id_initialization_uses_one_id() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let config_dir = Arc::new(temp_dir.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let config_dir = Arc::clone(&config_dir);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let storage = TokenStorage::with_config_dir(config_dir.as_ref().clone())
                        .expect("storage");
                    barrier.wait();
                    storage
                        .get_or_create_anonymous_telemetry_id()
                        .expect("expected telemetry id")
                })
            })
            .collect::<Vec<_>>();

        let ids = handles
            .into_iter()
            .map(|handle| handle.join().expect("telemetry id thread"))
            .collect::<HashSet<_>>();

        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn last_sent_person_email_round_trips_per_user() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let storage =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");

        assert_eq!(
            storage
                .load_last_sent_person_email("user-a")
                .expect("missing file is not an error"),
            None
        );

        storage
            .save_last_sent_person_email("user-a", "a@example.com")
            .expect("save user a");
        storage
            .save_last_sent_person_email("user-b", "b@example.com")
            .expect("save user b");
        storage
            .save_last_sent_person_email("user-a", "a2@example.com")
            .expect("overwrite user a");

        let reloaded =
            TokenStorage::with_config_dir(temp_dir.path().to_path_buf()).expect("storage");
        assert_eq!(
            reloaded.load_last_sent_person_email("user-a").unwrap(),
            Some("a2@example.com".to_string())
        );
        assert_eq!(
            reloaded.load_last_sent_person_email("user-b").unwrap(),
            Some("b@example.com".to_string())
        );

        let path = temp_dir.path().join("person_traits").join("user-a");
        fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
        assert_eq!(storage.load_last_sent_person_email("user-a").unwrap(), None);
        storage
            .save_last_sent_person_email("user-a", "healed@example.com")
            .expect("overwrite corrupt file");
        assert_eq!(
            storage.load_last_sent_person_email("user-a").unwrap(),
            Some("healed@example.com".to_string())
        );
    }

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

    #[cfg(unix)]
    #[test]
    fn save_auth_rejects_symlink_auth_path_without_touching_target() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let storage = TokenStorage::with_config_dir(temp_dir.path().join("codemod")).unwrap();
        let auth = stored_auth("https://app.codemod.com");
        let auth_path = storage.get_auth_path("https://app.codemod.com");
        fs::create_dir_all(auth_path.parent().unwrap()).unwrap();
        let symlink_target = temp_dir.path().join("target.json");
        fs::write(&symlink_target, "do not change").unwrap();
        symlink(&symlink_target, &auth_path).unwrap();

        let result = storage.save_auth(&auth);

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(symlink_target).unwrap(), "do not change");
        assert!(fs::symlink_metadata(auth_path)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
