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

#[async_trait]
pub trait TelemetrySender: Send + Sync + 'static {
    async fn send_event(
        &self,
        event: BaseEvent,
        options_override: Option<PartialTelemetrySenderOptions>,
    );
    async fn initialize_panic_telemetry(&self);
}

pub struct PostHogSender {
    client: Arc<posthog_rs::Client>,
    options: TelemetrySenderOptions,
}

pub const POSTHOG_API_KEY: &str = env!("POSTHOG_API_KEY");

impl PostHogSender {
    pub async fn new(options: TelemetrySenderOptions) -> Self {
        let client = posthog_rs::client(POSTHOG_API_KEY).await;
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
    ) {
        let distinct_id = options_override
            .as_ref()
            .and_then(|o| o.distinct_id.clone())
            .unwrap_or_else(|| self.options.distinct_id.clone());

        let cloud_role = options_override
            .as_ref()
            .and_then(|o| o.cloud_role.clone())
            .unwrap_or_else(|| self.options.cloud_role.clone());

        let mut posthog_event = posthog_rs::Event::new(
            format!("codemod.{}.{}", cloud_role, event.kind),
            distinct_id.clone(),
        );

        for (key, value) in event.properties {
            if let Err(e) = posthog_event.insert_prop(key, value) {
                eprintln!("Failed to insert property into PostHog event: {e}");
            }
        }

        if let Err(e) = self.client.capture(posthog_event).await {
            eprintln!("Failed to send PostHog event: {e}");
        }
    }

    async fn initialize_panic_telemetry(&self) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let _ = RUNTIME_HANDLE.set(handle);
        }

        let client = self.client.clone();
        let options = self.options.clone();

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
                let client = client.clone();
                let options = options.clone();

                handle.spawn(async move {
                    let mut posthog_event = posthog_rs::Event::new(
                        format!("codemod.{}.cliPanic", options.cloud_role),
                        options.distinct_id.clone(),
                    );

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

                    for (key, value) in properties {
                        let _ = posthog_event.insert_prop(key, value);
                    }

                    let _ =
                        tokio::time::timeout(Duration::from_secs(5), client.capture(posthog_event))
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
