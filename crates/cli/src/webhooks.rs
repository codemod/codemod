use anyhow::{anyhow, Result};
use log::{debug, warn};
use reqwest;
use serde::Serialize;
use std::fs;
use std::path::Path;

// Default Make.com webhook URL (public endpoint, not a secret)
const DEFAULT_MAKE_WEBHOOK_URL: &str = "https://hook.us1.make.com/x57ucqxc7k08kmq1948nlozpf57nr9p5";
const MAKE_WEBHOOK_URL_ENV: &str = "CODEMOD_MAKE_WEBHOOK_URL";

#[derive(Serialize, Debug)]
#[serde(rename_all = "snake_case")]
enum WebhookEventType {
    Publish,
    Unpublish,
}

#[derive(Serialize, Debug)]
struct MakeWebhookPayload {
    event_type: WebhookEventType,
    package_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_versions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    codemod_yaml: Option<String>,
    username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    github_url: Option<String>,
}

/// Send a webhook event to Make.com
async fn send_webhook(payload: MakeWebhookPayload) -> Result<()> {
    // Get webhook URL from environment variable or use default
    let webhook_url = std::env::var(MAKE_WEBHOOK_URL_ENV)
        .unwrap_or_else(|_| DEFAULT_MAKE_WEBHOOK_URL.to_string());

    debug!("Sending webhook to Make.com: event_type={:?}, package={}", payload.event_type, payload.package_name);
    debug!("Webhook URL: {}", webhook_url);

    // Send the webhook request
    let client = reqwest::Client::new();
    let response = client
        .post(&webhook_url)
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            if resp.status().is_success() {
                debug!("Successfully sent webhook to Make.com");
            } else {
                let status = resp.status();
                let error_text = resp.text().await.unwrap_or_default();
                warn!(
                    "Make.com webhook returned non-success status {}: {}",
                    status, error_text
                );
            }
        }
        Err(e) => {
            warn!("Failed to send webhook to Make.com: {}", e);
        }
    }

    Ok(())
}

/// Send a webhook event to Make.com when a package is successfully published
pub async fn send_publish_webhook(
    package_path: &Path,
    package_name: &str,
    package_version: &str,
    username: &str,
    github_url: Option<&str>,
) -> Result<()> {
    // Read the codemod.yaml file
    let manifest_path = package_path.join("codemod.yaml");
    let codemod_yaml = fs::read_to_string(&manifest_path)
        .map_err(|e| anyhow!("Failed to read codemod.yaml: {}", e))?;

    // Prepare the payload
    let payload = MakeWebhookPayload {
        event_type: WebhookEventType::Publish,
        package_name: package_name.to_string(),
        package_version: Some(package_version.to_string()),
        package_versions: None,
        codemod_yaml: Some(codemod_yaml),
        username: username.to_string(),
        github_url: github_url.map(|s| s.to_string()),
    };

    send_webhook(payload).await
}

/// Send a webhook event to Make.com when a package is successfully unpublished
pub async fn send_unpublish_webhook(
    package_name: &str,
    versions: &[String],
    username: &str,
    github_url: Option<&str>,
) -> Result<()> {
    // Prepare the payload
    // Note: versions should never be empty in practice (unpublish API always returns at least one version)
    let payload = MakeWebhookPayload {
        event_type: WebhookEventType::Unpublish,
        package_name: package_name.to_string(),
        package_version: match versions.len() {
            1 => Some(versions[0].clone()),
            _ => None,
        },
        package_versions: match versions.len() {
            n if n > 1 => Some(versions.to_vec()),
            _ => None,
        },
        codemod_yaml: None,
        username: username.to_string(),
        github_url: github_url.map(|s| s.to_string()),
    };

    send_webhook(payload).await
}

