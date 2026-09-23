use async_trait::async_trait;
use std::time::Duration;

use crate::send_event::{
    BaseEvent, MAX_CAPTURE_ATTEMPTS, PartialTelemetrySenderOptions, REQUEST_TIMEOUT_SECONDS,
    RETRY_INITIAL_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, TelemetryError, TelemetrySender,
    TelemetrySenderOptions, install_panic_hook,
};

/// Scarf gateway base URL, e.g. `https://codemod.gateway.scarf.sh/telemetry`.
/// Events are POSTed to `{base}/{cloud_role}/{kind}?<properties>`, matching
/// Scarf's own SDKs.
pub const SCARF_GATEWAY_URL: &str = env!("SCARF_GATEWAY_URL");

#[derive(Clone)]
pub struct ScarfSender {
    client: reqwest::Client,
    base_url: String,
    options: TelemetrySenderOptions,
}

impl ScarfSender {
    pub fn new(options: TelemetrySenderOptions) -> Result<Self, TelemetryError> {
        Self::new_with_base_url(options, SCARF_GATEWAY_URL)
    }

    pub fn new_with_base_url(
        options: TelemetrySenderOptions,
        base_url: &str,
    ) -> Result<Self, TelemetryError> {
        let base_url = base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            return Err(TelemetryError::Configuration(
                "SCARF_GATEWAY_URL is empty".to_string(),
            ));
        }

        // A bare host like `codemod.gateway.scarf.sh` is not a URL reqwest can
        // send to; assume HTTPS rather than failing on every event.
        let base_url = if base_url.contains("://") {
            base_url.to_string()
        } else {
            format!("https://{base_url}")
        };

        // Fail at construction instead of once per event.
        reqwest::Url::parse(&base_url).map_err(|error| {
            TelemetryError::Configuration(format!("SCARF_GATEWAY_URL: {error}"))
        })?;

        // Scarf's opt-out convention; the CLI-wide DISABLE_ANALYTICS switch is
        // handled by the caller before a sender is ever constructed.
        for variable in ["DO_NOT_TRACK", "SCARF_NO_ANALYTICS"] {
            match std::env::var(variable).as_deref() {
                Ok("1") | Ok("true") => {
                    return Err(TelemetryError::Configuration(format!(
                        "{variable} opts out of Scarf telemetry"
                    )));
                }
                _ => {}
            }
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
            .build()
            .map_err(|error| TelemetryError::Configuration(error.to_string()))?;

        Ok(Self {
            client,
            base_url,
            options,
        })
    }

    fn request_url(&self, cloud_role: &str, kind: &str) -> String {
        format!(
            "{}/{}/{}",
            self.base_url,
            encode_path_segment(cloud_role),
            encode_path_segment(kind)
        )
    }
}

/// Scarf interprets every value as a string, so only the URL structure has to
/// survive: keep unreserved characters and percent-encode everything else.
fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[async_trait]
impl TelemetrySender for ScarfSender {
    async fn send_event(
        &self,
        event: BaseEvent,
        options_override: Option<PartialTelemetrySenderOptions>,
    ) {
        let _ = self.try_send_event(event, options_override).await;
    }

    async fn try_send_event(
        &self,
        event: BaseEvent,
        options_override: Option<PartialTelemetrySenderOptions>,
    ) -> Result<(), TelemetryError> {
        let distinct_id = options_override
            .as_ref()
            .and_then(|o| o.distinct_id.clone())
            .unwrap_or_else(|| self.options.distinct_id.clone());

        let cloud_role = options_override
            .as_ref()
            .and_then(|o| o.cloud_role.clone())
            .unwrap_or_else(|| self.options.cloud_role.clone());

        let url = self.request_url(&cloud_role, &event.kind);

        // Matches Scarf's own SDKs (e.g. scarf-go): a POST request with an
        // empty body, with all properties encoded as URL query parameters.
        // Scarf's gateway reads properties from the query string, not a
        // request body, so that's not something we can change unilaterally.
        //
        // Properties win over the defaults; Scarf would otherwise receive the
        // same key twice for events that already carry `os`/`arch`/version.
        let mut query: Vec<(String, String)> = event.properties.into_iter().collect();
        let defaults = [
            ("distinctId", distinct_id.as_str()),
            ("cliVersion", env!("CARGO_PKG_VERSION")),
            ("os", std::env::consts::OS),
            ("arch", std::env::consts::ARCH),
        ];
        for (key, value) in defaults {
            if !query.iter().any(|(existing, _)| existing == key) {
                query.push((key.to_string(), value.to_string()));
            }
        }

        let mut backoff = Duration::from_millis(RETRY_INITIAL_BACKOFF_MS);
        let mut last_error = None;

        for attempt in 1..=MAX_CAPTURE_ATTEMPTS {
            match self.client.post(&url).query(&query).send().await {
                Ok(response) if response.status().is_success() => {
                    log::debug!("Scarf accepted event {url}");
                    return Ok(());
                }
                Ok(response) => {
                    let status = response.status();
                    // Scarf rejected the event itself; retrying cannot help.
                    if !status.is_server_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS
                    {
                        return Err(TelemetryError::ScarfStatus(status.as_u16()));
                    }
                    last_error = Some(TelemetryError::ScarfStatus(status.as_u16()));
                }
                Err(error) => last_error = Some(TelemetryError::Scarf(error.to_string())),
            }

            if attempt < MAX_CAPTURE_ATTEMPTS {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_millis(RETRY_MAX_BACKOFF_MS));
            }
        }

