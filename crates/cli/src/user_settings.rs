//! The person's run preferences, kept in the CLI's `config.json` next to the
//! registry settings (`<config dir>/codemod/config.json`):
//!
//! ```json
//! { "agent": "claude-code", "decisionModel": { "provider": "systemone", "model": "jev-latest" } }
//! ```
//!
//! `agent` is the coding agent `agent(...)` steps use; `decisionModel` is the
//! System One model decision and assessment calls use. Flags win over these
//! (`--agent`, `--model`), and these win over the defaults. Saving keeps
//! every other key in the file as it was.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The decision-model provider used when none is chosen.
pub const DEFAULT_DECISION_PROVIDER: &str = "systemone";
/// The model used when none is chosen: System One's own default alias.
pub const DEFAULT_DECISION_MODEL: &str = "jev-latest";

const AGENT_KEY: &str = "agent";
const DECISION_MODEL_KEY: &str = "decisionModel";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionModel {
    pub provider: String,
    pub model: String,
}

impl Default for DecisionModel {
    fn default() -> Self {
        Self {
            provider: DEFAULT_DECISION_PROVIDER.to_string(),
            model: DEFAULT_DECISION_MODEL.to_string(),
        }
    }
}

impl DecisionModel {
    /// `provider/model`, or a bare `model` of the default provider.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let (provider, model) = match text.split_once('/') {
            Some((provider, model)) => (provider.trim(), model.trim()),
            None => (DEFAULT_DECISION_PROVIDER, text),
        };
        // The names `codemod/assess` accepts for System One.
        let provider = match provider.to_ascii_lowercase().as_str() {
            "system-one" | "typesafe" => DEFAULT_DECISION_PROVIDER.to_string(),
            other => other.to_string(),
        };
        if provider.is_empty() || model.is_empty() {
            return Err(anyhow!(
                "invalid model '{text}': use <provider>/<model>, e.g. {DEFAULT_DECISION_PROVIDER}/{DEFAULT_DECISION_MODEL}"
            ));
        }
        Ok(Self {
            provider,
            model: model.to_string(),
        })
    }

    /// `provider/model`.
    pub fn id(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// What the config file says; anything missing is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserSettings {
    pub agent: Option<String>,
    pub decision_model: Option<DecisionModel>,
}

impl UserSettings {
    /// The coding agent to use: the flag's, else the saved one.
    pub fn agent(&self, flag: Option<&str>) -> Option<String> {
        flag.or(self.agent.as_deref())
            .map(str::trim)
            .filter(|agent| !agent.is_empty())
            .map(str::to_string)
    }

    /// The decision model to use: the flag's, else the saved one, else the
    /// default.
    pub fn decision_model(&self, flag: Option<&DecisionModel>) -> DecisionModel {
        flag.or(self.decision_model.as_ref())
            .cloned()
            .unwrap_or_default()
    }
}

/// The config file the settings live in.
pub struct SettingsFile {
    path: PathBuf,
}

impl SettingsFile {
    /// The CLI's own config file, the one `codemod login` uses too.
    pub fn default_location() -> Option<Self> {
        dirs::config_dir().map(|dir| Self::at(dir.join("codemod").join("config.json")))
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    #[cfg(test)]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The saved settings. A missing or unreadable file, or a value of the
    /// wrong shape, reads as nothing saved: preferences never stop a run.
    pub fn load(&self) -> UserSettings {
        let Some(Value::Object(object)) = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            return UserSettings::default();
        };
        UserSettings {
            agent: object
                .get(AGENT_KEY)
                .and_then(Value::as_str)
                .map(str::to_string),
            decision_model: object
                .get(DECISION_MODEL_KEY)
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok()),
        }
    }

    /// Save the settings, keeping every other key in the file. `None` removes
    /// its key. A file without registry settings (nobody logged in yet) gets
    /// the defaults written too, as `codemod login` would: earlier CLI
    /// versions (1.x, and 2.0 before alpha.7) cannot read the file without
    /// them, and fail every command that loads it.
    pub fn save(&self, settings: &UserSettings) -> Result<()> {
        let mut object = match std::fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(object)) => object,
                // Never overwrite a file we cannot read back.
                _ => {
                    return Err(anyhow!(
                        "{} is not a JSON object; not saving preferences",
                        self.path.display()
                    ))
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read {}", self.path.display()))
            }
        };
        match &settings.agent {
            Some(agent) => object.insert(AGENT_KEY.into(), Value::String(agent.clone())),
            None => object.remove(AGENT_KEY),
        };
        match &settings.decision_model {
            Some(model) => object.insert(DECISION_MODEL_KEY.into(), serde_json::to_value(model)?),
            None => object.remove(DECISION_MODEL_KEY),
        };
        let has_registry = object
            .get("default_registry")
            .and_then(Value::as_str)
            .is_some_and(|registry| !registry.trim().is_empty());
        if !has_registry {
            if let Value::Object(defaults) =
                serde_json::to_value(crate::auth::storage::Config::default())?
            {
                for key in ["default_registry", "registries"] {
                    if let Some(value) = defaults.get(key) {
                        object.insert(key.into(), value.clone());
                    }
                }
            }
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&Value::Object(object))?;
        std::fs::write(&self.path, text)
            .with_context(|| format!("Failed to write {}", self.path.display()))
    }
}

