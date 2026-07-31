use std::{future::Future, sync::Arc, time::Instant};

use async_trait::async_trait;
use butterflow_models::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedCodemodRun {
    pub codemod_name: String,
    pub package_version: String,
    pub execution_id: String,
    pub dependency_path: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NestedCodemodRunOutcome {
    Succeeded,
    Failed { error_message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NestedCodemodRunEvent {
    Started(NestedCodemodRun),
    Completed {
        run: NestedCodemodRun,
        outcome: NestedCodemodRunOutcome,
        duration_ms: u128,
    },
}

#[async_trait]
pub trait NestedCodemodRunObserver: Send + Sync {
    async fn record(&self, event: NestedCodemodRunEvent);
}

pub(crate) async fn observe_nested_codemod_run<T, F>(
    observer: Option<&Arc<dyn NestedCodemodRunObserver>>,
    run: NestedCodemodRun,
    operation: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if let Some(observer) = observer {
        observer
            .record(NestedCodemodRunEvent::Started(run.clone()))
            .await;
    }

    let started = Instant::now();
    let result = operation.await;
    let outcome = match &result {
        Ok(_) => NestedCodemodRunOutcome::Succeeded,
        Err(error) => NestedCodemodRunOutcome::Failed {
            error_message: error.to_string(),
        },
    };

    if let Some(observer) = observer {
        observer
            .record(NestedCodemodRunEvent::Completed {
                run,
                outcome,
                duration_ms: started.elapsed().as_millis(),
            })
            .await;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterflow_models::Error;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<NestedCodemodRunEvent>>,
    }

    #[async_trait]
    impl NestedCodemodRunObserver for RecordingObserver {
        async fn record(&self, event: NestedCodemodRunEvent) {
            self.events.lock().expect("events lock").push(event);
        }
    }

    fn test_run() -> NestedCodemodRun {
        NestedCodemodRun {
            codemod_name: "@codemod/react/child".to_string(),
            package_version: "1.2.3".to_string(),
            execution_id: "child-execution".to_string(),
            dependency_path: vec!["@codemod/react/child".to_string()],
        }
    }

    #[tokio::test]
    async fn observer_receives_started_and_successful_completion() {
        let observer = Arc::new(RecordingObserver::default());
        let observer_trait: Arc<dyn NestedCodemodRunObserver> = observer.clone();

        let result = observe_nested_codemod_run(Some(&observer_trait), test_run(), async {
            Ok::<_, Error>(())
        })
        .await;

        assert!(result.is_ok());
        let events = observer.events.lock().expect("events lock");
        assert!(matches!(
            events.as_slice(),
            [
                NestedCodemodRunEvent::Started(_),
                NestedCodemodRunEvent::Completed {
                    outcome: NestedCodemodRunOutcome::Succeeded,
                    ..
                }
            ]
        ));
    }

    #[tokio::test]
    async fn observer_receives_failed_completion_without_swallowing_error() {
        let observer = Arc::new(RecordingObserver::default());
        let observer_trait: Arc<dyn NestedCodemodRunObserver> = observer.clone();

        let result = observe_nested_codemod_run(Some(&observer_trait), test_run(), async {
            Err::<(), _>(Error::Other("child failed".to_string()))
        })
        .await;

        assert!(result.is_err());
        let events = observer.events.lock().expect("events lock");
        assert!(matches!(
            events.first(),
            Some(NestedCodemodRunEvent::Started(_))
        ));
        let Some(NestedCodemodRunEvent::Completed {
            outcome: NestedCodemodRunOutcome::Failed { error_message },
            ..
        }) = events.get(1)
        else {
            panic!("expected failed completion event");
        };
        assert!(error_message.contains("child failed"));
    }

    #[tokio::test]
    async fn recursive_children_each_emit_one_lifecycle_pair() {
        let observer = Arc::new(RecordingObserver::default());
        let observer_trait: Arc<dyn NestedCodemodRunObserver> = observer.clone();
        let mut child = test_run();
        child.codemod_name = "@codemod/react/child-a".to_string();
        child.dependency_path = vec!["@codemod/react/child-a".to_string()];
        let mut grandchild = test_run();
        grandchild.codemod_name = "@codemod/react/child-b".to_string();
        grandchild.dependency_path = vec![
            "@codemod/react/child-a".to_string(),
            "@codemod/react/child-b".to_string(),
        ];

        let result = observe_nested_codemod_run(Some(&observer_trait), child, async {
            observe_nested_codemod_run(Some(&observer_trait), grandchild, async {
                Ok::<_, Error>(())
            })
            .await
        })
        .await;

        assert!(result.is_ok());
        let events = observer.events.lock().expect("events lock");
        assert_eq!(events.len(), 4);
        let paths = events
            .iter()
            .filter_map(|event| match event {
                NestedCodemodRunEvent::Started(run) => Some(run.dependency_path.as_slice()),
                NestedCodemodRunEvent::Completed { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                ["@codemod/react/child-a"].as_slice(),
                ["@codemod/react/child-a", "@codemod/react/child-b"].as_slice(),
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, NestedCodemodRunEvent::Completed { .. }))
                .count(),
            2
        );
    }
}
