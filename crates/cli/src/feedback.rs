use crate::auth::TokenStorage;
use crate::CLI_VERSION;
use anyhow::{bail, Result};
use codemod_mcp::AnonymousFeedbackClient;
use std::collections::HashMap;

const FEEDBACK_ENDPOINT_PATH: &str = "/api/v1/ai/feedback";

pub fn feedback_disabled() -> bool {
    matches!(
        std::env::var("DISABLE_ANALYTICS").as_deref(),
        Ok("true") | Ok("1")
    )
}

pub fn feedback_endpoint(registry_url: &str) -> String {
    format!(
        "{}{}",
        registry_url.trim_end_matches('/'),
        FEEDBACK_ENDPOINT_PATH
    )
}

fn env_registry_url() -> Option<String> {
    std::env::var("CODEMOD_REGISTRY_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn persist_feedback_consent_if_requested(allow_feedback: bool) -> Result<()> {
    if allow_feedback {
        persist_feedback_consent()?;
    }
    Ok(())
}

pub fn persist_feedback_consent() -> Result<Option<String>> {
    let storage = TokenStorage::new()?;
    let config = storage.enable_anonymous_feedback()?;
    Ok(config.anonymous_feedback.consented_at)
}

pub fn anonymous_feedback_client(source: &'static str) -> Result<Option<AnonymousFeedbackClient>> {
    if feedback_disabled() {
        return Ok(None);
    }

    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    if !config.anonymous_feedback.enabled {
        return Ok(None);
    }
    let registry_url = env_registry_url().unwrap_or_else(|| config.default_registry.clone());

    Ok(AnonymousFeedbackClient::new(
        feedback_endpoint(&registry_url),
        source,
        CLI_VERSION.to_string(),
        config.anonymous_feedback.consented_at.clone(),
    ))
}

pub async fn send_anonymous_feedback_event(
    source: &'static str,
    event: &str,
    metadata: HashMap<String, String>,
) {
    let Ok(Some(client)) = anonymous_feedback_client(source) else {
        return;
    };

    client.submit(event, metadata).await;
}

pub async fn submit_anonymous_feedback(category: String, message: String) -> Result<()> {
    if feedback_disabled() {
        bail!("Anonymous feedback is disabled by DISABLE_ANALYTICS.");
    }

    let Some(client) = anonymous_feedback_client("cli-ai")? else {
        bail!("Anonymous feedback is not enabled. Run this command again after allowing feedback.");
    };

    client
        .submit_feedback("feedback", Some(category), Some(message), HashMap::new())
        .await
        .map_err(|error| anyhow::anyhow!("Failed to submit anonymous feedback: {error}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{env_registry_url, feedback_endpoint};

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                std::env::set_var(self.key, original);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn feedback_endpoint_uses_registry_api_path() {
        assert_eq!(
            feedback_endpoint("https://app.codemod.com/"),
            "https://app.codemod.com/api/v1/ai/feedback"
        );
    }

    #[test]
    fn env_registry_url_trims_and_ignores_empty_values() {
        let _registry_url_guard =
            EnvVarGuard::set("CODEMOD_REGISTRY_URL", " http://localhost:3000 ");
        assert_eq!(env_registry_url().as_deref(), Some("http://localhost:3000"));

        std::env::set_var("CODEMOD_REGISTRY_URL", " ");
        assert_eq!(env_registry_url(), None);
    }
}
