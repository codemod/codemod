use async_trait::async_trait;
use chrono::Utc;
use posthog_rs;
use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::panic;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

#[derive(Debug, Clone)]
pub struct TelemetrySenderOptions {
    pub distinct_id: String,
    pub cloud_role: String,
}

#[derive(Debug, Clone)]
pub struct PartialTelemetrySenderOptions {
    pub distinct_id: Option<String>,
    pub cloud_role: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct BaseEvent {
    pub kind: String,
    #[serde(flatten)]
    pub properties: HashMap<String, String>,
}

static RUNTIME_HANDLE: OnceCell<tokio::runtime::Handle> = OnceCell::const_new();
pub type TelemetryError = posthog_rs::Error;

#[async_trait]
pub trait TelemetrySender: Send + Sync + 'static {
    async fn send_event(
        &self,
        event: BaseEvent,
        options_override: Option<PartialTelemetrySenderOptions>,
    ) -> Result<(), TelemetryError>;
    async fn initialize_panic_telemetry(&self);
}

#[derive(Clone)]
pub struct PostHogSender {
    client: Arc<posthog_rs::Client>,
    options: TelemetrySenderOptions,
}

pub const POSTHOG_API_KEY: &str = env!("POSTHOG_API_KEY");

impl PostHogSender {
    pub async fn new(options: TelemetrySenderOptions) -> Self {
        let client = posthog_rs::client(POSTHOG_API_KEY).await;
        Self::with_client(options, client)
    }

    fn with_client(options: TelemetrySenderOptions, client: posthog_rs::Client) -> Self {
        Self {
            client: Arc::new(client),
            options,
        }
    }
}

#[async_trait]
impl TelemetrySender for PostHogSender {
    async fn send_event(
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

        let mut posthog_event =
            posthog_rs::Event::new(format!("codemod.{cloud_role}.{}", event.kind), distinct_id);

        for (key, value) in event.properties {
            posthog_event.insert_prop(key, value)?;
        }

        self.client.capture_immediate(posthog_event).await?;
        Ok(())
    }

    async fn initialize_panic_telemetry(&self) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let _ = RUNTIME_HANDLE.set(handle);
        }

        let sender = self.clone();

        panic::set_hook(Box::new(move |panic_info| {
            let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

            let panic_message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic occurred".to_string()
            };

            let location = if let Some(location) = panic_info.location() {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            } else {
                "Unknown location".to_string()
            };

            if let Some(handle) = RUNTIME_HANDLE.get() {
                let sender = sender.clone();

                handle.spawn(async move {
                    let properties = HashMap::from([
                        ("timestamp".to_string(), timestamp),
                        ("message".to_string(), panic_message),
                        ("location".to_string(), location),
                        (
                            "cliVersion".to_string(),
                            env!("CARGO_PKG_VERSION").to_string(),
                        ),
                        ("os".to_string(), std::env::consts::OS.to_string()),
                        ("arch".to_string(), std::env::consts::ARCH.to_string()),
                    ]);

                    let _ = tokio::time::timeout(
                        Duration::from_secs(5),
                        sender.send_event(
                            BaseEvent {
                                kind: "cliPanic".to_string(),
                                properties,
                            },
                            None,
                        ),
                    )
                    .await;
                });

                std::thread::sleep(Duration::from_millis(100));
            }

            if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                std::panic::resume_unwind(Box::new(*s));
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                std::panic::resume_unwind(Box::new(s.clone()));
            } else {
                std::panic::resume_unwind(Box::new("Unknown panic"));
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_request(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let size = stream
                .read(&mut chunk)
                .await
                .expect("read telemetry request");
            assert!(size > 0, "connection closed before request completed");
            request.extend_from_slice(&chunk[..size]);

            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                return String::from_utf8_lossy(&request).into_owned();
            }
        }
    }

    async fn test_sender(
        statuses: Vec<&'static str>,
    ) -> (PostHogSender, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind telemetry test server");
        let address = listener.local_addr().expect("telemetry test address");
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for status in statuses {
                let (mut stream, _) = listener.accept().await.expect("accept telemetry request");
                let request = read_request(&mut stream).await;
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write telemetry response");
                requests.push(request);
            }
            requests
        });

        let client_options = posthog_rs::ClientOptionsBuilder::default()
            .api_key("test-key".to_string())
            .host(format!("http://{address}"))
            .request_timeout_seconds(1)
            .max_capture_attempts(3)
            .retry_initial_backoff_ms(1)
            .retry_max_backoff_ms(2)
            .build()
            .expect("build test PostHog client options");
        let client = posthog_rs::client(client_options).await;
        (
            PostHogSender::with_client(
                TelemetrySenderOptions {
                    distinct_id: "installation-123".to_string(),
                    cloud_role: "CLI".to_string(),
                },
                client,
            ),
            server,
        )
    }

    #[tokio::test]
    async fn send_event_includes_stable_identity_and_event_name() {
        let (sender, server) = test_sender(vec!["200 OK"]).await;

        sender
            .send_event(
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

        let requests = server.await.expect("telemetry server should finish");
        let request = &requests[0];
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("request should contain a body");
        let payload: serde_json::Value =
            serde_json::from_str(body).expect("request body should be JSON");
        let event = &payload["batch"][0];

        assert_eq!(event["distinct_id"], "installation-123");
        assert!(event.get("$distinct_id").is_none());
        assert_eq!(event["event"], "codemod.CLI.codemodRunStarted");
    }

    #[tokio::test]
    async fn send_event_surfaces_non_success_responses() {
        let (sender, server) = test_sender(vec!["400 Bad Request"]).await;

        let error = sender
            .send_event(
                BaseEvent {
                    kind: "codemodRunStarted".to_string(),
                    properties: HashMap::new(),
                },
                None,
            )
            .await
            .expect_err("non-success response should be reported");

        assert!(matches!(error, posthog_rs::Error::BadRequest(_)));
        server.await.expect("telemetry server should finish");
    }

    #[tokio::test]
    async fn send_event_retries_transient_responses() {
        let (sender, server) = test_sender(vec![
            "500 Internal Server Error",
            "503 Unavailable",
            "200 OK",
        ])
        .await;

        sender
            .send_event(
                BaseEvent {
                    kind: "codemodRunStarted".to_string(),
                    properties: HashMap::new(),
                },
                None,
            )
            .await
            .expect("event should succeed after retries");

        let requests = server.await.expect("telemetry server should finish");
        assert_eq!(requests.len(), 3);
    }

    #[tokio::test]
    async fn send_event_reports_exhausted_transient_responses() {
        let (sender, server) = test_sender(vec![
            "500 Internal Server Error",
            "503 Unavailable",
            "504 Gateway Timeout",
        ])
        .await;

        let error = sender
            .send_event(
                BaseEvent {
                    kind: "codemodRunStarted".to_string(),
                    properties: HashMap::new(),
                },
                None,
            )
            .await
            .expect_err("exhausted retries should be reported");

        assert!(error.to_string().contains("504"));
        let requests = server.await.expect("telemetry server should finish");
        assert_eq!(requests.len(), 3);
    }
}
