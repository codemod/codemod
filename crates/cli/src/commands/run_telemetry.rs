use crate::commands::TelemetrySenderExt;
use crate::{CLI_VERSION, TelemetrySenderMutex};
use butterflow_core::nested_codemod_run::{
    NestedCodemodRun, NestedCodemodRunEvent, NestedCodemodRunObserver, NestedCodemodRunOutcome,
};
use butterflow_models::WorkflowRun;
use codemod_telemetry::send_event::BaseEvent;
use std::collections::HashMap;
use std::sync::Arc;

pub(super) struct CodemodRunTelemetry {
    codemod_name: String,
    package_version: String,
    execution_id: String,
    workflow_name: Option<String>,
    dry_run: Option<bool>,
    nested: Option<NestedRunProperties>,
}

#[derive(Clone, Copy)]
pub(super) struct CodemodRunStats {
    pub(super) files_modified: usize,
    pub(super) files_unmodified: usize,
    pub(super) files_with_errors: usize,
}

pub(super) enum CodemodRunOutcome {
    Succeeded { stats: Option<CodemodRunStats> },
    Failed { error_message: String },
}

struct NestedRunProperties {
    root_execution_id: String,
    root_codemod_name: String,
    dependency_path: Vec<String>,
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
            dry_run: Some(dry_run),
            nested: None,
        }
    }

    fn for_nested(
        run: &NestedCodemodRun,
        root_execution_id: String,
        root_codemod_name: String,
        dry_run: Option<bool>,
    ) -> Self {
        Self {
            codemod_name: run.codemod_name.clone(),
            package_version: run.package_version.clone(),
            execution_id: run.execution_id.clone(),
            workflow_name: None,
            dry_run,
            nested: Some(NestedRunProperties {
                root_execution_id,
                root_codemod_name,
                dependency_path: run.dependency_path.clone(),
            }),
        }
    }

    pub(super) fn nested_observer(
        &self,
        telemetry: TelemetrySenderMutex,
    ) -> Arc<dyn NestedCodemodRunObserver> {
        nested_codemod_run_observer(
            telemetry,
            self.execution_id.clone(),
            self.codemod_name.clone(),
            self.dry_run,
        )
    }

    fn common_properties(&self) -> HashMap<String, String> {
        let mut properties = HashMap::from([
            ("codemodName".to_string(), self.codemod_name.clone()),
            ("packageVersion".to_string(), self.package_version.clone()),
            ("executionId".to_string(), self.execution_id.clone()),
            ("cliVersion".to_string(), CLI_VERSION.to_string()),
            ("os".to_string(), std::env::consts::OS.to_string()),
            ("arch".to_string(), std::env::consts::ARCH.to_string()),
            ("isNested".to_string(), self.nested.is_some().to_string()),
        ]);
        if let Some(dry_run) = self.dry_run {
            properties.insert("dryRun".to_string(), dry_run.to_string());
        }
        if let Some(workflow_name) = &self.workflow_name {
            properties.insert("workflowName".to_string(), workflow_name.clone());
        }
        if let Some(nested) = &self.nested {
            properties.insert(
                "rootExecutionId".to_string(),
                nested.root_execution_id.clone(),
            );
            properties.insert(
                "rootCodemodName".to_string(),
                nested.root_codemod_name.clone(),
            );
            properties.insert(
                "dependencyDepth".to_string(),
                nested.dependency_path.len().to_string(),
            );
            properties.insert(
                "dependencyPath".to_string(),
                nested.dependency_path.join(" > "),
            );
            let parent_codemod_name = nested
                .dependency_path
                .iter()
                .rev()
                .nth(1)
                .cloned()
                .unwrap_or_else(|| nested.root_codemod_name.clone());
            properties.insert("parentCodemodName".to_string(), parent_codemod_name);
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
            CodemodRunOutcome::Succeeded { stats } => {
                properties.insert("outcome".to_string(), "succeeded".to_string());
                if let Some(stats) = stats {
                    properties.insert(
                        "filesModified".to_string(),
                        stats.files_modified.to_string(),
                    );
                    properties.insert(
                        "filesUnmodified".to_string(),
                        stats.files_unmodified.to_string(),
                    );
                    properties.insert(
                        "filesWithErrors".to_string(),
                        stats.files_with_errors.to_string(),
                    );
                }
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

    fn legacy_executed_event(
        &self,
        stats: Option<CodemodRunStats>,
        duration_ms: u128,
    ) -> BaseEvent {
        let mut properties = self.common_properties();
        if let Some(stats) = stats {
            properties.insert("fileCount".to_string(), stats.files_modified.to_string());
        }
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
    stats: Option<CodemodRunStats>,
    duration_ms: u128,
) {
    tokio::join!(
        send_event(
            telemetry,
            run_telemetry.legacy_executed_event(stats, duration_ms),
        ),
        send_completed_event(
            telemetry,
            run_telemetry,
            CodemodRunOutcome::Succeeded { stats },
            duration_ms,
        ),
    );
}

pub(crate) fn nested_codemod_run_observer(
    telemetry: TelemetrySenderMutex,
    root_execution_id: String,
    root_codemod_name: String,
    dry_run: impl Into<Option<bool>>,
) -> Arc<dyn NestedCodemodRunObserver> {
    Arc::new(NestedCodemodTelemetryObserver {
        telemetry,
        root_execution_id,
        root_codemod_name,
        dry_run: dry_run.into(),
    })
}

pub(crate) fn persisted_workflow_root_name(workflow_run: &WorkflowRun) -> String {
    workflow_run
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            workflow_run
                .bundle_path
                .as_deref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("workflow:{}", workflow_run.id))
}

struct NestedCodemodTelemetryObserver {
    telemetry: TelemetrySenderMutex,
    root_execution_id: String,
    root_codemod_name: String,
    dry_run: Option<bool>,
}

impl NestedCodemodTelemetryObserver {
    fn run_telemetry(&self, run: &NestedCodemodRun) -> CodemodRunTelemetry {
        CodemodRunTelemetry::for_nested(
            run,
            self.root_execution_id.clone(),
            self.root_codemod_name.clone(),
            self.dry_run,
        )
    }
}

#[async_trait::async_trait]
impl NestedCodemodRunObserver for NestedCodemodTelemetryObserver {
    async fn record(&self, event: NestedCodemodRunEvent) {
        match event {
            NestedCodemodRunEvent::Started(run) => {
                send_started_event(&self.telemetry, &self.run_telemetry(&run)).await;
            }
            NestedCodemodRunEvent::Completed {
                run,
                outcome,
                duration_ms,
            } => {
                let run_telemetry = self.run_telemetry(&run);
                match outcome {
                    NestedCodemodRunOutcome::Succeeded => {
                        send_success_events(&self.telemetry, &run_telemetry, None, duration_ms)
                            .await;
                    }
                    NestedCodemodRunOutcome::Failed { error_message } => {
                        send_completed_event(
                            &self.telemetry,
                            &run_telemetry,
                            CodemodRunOutcome::Failed { error_message },
                            duration_ms,
                        )
                        .await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterflow_models::{WorkflowStatus, workflow::Workflow};
    use chrono::Utc;
    use codemod_telemetry::send_event::{PartialTelemetrySenderOptions, TelemetrySender};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

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
                stats: Some(CodemodRunStats {
                    files_modified: 4,
                    files_unmodified: 2,
                    files_with_errors: 0,
                }),
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
            assert_eq!(
                event.properties.get("isNested").map(String::as_str),
                Some("false")
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
            Some(CodemodRunStats {
                files_modified: 1,
                files_unmodified: 0,
                files_with_errors: 0,
            }),
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

    struct RecordingEventSender {
        events: Arc<Mutex<Vec<BaseEvent>>>,
    }

    #[async_trait::async_trait]
    impl TelemetrySender for RecordingEventSender {
        async fn send_event(
            &self,
            event: BaseEvent,
            _options_override: Option<PartialTelemetrySenderOptions>,
        ) {
            self.events.lock().expect("events lock").push(event);
        }

        async fn initialize_panic_telemetry(&self) {}
    }

    fn nested_run() -> NestedCodemodRun {
        NestedCodemodRun {
            codemod_name: "@codemod/child-b".to_string(),
            package_version: "2.0.0".to_string(),
            execution_id: "child-execution".to_string(),
            dependency_path: vec![
                "@codemod/child-a".to_string(),
                "@codemod/child-b".to_string(),
            ],
        }
    }

    fn persisted_workflow_run(
        id: Uuid,
        name: Option<&str>,
        bundle_path: Option<&str>,
    ) -> WorkflowRun {
        WorkflowRun {
            id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status: WorkflowStatus::Running,
            params: Default::default(),
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            bundle_path: bundle_path.map(PathBuf::from),
            capabilities: None,
            name: name.map(str::to_owned),
            target_path: None,
        }
    }

    #[test]
    fn persisted_workflow_name_uses_name_then_bundle_basename_then_id() {
        let run_id = Uuid::new_v4();

        let named = persisted_workflow_run(
            run_id,
            Some("  persisted migration  "),
            Some("/private/source/bundle-name"),
        );
        assert_eq!(persisted_workflow_root_name(&named), "persisted migration");

        let bundled =
            persisted_workflow_run(run_id, Some("   "), Some("/private/source/bundle-name"));
        assert_eq!(persisted_workflow_root_name(&bundled), "bundle-name");

        let anonymous = persisted_workflow_run(run_id, None, None);
        assert_eq!(
            persisted_workflow_root_name(&anonymous),
            format!("workflow:{run_id}")
        );
    }

    #[test]
    fn direct_child_uses_root_codemod_as_parent() {
        let mut run = nested_run();
        run.dependency_path = vec!["@codemod/child-b".to_string()];
        let event = CodemodRunTelemetry::for_nested(
            &run,
            "root-execution".to_string(),
            "@codemod/root".to_string(),
            Some(false),
        )
        .started_event();

        assert_eq!(
            event
                .properties
                .get("parentCodemodName")
                .map(String::as_str),
            Some("@codemod/root")
        );
        assert_eq!(
            event.properties.get("dependencyDepth").map(String::as_str),
            Some("1")
        );
    }

    #[tokio::test]
    async fn nested_run_omits_dry_run_when_attach_context_is_unknown() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sender: TelemetrySenderMutex = Arc::new(Box::new(RecordingEventSender {
            events: Arc::clone(&events),
        }));
        let observer = nested_codemod_run_observer(
            sender,
            "root-execution".to_string(),
            "@codemod/root".to_string(),
            None,
        );

        observer
            .record(NestedCodemodRunEvent::Started(nested_run()))
            .await;
        observer
            .record(NestedCodemodRunEvent::Completed {
                run: nested_run(),
                outcome: NestedCodemodRunOutcome::Succeeded,
                duration_ms: 10,
            })
            .await;

        let events = events.lock().expect("events lock");
        assert_eq!(events.len(), 3);
        assert!(
            events
                .iter()
                .all(|event| !event.properties.contains_key("dryRun"))
        );
    }

    #[tokio::test]
    async fn nested_success_emits_correlated_run_events_without_invented_file_counts() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sender: TelemetrySenderMutex = Arc::new(Box::new(RecordingEventSender {
            events: Arc::clone(&events),
        }));
        let observer = nested_codemod_run_observer(
            sender,
            "root-execution".to_string(),
            "@codemod/root".to_string(),
            true,
        );
        let run = nested_run();

        observer
            .record(NestedCodemodRunEvent::Started(run.clone()))
            .await;
        observer
            .record(NestedCodemodRunEvent::Completed {
                run,
                outcome: NestedCodemodRunOutcome::Succeeded,
                duration_ms: 250,
            })
            .await;

        let events = events.lock().expect("events lock");
        assert_eq!(events.len(), 3);
        for kind in [
            "codemodRunStarted",
            "codemodExecuted",
            "codemodRunCompleted",
        ] {
            let event = events
                .iter()
                .find(|event| event.kind == kind)
                .expect("nested telemetry event");
            assert_eq!(
                event.properties.get("codemodName").map(String::as_str),
                Some("@codemod/child-b")
            );
            assert_eq!(
                event.properties.get("executionId").map(String::as_str),
                Some("child-execution")
            );
            assert_eq!(
                event.properties.get("isNested").map(String::as_str),
                Some("true")
            );
            assert_eq!(
                event.properties.get("rootExecutionId").map(String::as_str),
                Some("root-execution")
            );
            assert_eq!(
                event.properties.get("rootCodemodName").map(String::as_str),
                Some("@codemod/root")
            );
            assert_eq!(
                event
                    .properties
                    .get("parentCodemodName")
                    .map(String::as_str),
                Some("@codemod/child-a")
            );
            assert_eq!(
                event.properties.get("dependencyDepth").map(String::as_str),
                Some("2")
            );
            assert_eq!(
                event.properties.get("dependencyPath").map(String::as_str),
                Some("@codemod/child-a > @codemod/child-b")
            );
            assert_eq!(
                event.properties.get("dryRun").map(String::as_str),
                Some("true")
            );
        }

        let executed = events
            .iter()
            .find(|event| event.kind == "codemodExecuted")
            .expect("legacy nested event");
        assert!(!executed.properties.contains_key("fileCount"));

        let completed = events
            .iter()
            .find(|event| event.kind == "codemodRunCompleted")
            .expect("nested completion event");
        assert_eq!(
            completed.properties.get("outcome").map(String::as_str),
            Some("succeeded")
        );
        assert!(!completed.properties.contains_key("filesModified"));
    }

    #[tokio::test]
    async fn nested_failure_emits_failed_completion_without_legacy_success_event() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sender: TelemetrySenderMutex = Arc::new(Box::new(RecordingEventSender {
            events: Arc::clone(&events),
        }));
        let observer = nested_codemod_run_observer(
            sender,
            "root-execution".to_string(),
            "@codemod/root".to_string(),
            false,
        );
        let run = nested_run();

        observer
            .record(NestedCodemodRunEvent::Started(run.clone()))
            .await;
        observer
            .record(NestedCodemodRunEvent::Completed {
                run,
                outcome: NestedCodemodRunOutcome::Failed {
                    error_message: "child failed".to_string(),
                },
                duration_ms: 50,
            })
            .await;

        let events = events.lock().expect("events lock");
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.kind != "codemodExecuted"));
        let completed = events
            .iter()
            .find(|event| event.kind == "codemodRunCompleted")
            .expect("nested completion event");
        assert_eq!(
            completed.properties.get("outcome").map(String::as_str),
            Some("failed")
        );
        assert_eq!(
            completed.properties.get("errorMessage").map(String::as_str),
            Some("child failed")
        );
    }
}
