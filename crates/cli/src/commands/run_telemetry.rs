use crate::commands::TelemetrySenderExt;
use crate::{TelemetrySenderMutex, CLI_VERSION};
use codemod_telemetry::send_event::BaseEvent;
use std::collections::HashMap;

pub(super) struct CodemodRunTelemetry {
    codemod_name: String,
    package_version: String,
    execution_id: String,
    workflow_name: Option<String>,
    dry_run: bool,
}

pub(super) enum CodemodRunOutcome {
    Succeeded {
        files_modified: usize,
        files_unmodified: usize,
        files_with_errors: usize,
    },
    Failed {
        error_message: String,
    },
}

impl CodemodRunTelemetry {
    pub(super) fn new(
        codemod_name: String,
        package_version: String,
        execution_id: String,
        workflow_name: Option<String>,
        dry_run: bool,
    ) -> Self {
        Self {
            codemod_name,
            package_version,
            execution_id,
            workflow_name,
            dry_run,
        }
    }

    fn common_properties(&self) -> HashMap<String, String> {
        let mut properties = HashMap::from([
            ("codemodName".to_string(), self.codemod_name.clone()),
            ("packageVersion".to_string(), self.package_version.clone()),
            ("executionId".to_string(), self.execution_id.clone()),
            ("dryRun".to_string(), self.dry_run.to_string()),
            ("cliVersion".to_string(), CLI_VERSION.to_string()),
            ("os".to_string(), std::env::consts::OS.to_string()),
            ("arch".to_string(), std::env::consts::ARCH.to_string()),
        ]);
        if let Some(workflow_name) = &self.workflow_name {
            properties.insert("workflowName".to_string(), workflow_name.clone());
        }
        properties
    }

    fn started_event(&self) -> BaseEvent {
        BaseEvent {
            kind: "codemodRunStarted".to_string(),
            properties: self.common_properties(),
        }
    }

    fn completed_event(&self, outcome: CodemodRunOutcome, duration_ms: u128) -> BaseEvent {
        let mut properties = self.common_properties();
        properties.insert("durationMs".to_string(), duration_ms.to_string());
        match outcome {
            CodemodRunOutcome::Succeeded {
                files_modified,
                files_unmodified,
                files_with_errors,
            } => {
                properties.insert("outcome".to_string(), "succeeded".to_string());
                properties.insert("filesModified".to_string(), files_modified.to_string());
                properties.insert("filesUnmodified".to_string(), files_unmodified.to_string());
                properties.insert("filesWithErrors".to_string(), files_with_errors.to_string());
            }
            CodemodRunOutcome::Failed { error_message } => {
                properties.insert("outcome".to_string(), "failed".to_string());
                properties.insert("errorMessage".to_string(), error_message);
            }
        }
        BaseEvent {
            kind: "codemodRunCompleted".to_string(),
            properties,
        }
    }

    fn legacy_executed_event(&self, files_modified: usize, duration_ms: u128) -> BaseEvent {
        let mut properties = self.common_properties();
        properties.insert("fileCount".to_string(), files_modified.to_string());
        properties.insert("durationMs".to_string(), duration_ms.to_string());
        BaseEvent {
            kind: "codemodExecuted".to_string(),
            properties,
        }
    }
}

pub(super) async fn send_event(telemetry: &TelemetrySenderMutex, event: BaseEvent) {
    telemetry.send_event_logged(event, None).await;
}

pub(super) async fn send_started_event(
    telemetry: &TelemetrySenderMutex,
    run_telemetry: &CodemodRunTelemetry,
) {
    send_event(telemetry, run_telemetry.started_event()).await;
}

pub(super) async fn send_completed_event(
    telemetry: &TelemetrySenderMutex,
    run_telemetry: &CodemodRunTelemetry,
    outcome: CodemodRunOutcome,
    duration_ms: u128,
) {
    send_event(
        telemetry,
        run_telemetry.completed_event(outcome, duration_ms),
    )
    .await;
}