/// The saved settings, or none when the config directory is unknown.
pub fn load() -> UserSettings {
    SettingsFile::default_location()
        .map(|file| file.load())
        .unwrap_or_default()
}

// ------------------------------------------------------------- models ---

/// Models a provider is known to serve, for when its list endpoint cannot
/// be reached.
///
/// The providers and their defaults are `codemod/assess`'s.
pub fn known_models(provider: &str) -> Vec<String> {
    match provider {
        DEFAULT_DECISION_PROVIDER => vec!["jev-latest".into(), "jev-preview".into()],
        "openai" => vec!["gpt-6-luna".into()],
        _ => Vec::new(),
    }
}

/// Where System One's API is: `TYPESAFE_BASE_URL` (as `codemod/assess`
/// reads it), else TypeSafe's.
pub fn systemone_base_url() -> String {
    std::env::var("TYPESAFE_BASE_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| "https://api.typesafe.ai".to_string())
}

/// The model names in a `GET /v1/models` answer: OpenAI's shape
/// (`{ "data": [{ "id" }] }`) or System One's (`{ "models": [{ "name" }] }`).
pub fn parse_model_list(body: &Value) -> Vec<String> {
    let entries = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array);
    let mut names: Vec<String> = entries
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("id")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    names.dedup();
    names
}

