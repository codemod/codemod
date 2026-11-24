//! Which decision model answers `assess()`, and with which key.
//!
//! The provider and model come from, first to last:
//!
//! 1. the call: `assess(file, { provider, model })`;
//! 2. the workflow: `decision_model: { provider, model }` in its package's
//!    `codemod.yaml`;
//! 3. the user config: `"decisionModel": { "provider", "model" }` in
//!    `config.json` in the codemod config directory ([`user_config_path`]),
//!    the file the CLI keeps its settings in;
//! 4. the environment: `DECISION_PROVIDER` and `DECISION_MODEL`
//!    (`TYPESAFE_DEFAULT_MODEL` for System One);
//! 5. the default: System One with `jev-latest`.
//!
//! A level that names a provider only lends its model to that provider, so a
//! call that switches provider does not take another provider's model.
//!
//! The key, base URL and the rest come from the environment only:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `TYPESAFE_API_KEY` / `OPENAI_API_KEY` | the provider's key |
//! | `DECISION_API_KEY` | a key for whichever provider `DECISION_PROVIDER` names (or any, when it names none) |
//! | `TYPESAFE_BASE_URL` / `OPENAI_BASE_URL` | the provider's API root |
//! | `DECISION_BASE_URL`, `DECISION_PATH` | API root and request path, as `DECISION_API_KEY` applies |
//! | `DECISION_AUTH_HEADER` | `authorization` (default, `Bearer <key>`) or a header carrying the raw key (`api-key` on Azure) |
//! | `DECISION_CONCURRENCY` | requests in flight at once (default 4) |

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The environment, as a map: blank values count as unset.
pub type Env = HashMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    SystemOne,
    OpenAi,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::SystemOne, Provider::OpenAi];

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "systemone" | "system-one" | "typesafe" => Ok(Self::SystemOne),
            "openai" => Ok(Self::OpenAi),
            _ => Err(format!(
                "unknown decision model provider '{name}': use \"systemone\" or \"openai\""
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemOne => "systemone",
            Self::OpenAi => "openai",
        }
    }

    /// As people know it.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::SystemOne => "System One",
            Self::OpenAi => "OpenAI Decisions",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::SystemOne => "jev-latest",
            Self::OpenAi => "gpt-6-luna",
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::SystemOne => "https://api.typesafe.ai",
            Self::OpenAi => "https://api.openai.com",
        }
    }

    fn default_path(self) -> &'static str {
        match self {
            Self::SystemOne => "/v1/systemone",
            Self::OpenAi => "/v1/decisions",
        }
    }

    /// The variable that holds this provider's key.
    pub fn key_var(self) -> &'static str {
        match self {
            Self::SystemOne => "TYPESAFE_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
        }
    }

    /// The variable that holds this provider's API root.
    pub fn base_url_var(self) -> &'static str {
        match self {
            Self::SystemOne => "TYPESAFE_BASE_URL",
            Self::OpenAi => "OPENAI_BASE_URL",
        }
    }
}

/// A provider and model chosen at one level.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPreference {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl ModelPreference {
    fn provider(&self) -> Result<Option<Provider>, String> {
        self.provider
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(Provider::parse)
            .transpose()
    }

    fn model(&self) -> Option<&str> {
        self.model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
    }
}

/// The levels below the call: the workflow's and the user's.
#[derive(Debug, Clone, Default)]
pub struct Layers {
    pub workflow: ModelPreference,
    pub user: ModelPreference,
}

impl Layers {
    /// The workflow's choice (from the `codemod.yaml` of the package the
    /// workflow file is in) and the user's (from [`user_config_path`]). A
    /// file that cannot be read is an error naming it; a missing one chooses
    /// nothing.
    pub fn for_workflow(workflow: &Path) -> Result<Self, String> {
        Self::load(
            &crate::run::package_dir(workflow),
            user_config_path().as_deref(),
        )
    }

    /// As [`Layers::for_workflow`], given the package's directory and the
    /// user config file.
    pub fn load(package_dir: &Path, user_config: Option<&Path>) -> Result<Self, String> {
        Ok(Self {
            workflow: workflow_preference(package_dir)?,
            user: match user_config {
                Some(path) => user_preference(path)?,
                None => ModelPreference::default(),
            },
        })
    }
}

/// Everything needed to call a provider.
#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    pub provider: Provider,
    pub model: String,
    /// The API root joined with the request path.
    pub url: String,
    pub api_key: Option<String>,
    /// Lower case; `authorization` sends `Bearer <key>`.
    pub auth_header: String,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key.
        f.debug_struct("Settings")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("url", &self.url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .field("auth_header", &self.auth_header)
            .finish()
    }
}

impl Settings {
    /// Where requests go, for messages: the host of the URL.
    pub fn host(&self) -> String {
        reqwest::Url::parse(&self.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| self.url.clone())
    }

