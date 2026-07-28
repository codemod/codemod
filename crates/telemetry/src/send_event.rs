use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::panic;
use std::time::Duration;
use tokio::sync::OnceCell;
use uuid::Uuid;

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
const POSTHOG_CAPTURE_URL: &str = "https://us.i.posthog.com/i/v0/e/";
const MAX_CAPTURE_ATTEMPTS: usize = 3;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("PostHog request failed: {0}")]
    Connection(String),
    #[error("PostHog rejected the event with HTTP {status}: {body}")]
    Rejected { status: u16, body: String },
    #[error("PostHog delivery timed out")]
    Timeout,
}

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
    client: reqwest::Client,
    capture_url: String,
    options: TelemetrySenderOptions,
}

pub const POSTHOG_API_KEY: &str = env!("POSTHOG_API_KEY");

#[derive(Serialize)]
struct PostHogCapture {
    api_key: &'static str,
    uuid: Uuid,
    event: String,
    distinct_id: String,
    properties: HashMap<String, Value>,
}

impl PostHogSender {
    pub async fn new(options: TelemetrySenderOptions) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("static PostHog HTTP client options should be valid");
        Self::with_client(options, client, POSTHOG_CAPTURE_URL.to_string())
    }

    fn with_client(
        options: TelemetrySenderOptions,
        client: reqwest::Client,
        capture_url: String,
    ) -> Self {
        Self {
            client,
            capture_url,
            options,
        }
    }

    async fn capture(&self, event: PostHogCapture) -> Result<(), TelemetryError> {
        tokio::time::timeout(CAPTURE_TIMEOUT, self.capture_with_retries(event))
            .await
            .map_err(|_| TelemetryError::Timeout)?
    }

    async fn capture_with_retries(&self, event: PostHogCapture) -> Result<(), TelemetryError> {
        let mut backoff = Duration::from_millis(100);

        for attempt in 1..=MAX_CAPTURE_ATTEMPTS {
            let response = self
                .client
                .post(&self.capture_url)
                .json(&event)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    let status = response.status().as_u16();
                    let retryable = matches!(status, 408 | 429 | 500 | 502 | 503 | 504);
                    let body = response.text().await.unwrap_or_default();
                    if !retryable || attempt == MAX_CAPTURE_ATTEMPTS {
                        return Err(TelemetryError::Rejected { status, body });
                    }
                }
                Err(error) if attempt == MAX_CAPTURE_ATTEMPTS => {
                    return Err(TelemetryError::Connection(error.to_string()));
                }
                Err(_) => {}
            }

            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }

        unreachable!("capture attempt loop always returns on its final attempt")
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

        let mut properties = event
            .properties
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect::<HashMap<_, _>>();
        properties.insert("cloudRole".to_string(), Value::String(cloud_role.clone()));
        properties.insert(
            "$lib".to_string(),
            Value::String("codemod-rust-cli".to_string()),
        );

        self.capture(PostHogCapture {
            api_key: POSTHOG_API_KEY,
            uuid: Uuid::new_v4(),
            event: format!("codemod.{cloud_role}.{}", event.kind),
            distinct_id,
            properties,
        })
        .await
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

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .expect("build test HTTP client");
        (
            PostHogSender::with_client(
                TelemetrySenderOptions {
                    distinct_id: "installation-123".to_string(),
                    cloud_role: "CLI".to_string(),
                },
                client,
                format!("http://{address}/i/v0/e/"),
            ),
            server,
        )
    }

    #[tokio::test]
    async fn send_event_includes_stable_identity_and_cloud_role() {
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
        let payload: Value = serde_json::from_str(body).expect("request body should be JSON");

        assert_eq!(payload["distinct_id"], "installation-123");
        assert!(payload.get("$distinct_id").is_none());
        assert_eq!(payload["properties"]["cloudRole"], "CLI");
        assert_eq!(payload["event"], "codemod.CLI.codemodRunStarted");
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

        assert!(error.to_string().contains("400"));
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
