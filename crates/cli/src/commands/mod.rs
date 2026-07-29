use crate::TelemetrySenderMutex;
use async_trait::async_trait;
use codemod_telemetry::send_event::{BaseEvent, PartialTelemetrySenderOptions};
use log::debug;

pub mod ai;
pub mod cache;
pub mod harness_adapter;
pub mod init;
pub mod jssg;
pub mod login;
pub mod logout;
pub mod mcp;
pub mod output;
pub mod package_skill;
pub mod publish;
pub mod run;
mod run_telemetry;
pub mod search;
pub mod unpublish;
pub mod whoami;
pub mod workflow;

#[async_trait]
pub(crate) trait TelemetrySenderExt {
    async fn send_event_logged(
        &self,
        event: BaseEvent,
        options_override: Option<PartialTelemetrySenderOptions>,
    );
}

#[async_trait]
impl TelemetrySenderExt for TelemetrySenderMutex {
    async fn send_event_logged(
        &self,
        event: BaseEvent,
        options_override: Option<PartialTelemetrySenderOptions>,
    ) {
        if let Err(error) = self.try_send_event(event, options_override).await {
            debug!("Failed to deliver telemetry event: {error}");
        }
    }
}
