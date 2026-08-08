use async_trait::async_trait;
use std::sync::Arc;

use crate::send_event::{
    install_panic_hook, BaseEvent, PartialTelemetrySenderOptions, TelemetryError, TelemetrySender,
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
        let secondaries = futures::future::join_all(self.secondary.iter().map(|sender| {
            let event = event.clone();
            let options_override = options_override.clone();
            async move { sender.try_send_event(event, options_override).await }
        }));

        let (primary, secondaries) = futures::future::join(
            self.primary.try_send_event(event, options_override),
            secondaries,
        )
        .await;

        for error in secondaries.into_iter().filter_map(Result::err) {
            log::debug!("Secondary telemetry backend failed: {error}");
        }

        primary
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
        assert_eq!(secondary.seen.lock().unwrap().len(), 1);
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

        assert_eq!(secondary.seen.lock().unwrap().len(), 1);
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

        assert_eq!(secondary.seen.lock().unwrap().len(), 1);
    }
}
