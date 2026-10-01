use async_trait::async_trait;
use std::sync::Arc;

use crate::send_event::{
    BaseEvent, PartialTelemetrySenderOptions, TelemetryError, TelemetrySender, install_panic_hook,
};

/// Fans an event out to several backends at once.
///
/// The first sender is the primary one: its result is what `try_send_event`
/// reports. Every other sender is best-effort — its failure is logged and
/// otherwise ignored, so a degraded secondary backend can never turn into a
/// user-visible error.
#[derive(Clone)]
pub struct CompositeSender {
    primary: Arc<dyn TelemetrySender>,
    secondary: Vec<Arc<dyn TelemetrySender>>,
}

impl CompositeSender {
    pub fn new(primary: Arc<dyn TelemetrySender>) -> Self {
        Self {
            primary,
            secondary: Vec::new(),
        }
    }

    pub fn with_secondary(mut self, sender: Arc<dyn TelemetrySender>) -> Self {
        self.secondary.push(sender);
        self
    }
}

#[async_trait]
impl TelemetrySender for CompositeSender {
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
        // Secondaries are detached rather than awaited: they may retry/back
        // off internally, and a degraded secondary must not add latency to
        // the caller once the primary backend has already responded.
        for secondary in &self.secondary {
            let secondary = Arc::clone(secondary);
            let event = event.clone();
            let options_override = options_override.clone();
            tokio::spawn(async move {
                if let Err(error) = secondary.try_send_event(event, options_override).await {
                    log::debug!("Secondary telemetry backend failed: {error}");
                }
            });
        }

        self.primary.try_send_event(event, options_override).await
    }

    async fn initialize_panic_telemetry(&self) {
        // A process has a single panic hook, so the composite installs one that
        // fans out just like a regular event.
        install_panic_hook(self.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    struct RecordingSender {
        result: Option<TelemetryError>,
        seen: Mutex<Vec<String>>,
    }

    impl RecordingSender {
        fn new(result: Option<TelemetryError>) -> Arc<Self> {
            Arc::new(Self {
                result,
                seen: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl TelemetrySender for RecordingSender {
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
            _options_override: Option<PartialTelemetrySenderOptions>,
        ) -> Result<(), TelemetryError> {
            self.seen.lock().unwrap().push(event.kind);
            match &self.result {
                None => Ok(()),
                Some(error) => Err(TelemetryError::Scarf(error.to_string())),
            }
        }

        async fn initialize_panic_telemetry(&self) {}
    }

    fn event() -> BaseEvent {
        BaseEvent {
            kind: "codemodRunStarted".to_string(),
            properties: HashMap::from([("codemodName".to_string(), "demo".to_string())]),
        }
    }

    /// Secondaries are detached, so their delivery happens on a spawned task
    /// after `try_send_event` already returned; poll briefly instead of
    /// asserting on it immediately.
    async fn wait_until_seen(sender: &RecordingSender, count: usize) {
        for _ in 0..200 {
            if sender.seen.lock().unwrap().len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("secondary did not observe the expected event in time");
    }

    #[tokio::test]
    async fn every_backend_receives_the_event() {
        let primary = RecordingSender::new(None);
        let secondary = RecordingSender::new(None);
        let sender = CompositeSender::new(primary.clone()).with_secondary(secondary.clone());

        sender
            .try_send_event(event(), None)
            .await
            .expect("event should be accepted");

        assert_eq!(primary.seen.lock().unwrap().len(), 1);
        wait_until_seen(&secondary, 1).await;
    }

    #[tokio::test]
    async fn secondary_failure_does_not_fail_the_send() {
        let primary = RecordingSender::new(None);
        let secondary = RecordingSender::new(Some(TelemetryError::ScarfStatus(500)));
        let sender = CompositeSender::new(primary.clone()).with_secondary(secondary.clone());

        sender
            .try_send_event(event(), None)
            .await
            .expect("a failing secondary must not fail the send");

        wait_until_seen(&secondary, 1).await;
    }

    #[tokio::test]
    async fn primary_failure_is_reported_and_secondaries_still_run() {
        let primary = RecordingSender::new(Some(TelemetryError::NoEventSubmitted));
        let secondary = RecordingSender::new(None);
        let sender = CompositeSender::new(primary.clone()).with_secondary(secondary.clone());

        sender
            .try_send_event(event(), None)
            .await
            .expect_err("primary failure should be reported");

        wait_until_seen(&secondary, 1).await;
    }
}