pub(super) async fn send_success_events(
    telemetry: &TelemetrySenderMutex,
    run_telemetry: &CodemodRunTelemetry,
    outcome: CodemodRunOutcome,
    files_modified: usize,
    duration_ms: u128,
) {
    tokio::join!(
        send_event(
            telemetry,
            run_telemetry.legacy_executed_event(files_modified, duration_ms),
        ),
        send_completed_event(telemetry, run_telemetry, outcome, duration_ms),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemod_telemetry::send_event::{PartialTelemetrySenderOptions, TelemetrySender};
    use std::sync::{Arc, Mutex};

    struct RecordingTelemetrySender {
        events: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl TelemetrySender for RecordingTelemetrySender {
        async fn send_event(
            &self,
            event: BaseEvent,
            _options_override: Option<PartialTelemetrySenderOptions>,
        ) {
            self.events.lock().expect("events lock").push(event.kind);
        }

        async fn initialize_panic_telemetry(&self) {}
    }

    #[test]
    fn run_events_share_stable_canonical_properties() {
        let telemetry = CodemodRunTelemetry::new(
            "@codemod/react/19/migration-recipe".to_string(),
            "1.2.3".to_string(),
            "execution-123".to_string(),
            Some("migration".to_string()),
            true,
        );

        let started = telemetry.started_event();
        let completed = telemetry.completed_event(
            CodemodRunOutcome::Succeeded {
                files_modified: 4,
                files_unmodified: 2,
                files_with_errors: 0,
            },
            1250,
        );

        for event in [&started, &completed] {
            assert_eq!(
                event.properties.get("codemodName").map(String::as_str),
                Some("@codemod/react/19/migration-recipe")
            );
            assert_eq!(
                event.properties.get("executionId").map(String::as_str),
                Some("execution-123")
            );
            assert_eq!(
                event.properties.get("packageVersion").map(String::as_str),
                Some("1.2.3")
            );
            assert_eq!(
                event.properties.get("workflowName").map(String::as_str),
                Some("migration")
            );
            assert_eq!(
                event.properties.get("dryRun").map(String::as_str),
                Some("true")
            );
        }
        assert_eq!(started.kind, "codemodRunStarted");
        assert_eq!(completed.kind, "codemodRunCompleted");
        assert_eq!(
            completed.properties.get("outcome").map(String::as_str),
            Some("succeeded")
        );
        assert_eq!(
            completed.properties.get("durationMs").map(String::as_str),
            Some("1250")
        );
        assert_eq!(
            completed
                .properties
                .get("filesModified")
                .map(String::as_str),
            Some("4")
        );

        let failed = telemetry.completed_event(
            CodemodRunOutcome::Failed {
                error_message: "workflow failed".to_string(),
            },
            500,
        );
        assert_eq!(
            failed.properties.get("outcome").map(String::as_str),
            Some("failed")
        );
        assert_eq!(
            failed.properties.get("errorMessage").map(String::as_str),
            Some("workflow failed")
        );
    }

    #[tokio::test]
    async fn success_events_are_delivered_before_report_handling() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sender: TelemetrySenderMutex = Arc::new(Box::new(RecordingTelemetrySender {
            events: Arc::clone(&events),
        }));
        let telemetry = CodemodRunTelemetry::new(
            "@codemod/react/19/migration-recipe".to_string(),
            "1.2.3".to_string(),
            "execution-123".to_string(),
            None,
            false,
        );

        send_success_events(
            &sender,
            &telemetry,
            CodemodRunOutcome::Succeeded {
                files_modified: 1,
                files_unmodified: 0,
                files_with_errors: 0,
            },
            1,
            10,
        )
        .await;
        events
            .lock()
            .expect("events lock")
            .push("reportHandlingStarted".to_string());

        let events = events.lock().expect("events lock");
        let report_position = events
            .iter()
            .position(|event| event == "reportHandlingStarted")
            .expect("report marker");
        for kind in ["codemodExecuted", "codemodRunCompleted"] {
            let position = events
                .iter()
                .position(|event| event == kind)
                .expect("telemetry event");
            assert!(position < report_position);
        }
    }
}
