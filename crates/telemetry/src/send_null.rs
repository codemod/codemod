use crate::send_event::{BaseEvent, PartialTelemetrySenderOptions, TelemetrySender};
use async_trait::async_trait;

pub struct NullSender;

#[async_trait]
impl TelemetrySender for NullSender {
    async fn send_event(
        &self,
        _event: BaseEvent,
        _options_override: Option<PartialTelemetrySenderOptions>,
    ) {
        // Do nothing
    }
    async fn initialize_panic_telemetry(&self) {
        // Do nothing
    }
}
