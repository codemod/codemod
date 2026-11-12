//! The public entry point: start, resume and cancel runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use butterflow_state::run_store::{
    MemoryStore, RunRecord, RunStatus, RunStore, StepRecord, StoreError,
};
use chrono::Utc;
use serde_json::Value;

use crate::build::{build, BuildError, Built};
use crate::events::{Event, EventSink};
use crate::interaction::Interaction;
use crate::run::{execute, Ending, RunContext, RunError};
use crate::Services;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("{0}")]
    Compat(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("run '{0}' not found")]
    NotFound(String),
    #[error("run '{0}' already finished; start a new run instead")]
    AlreadyFinished(String),
}

/// A workflow ready to run.
#[derive(Debug, Clone)]
pub struct LoadedWorkflow {
    pub path: PathBuf,
    pub built: Built,
}

/// Read and build a workflow file: `.ts`/`.mts`, or `.yaml`/`.yml`/`.json`
/// through the compatibility layer.
pub fn load_workflow(path: &Path) -> Result<LoadedWorkflow, EngineError> {
    let source = std::fs::read_to_string(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let file = path.display().to_string();
    let typescript = if is_typescript(path) {
        source
    } else {
        crate::compat::translate_source(&source, path)
            .map_err(EngineError::Compat)?
            .source
    };
    Ok(LoadedWorkflow {
        path: path.to_path_buf(),
        built: build(&file, &typescript)?,
    })
}

pub fn is_typescript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts" | "mts")
    )
}

pub struct StartRequest {
    pub workflow: LoadedWorkflow,
    pub target: PathBuf,
    pub params: Value,
    pub name: Option<String>,
    /// Run every step but change nothing on disk and persist nothing.
    pub dry_run: bool,
}

/// What a run ended as.
#[derive(Debug, Clone)]
pub struct RunReport {
    pub run: RunRecord,
    pub ending: Ending,
    /// Every recorded step, ordered by position.
    pub steps: Vec<StepRecord>,
}

pub struct Engine {
    store: Arc<dyn RunStore>,
    events: Arc<dyn EventSink>,
    interaction: Arc<dyn Interaction>,
    services: Arc<Services>,
    /// Set to stop the run in progress (Ctrl-C).
    cancel: Arc<AtomicBool>,
    current: Mutex<Option<String>>,
}

impl Engine {
    pub fn new(
        store: Arc<dyn RunStore>,
        events: Arc<dyn EventSink>,
        interaction: Arc<dyn Interaction>,
        services: Services,
    ) -> Self {
        Self {
            store,
            events,
            interaction,
            services: Arc::new(services),
            cancel: Arc::new(AtomicBool::new(false)),
            current: Mutex::new(None),
        }
    }

    pub fn store(&self) -> &Arc<dyn RunStore> {
        &self.store
    }

    /// A flag that stops the run in progress when set.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    pub fn start(&self, request: StartRequest) -> Result<RunReport, EngineError> {
        let record = RunRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: request.name,
            workflow_path: absolute(&request.workflow.path),
            target: absolute(&request.target),
            params: request.params,
            status: RunStatus::Running,
            started_at: Utc::now(),
            ended_at: None,
            awaiting: None,
            error: None,
            output: None,
        };
        let store: Arc<dyn RunStore> = if request.dry_run {
            Arc::new(MemoryStore::default())
        } else {
            Arc::clone(&self.store)
        };
        store.create_run(&record)?;
        self.drive(store, record, &request.workflow, request.dry_run, false)
    }

    /// Continue a run that failed, was canceled, or waits for an approval.
    pub fn resume(&self, run_id: &str) -> Result<RunReport, EngineError> {
        let mut record = self
            .store
            .get_run(run_id)?
            .ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
        if record.status == RunStatus::Succeeded {
            return Err(EngineError::AlreadyFinished(run_id.to_string()));
        }
        let workflow = load_workflow(&record.workflow_path)?;
        record.status = RunStatus::Running;
        record.ended_at = None;
        record.awaiting = None;
        record.error = None;
        self.store.update_run(&record)?;
        self.drive(Arc::clone(&self.store), record, &workflow, false, true)
    }

    /// Ask a run, possibly in another process, to stop.
    pub fn cancel(&self, run_id: &str) -> Result<(), EngineError> {
        let record = self
            .store
            .get_run(run_id)?
            .ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
        if self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            == Some(run_id)
        {
            self.cancel.store(true, Ordering::SeqCst);
        }
        if record.status == RunStatus::AwaitingApproval {
            // Nothing is running; mark it directly.
            let mut record = record;
            record.status = RunStatus::Canceled;
            record.ended_at = Some(Utc::now());
            record.awaiting = None;
            self.store.update_run(&record)?;
            return Ok(());
        }
        self.store.request_cancel(run_id)?;
        Ok(())
    }

    fn drive(
        &self,
        store: Arc<dyn RunStore>,
        mut record: RunRecord,
        workflow: &LoadedWorkflow,
        dry_run: bool,
        resumed: bool,
    ) -> Result<RunReport, EngineError> {
        self.cancel.store(false, Ordering::SeqCst);
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(record.id.clone());
        self.events.emit(Event::RunStarted {
            run_id: record.id.clone(),
            workflow: record.workflow_path.clone(),
            resumed,
        });

        let context = Arc::new(RunContext {
            run_id: record.id.clone(),
            target: record.target.clone(),
            bundle_dir: record
                .workflow_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            params: record.params.clone(),
            dry_run,
            callbacks: workflow.built.callbacks.clone(),
            store: Arc::clone(&store),
            events: Arc::clone(&self.events),
            interaction: Arc::clone(&self.interaction),
            services: Arc::clone(&self.services),
            cancel: Arc::clone(&self.cancel),
            commit_lock: Mutex::new(()),
        });
        let ending = execute(&workflow.built, context);
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let ending = ending?;

        match &ending {
            Ending::Succeeded(output) => {
                record.status = RunStatus::Succeeded;
                record.output = Some(output.clone());
            }
            Ending::Failed(error) => {
                record.status = RunStatus::Failed;
                record.error = Some(error.clone());
            }
            Ending::Canceled => record.status = RunStatus::Canceled,
            Ending::AwaitingApproval(message) => {
                record.status = RunStatus::AwaitingApproval;
                record.awaiting = Some(message.clone());
            }
        }
        if record.status.is_finished() {
            record.ended_at = Some(Utc::now());
        }
        store.update_run(&record)?;
        self.events.emit(Event::RunFinished {
            run_id: record.id.clone(),
            status: record.status,
            error: record.error.clone(),
        });
        let steps = store.load_steps(&record.id)?;
        Ok(RunReport {
            run: record,
            ending,
            steps,
        })
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