/// The models a provider lists, asking its API; `None` when it cannot say.
pub async fn fetch_models(provider: &str) -> Option<Vec<String>> {
    if provider != DEFAULT_DECISION_PROVIDER {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;
    let mut request = client.get(format!("{}/v1/models", systemone_base_url()));
    if let Some(key) = ["TYPESAFE_API_KEY", "DECISION_API_KEY"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|key| !key.is_empty()))
    {
        request = request.bearer_auth(key);
    }
    let body: Value = request
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    Some(parse_model_list(&body)).filter(|models| !models.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file_in(dir: &tempfile::TempDir) -> SettingsFile {
        SettingsFile::at(dir.path().join("codemod").join("config.json"))
    }

    #[test]
    fn a_missing_file_reads_as_nothing_saved() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(file_in(&dir).load(), UserSettings::default());
    }

    #[test]
    fn saving_keeps_the_other_keys_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_in(&dir);
        std::fs::create_dir_all(file.path().parent().unwrap()).unwrap();
        std::fs::write(
            file.path(),
            r#"{"default_registry":"https://app.codemod.com","registries":{}}"#,
        )
        .unwrap();
        let settings = UserSettings {
            agent: Some("codex".into()),
            decision_model: Some(DecisionModel::parse("systemone/jev-preview").unwrap()),
        };
        file.save(&settings).unwrap();

        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
        assert_eq!(
            saved,
            json!({
                "default_registry": "https://app.codemod.com",
                "registries": {},
                "agent": "codex",
                "decisionModel": { "provider": "systemone", "model": "jev-preview" }
            })
        );
        assert_eq!(file.load(), settings);
    }

    #[test]
    fn a_new_file_gets_the_registry_settings_older_versions_need() {
        // What 1.x and 2.0.0-alpha.6 require of config.json.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct EarlierConfig {
            default_registry: String,
            registries: std::collections::HashMap<String, Value>,
        }

        let dir = tempfile::tempdir().unwrap();
        let file = file_in(&dir);
        let settings = UserSettings {
            agent: None,
            decision_model: Some(DecisionModel::default()),
        };
        file.save(&settings).unwrap();

        let text = std::fs::read_to_string(file.path()).unwrap();
        let earlier: EarlierConfig = serde_json::from_str(&text).unwrap();
        let defaults = crate::auth::storage::Config::default();
        assert_eq!(earlier.default_registry, defaults.default_registry);
        assert!(earlier.registries.contains_key(&defaults.default_registry));
        assert_eq!(file.load(), settings);

        // Registry settings already there are left as they are.
        std::fs::write(
            file.path(),
            r#"{"default_registry":"https://registry.example","registries":{"https://registry.example":{}}}"#,
        )
        .unwrap();
        file.save(&settings).unwrap();
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
        assert_eq!(saved["default_registry"], "https://registry.example");
        assert_eq!(
            saved["registries"],
            json!({ "https://registry.example": {} })
        );
    }

    #[test]
    fn saving_none_removes_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_in(&dir);
        file.save(&UserSettings {
            agent: Some("codex".into()),
            decision_model: None,
        })
        .unwrap();
        file.save(&UserSettings::default()).unwrap();
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
        let mut keys: Vec<&String> = saved.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["default_registry", "registries"]);
    }

    #[test]
    fn a_file_that_is_not_an_object_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_in(&dir);
        std::fs::create_dir_all(file.path().parent().unwrap()).unwrap();
        std::fs::write(file.path(), "not json").unwrap();
        assert!(file.save(&UserSettings::default()).is_err());
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), "not json");
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_in(&dir);
        std::fs::create_dir_all(file.path().parent().unwrap()).unwrap();
        std::fs::write(file.path(), r#"{"agent": 3, "decisionModel": "jev"}"#).unwrap();
        assert_eq!(file.load(), UserSettings::default());
    }

    #[test]
    fn flags_win_over_saved_settings_which_win_over_defaults() {
        let saved = UserSettings {
            agent: Some("codex".into()),
            decision_model: Some(DecisionModel::parse("systemone/jev-preview").unwrap()),
        };
        assert_eq!(saved.agent(Some("claude")).as_deref(), Some("claude"));
        assert_eq!(saved.agent(None).as_deref(), Some("codex"));
        assert_eq!(UserSettings::default().agent(None), None);

        let flag = DecisionModel::parse("other/m1").unwrap();
        assert_eq!(saved.decision_model(Some(&flag)).id(), "other/m1");
        assert_eq!(saved.decision_model(None).id(), "systemone/jev-preview");
        assert_eq!(
            UserSettings::default().decision_model(None).id(),
            "systemone/jev-latest"
        );
    }

    #[test]
    fn a_bare_model_belongs_to_the_default_provider() {
        assert_eq!(
            DecisionModel::parse("jev-1.14.0").unwrap().id(),
            "systemone/jev-1.14.0"
        );
        assert_eq!(
            DecisionModel::parse("SystemOne/jev").unwrap().id(),
            "systemone/jev"
        );
        assert_eq!(
            DecisionModel::parse("typesafe/jev").unwrap().id(),
            "systemone/jev"
        );
        assert_eq!(
            DecisionModel::parse("openai/gpt-6-luna").unwrap().id(),
            "openai/gpt-6-luna"
        );
        assert!(DecisionModel::parse("systemone/").is_err());
        assert!(DecisionModel::parse("").is_err());
    }

    #[test]
    fn model_lists_in_either_shape_are_read() {
        assert_eq!(
            parse_model_list(&json!({ "data": [{ "id": "jev-latest" }, { "id": "jev-preview" }] })),
            vec!["jev-latest", "jev-preview"]
        );
        assert_eq!(
            parse_model_list(&json!({ "models": [{ "name": "clef", "displayName": "Clef" }] })),
            vec!["clef"]
        );
        assert!(parse_model_list(&json!({ "error": "nope" })).is_empty());
    }
}
