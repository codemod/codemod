use async_trait::async_trait;

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
