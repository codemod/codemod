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
    let env: HashMap<String, String> = std::env::vars().collect();
    let config = storage.load_config_with_env(Some(&env))?;
    if !config.anonymous_feedback.enabled {
        return Ok(None);
    }

    Ok(AnonymousFeedbackClient::new(
        feedback_endpoint(&config.default_registry),
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
    use super::feedback_endpoint;

    #[test]
    fn feedback_endpoint_uses_registry_api_path() {
        assert_eq!(
            feedback_endpoint("https://app.codemod.com/"),
            "https://app.codemod.com/api/v1/ai/feedback"
        );
    }
}