    /// The variables a key can be set in, as a message names them.
    pub fn key_vars(&self) -> String {
        format!("{} (or DECISION_API_KEY)", self.provider.key_var())
    }
}

fn read<'a>(env: &'a Env, name: &str) -> Option<&'a str> {
    env.get(name).map(|v| v.trim()).filter(|v| !v.is_empty())
}

/// The provider, model, endpoint and key for a call that names `call`.
pub fn resolve(call: &ModelPreference, layers: &Layers, env: &Env) -> Result<Settings, String> {
    let env_layer = ModelPreference {
        provider: read(env, "DECISION_PROVIDER").map(str::to_string),
        model: read(env, "DECISION_MODEL").map(str::to_string),
    };
    let levels = [call, &layers.workflow, &layers.user, &env_layer];
    let mut provider = None;
    for level in levels {
        if let Some(found) = level.provider()? {
            provider = Some(found);
            break;
        }
    }
    let provider = provider.unwrap_or(Provider::SystemOne);
    let mut model = None;
    for level in levels {
        let applies = level.provider()?.is_none_or(|p| p == provider);
        if let (true, Some(found)) = (applies, level.model()) {
            model = Some(found.to_string());
            break;
        }
    }
    if model.is_none() && provider == Provider::SystemOne {
        model = read(env, "TYPESAFE_DEFAULT_MODEL").map(str::to_string);
    }
    let model = model.unwrap_or_else(|| provider.default_model().to_string());

    // The `DECISION_*` endpoint and key belong to the provider
    // `DECISION_PROVIDER` names, or to any when it names none.
    let env_provider = env_layer.provider()?;
    let generic = |name: &str| {
        if env_provider.is_none_or(|p| p == provider) {
            read(env, name)
        } else {
            None
        }
    };
    let base_url = read(env, provider.base_url_var())
        .or_else(|| generic("DECISION_BASE_URL"))
        .unwrap_or(provider.default_base_url());
    let base = reqwest::Url::parse(base_url)
        .map_err(|_| format!("the decision model's base URL '{base_url}' is not a URL"))?;
    if base.scheme() != "https" && base.scheme() != "http" {
        return Err(format!(
            "the decision model's base URL '{base_url}' is not http(s)"
        ));
    }
    if base.query().is_some()
        || base.fragment().is_some()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        return Err(format!(
            "the decision model's base URL '{base_url}' must not carry a query, fragment or user"
        ));
    }
    let path = generic("DECISION_PATH").unwrap_or(provider.default_path());
    Ok(Settings {
        provider,
        model,
        url: format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        ),
        api_key: read(env, provider.key_var())
            .or_else(|| generic("DECISION_API_KEY"))
            .map(str::to_string),
        auth_header: generic("DECISION_AUTH_HEADER")
            .unwrap_or("authorization")
            .to_ascii_lowercase(),
    })
}

/// Requests in flight at once: `DECISION_CONCURRENCY`, else 4.
pub fn concurrency(env: &Env) -> Result<usize, String> {
    match read(env, "DECISION_CONCURRENCY") {
        None => Ok(4),
        Some(text) => match text.parse::<usize>() {
            Ok(n) if (1..=64).contains(&n) => Ok(n),
            _ => Err(format!(
                "DECISION_CONCURRENCY must be a whole number from 1 to 64, not '{text}'"
            )),
        },
    }
}

/// The user config file the CLI keeps its settings in, shared with other
/// tools that set the decision model or the coding agent:
/// `<config dir>/codemod/config.json` (`~/.config/codemod` on Linux,
/// `~/Library/Application Support/codemod` on macOS, `%APPDATA%\codemod` on
/// Windows).
pub fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("codemod").join("config.json"))
}

/// The `decisionModel` of a user config file; nothing when the file or the
/// key is missing.
pub fn user_preference(path: &Path) -> Result<ModelPreference, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ModelPreference::default())
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let config: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    match config.get("decisionModel") {
        None | Some(serde_json::Value::Null) => Ok(ModelPreference::default()),
        Some(value) => serde_json::from_value(value.clone()).map_err(|e| {
            format!(
                "{}: decisionModel must be {{ \"provider\", \"model\" }}: {e}",
                path.display()
            )
        }),
    }
}