        Err(last_error.unwrap_or_else(|| TelemetryError::Scarf("no attempt was made".to_string())))
    }

    async fn initialize_panic_telemetry(&self) {
        install_panic_hook(self.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send_event::test_support::TestServer;
    use std::collections::HashMap;

    fn sender(base_url: &str) -> ScarfSender {
        ScarfSender::new_with_base_url(
            TelemetrySenderOptions {
                distinct_id: "installation-123".to_string(),
                cloud_role: "CLI".to_string(),
            },
            base_url,
        )
        .expect("build Scarf sender")
    }

    #[tokio::test]
    async fn send_event_encodes_event_in_path_and_properties_in_query() {
        let server = TestServer::start(vec!["200 OK"]).await;

        sender(&server.base_url())
            .try_send_event(
                BaseEvent {
                    kind: "codemodRunStarted".to_string(),
                    properties: HashMap::from([(
                        "codemodName".to_string(),
                        "@codemod/react/19/migration-recipe".to_string(),
                    )]),
                },
                None,
            )
            .await
            .expect("event should be accepted");

        let requests = server.finish().await;
        let request_line = requests[0]
            .lines()
            .next()
            .expect("request line")
            .to_string();

        assert!(
            request_line.starts_with("POST /CLI/codemodRunStarted?"),
            "unexpected request line: {request_line}"
        );
        assert!(request_line.contains("distinctId=installation-123"));
        assert!(request_line.contains("codemodName=%40codemod%2Freact%2F19%2Fmigration-recipe"));
    }

    #[tokio::test]
    async fn send_event_retries_transient_responses() {
        let server = TestServer::start(vec!["500 Internal Server Error", "200 OK"]).await;

        sender(&server.base_url())
            .try_send_event(
                BaseEvent {
                    kind: "codemodRunStarted".to_string(),
                    properties: HashMap::new(),
                },
                None,
            )
            .await
            .expect("event should succeed after a retry");

        assert_eq!(server.finish().await.len(), 2);
    }

    #[tokio::test]
    async fn send_event_does_not_retry_client_errors() {
        let server = TestServer::start(vec!["400 Bad Request"]).await;

        let error = sender(&server.base_url())
            .try_send_event(
                BaseEvent {
                    kind: "codemodRunStarted".to_string(),
                    properties: HashMap::new(),
                },
                None,
            )
            .await
            .expect_err("client errors should be reported");

        assert!(matches!(error, TelemetryError::ScarfStatus(400)));
        assert_eq!(server.finish().await.len(), 1);
    }

    #[tokio::test]
    async fn base_url_without_a_scheme_is_assumed_to_be_https() {
        let sender = sender("codemod.gateway.scarf.sh/telemetry");

        assert_eq!(
            sender.request_url("CLI", "codemodRunStarted"),
            "https://codemod.gateway.scarf.sh/telemetry/CLI/codemodRunStarted"
        );
    }

    #[tokio::test]
    async fn event_properties_override_the_defaults_instead_of_duplicating_them() {
        let server = TestServer::start(vec!["200 OK"]).await;

        sender(&server.base_url())
            .try_send_event(
                BaseEvent {
                    kind: "codemodRunStarted".to_string(),
                    properties: HashMap::from([("os".to_string(), "reported-os".to_string())]),
                },
                None,
            )
            .await
            .expect("event should be accepted");

        let requests = server.finish().await;
        let request_line = requests[0]
            .lines()
            .next()
            .expect("request line")
            .to_string();

        assert!(request_line.contains("os=reported-os"), "{request_line}");
        assert_eq!(request_line.matches("os=").count(), 1, "{request_line}");
    }

    #[tokio::test]
    async fn empty_base_url_is_rejected() {
        let result = ScarfSender::new_with_base_url(
            TelemetrySenderOptions {
                distinct_id: "installation-123".to_string(),
                cloud_role: "CLI".to_string(),
            },
            "  ",
        );

        assert!(matches!(result, Err(TelemetryError::Configuration(_))));
    }
}