/// The `decision_model` of the `codemod.yaml` in `package_dir`.
pub fn workflow_preference(package_dir: &Path) -> Result<ModelPreference, String> {
    let path = package_dir.join("codemod.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(ModelPreference::default());
    };
    let manifest: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    match manifest.get("decision_model") {
        None | Some(serde_yaml::Value::Null) => Ok(ModelPreference::default()),
        Some(value) => serde_yaml::from_value(value.clone()).map_err(|e| {
            format!(
                "{}: decision_model must be {{ provider, model }}: {e}",
                path.display()
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn pref(provider: Option<&str>, model: Option<&str>) -> ModelPreference {
        ModelPreference {
            provider: provider.map(str::to_string),
            model: model.map(str::to_string),
        }
    }

    #[test]
    fn system_one_with_jev_latest_is_the_default() {
        let settings = resolve(&pref(None, None), &Layers::default(), &env(&[])).unwrap();
        assert_eq!(settings.provider, Provider::SystemOne);
        assert_eq!(settings.model, "jev-latest");
        assert_eq!(settings.url, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(settings.api_key, None);
        assert_eq!(settings.auth_header, "authorization");
    }

    #[test]
    fn the_call_then_the_workflow_then_the_user_then_the_environment_choose() {
        let layers = Layers {
            workflow: pref(None, Some("jev-workflow")),
            user: pref(Some("openai"), Some("gpt-user")),
        };
        let environment = env(&[
            ("DECISION_PROVIDER", "systemone"),
            ("DECISION_MODEL", "jev-env"),
        ]);
        // The user's provider outranks the environment's; the workflow's
        // model names no provider, so it applies to OpenAI too.
        let settings = resolve(&pref(None, None), &layers, &environment).unwrap();
        assert_eq!(settings.provider, Provider::OpenAi);
        assert_eq!(settings.model, "jev-workflow");
        // A call that names System One does not take the user's OpenAI model.
        let layers = Layers {
            workflow: ModelPreference::default(),
            user: pref(Some("openai"), Some("gpt-user")),
        };
        let settings = resolve(&pref(Some("systemone"), None), &layers, &environment).unwrap();
        assert_eq!(settings.provider, Provider::SystemOne);
        assert_eq!(settings.model, "jev-env");
        let settings = resolve(&pref(None, Some("jev-call")), &layers, &environment).unwrap();
        assert_eq!(settings.model, "jev-call");
    }

    #[test]
    fn keys_and_endpoints_come_from_the_environment() {
        let environment = env(&[
            ("TYPESAFE_API_KEY", "ts-key"),
            ("OPENAI_API_KEY", " "),
            ("DECISION_PROVIDER", "openai"),
            ("DECISION_API_KEY", "azure-key"),
            ("DECISION_BASE_URL", "https://example.openai.azure.com/"),
            ("DECISION_PATH", "openai/v1/decisions"),
            ("DECISION_AUTH_HEADER", "Api-Key"),
        ]);
        let openai = resolve(&pref(None, None), &Layers::default(), &environment).unwrap();
        assert_eq!(openai.provider, Provider::OpenAi);
        assert_eq!(openai.api_key.as_deref(), Some("azure-key"));
        assert_eq!(
            openai.url,
            "https://example.openai.azure.com/openai/v1/decisions"
        );
        assert_eq!(openai.auth_header, "api-key");
        // The DECISION_* settings are OpenAI's here, not System One's.
        let systemone = resolve(
            &pref(Some("systemone"), None),
            &Layers::default(),
            &environment,
        )
        .unwrap();
        assert_eq!(systemone.api_key.as_deref(), Some("ts-key"));
        assert_eq!(systemone.url, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(systemone.auth_header, "authorization");
        assert!(!format!("{systemone:?}").contains("ts-key"));
    }

    #[test]
    fn bad_settings_are_errors() {
        let error =
            resolve(&pref(Some("gemini"), None), &Layers::default(), &env(&[])).unwrap_err();
        assert!(
            error.contains("unknown decision model provider 'gemini'"),
            "{error}"
        );
        let error = resolve(
            &pref(None, None),
            &Layers::default(),
            &env(&[("TYPESAFE_BASE_URL", "ftp://x")]),
        )
        .unwrap_err();
        assert!(error.contains("not http(s)"), "{error}");
        assert!(concurrency(&env(&[("DECISION_CONCURRENCY", "0")])).is_err());
        assert_eq!(concurrency(&env(&[])).unwrap(), 4);
    }

    #[test]
    fn the_user_config_and_codemod_yaml_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        assert_eq!(
            user_preference(&config).unwrap(),
            ModelPreference::default()
        );
        std::fs::write(
            &config,
            r#"{ "default_registry": "x", "agent": "claude-code", "decisionModel": { "provider": "openai", "model": "gpt-6-luna" } }"#,
        )
        .unwrap();
        assert_eq!(
            user_preference(&config).unwrap(),
            pref(Some("openai"), Some("gpt-6-luna"))
        );
        std::fs::write(
            dir.path().join("codemod.yaml"),
            "name: x\ndecision_model:\n  model: jev-1.13.0\n",
        )
        .unwrap();
        assert_eq!(
            workflow_preference(dir.path()).unwrap(),
            pref(None, Some("jev-1.13.0"))
        );
        std::fs::write(&config, r#"{ "decisionModel": { "provider": 1 } }"#).unwrap();
        assert!(user_preference(&config)
            .unwrap_err()
            .contains("decisionModel must be"));
    }
}
