#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use codemod_workflow::events::CollectEvents;
use codemod_workflow::interaction::{Interaction, NonInteractive};
use codemod_workflow::{
    load_workflow, Ending, Engine, LoadedWorkflow, MemoryStore, RunReport, RunStore, Services,
    StartRequest,
};
use serde_json::Value;
use tempfile::TempDir;

/// A target directory, a directory for the workflow file, and an engine.
pub struct Harness {
    pub target: TempDir,
    pub bundle: TempDir,
    pub store: Arc<dyn RunStore>,
    pub events: Arc<CollectEvents>,
    pub engine: Engine,
}

impl Harness {
    pub fn new() -> Self {
        Self::with(
            Arc::new(MemoryStore::default()),
            Arc::new(NonInteractive { approve_all: false }),
        )
    }

    pub fn with(store: Arc<dyn RunStore>, interaction: Arc<dyn Interaction>) -> Self {
        Self::with_services(store, interaction, Services::default())
    }

    pub fn with_services(
        store: Arc<dyn RunStore>,
        interaction: Arc<dyn Interaction>,
        services: Services,
    ) -> Self {
        let events = Arc::new(CollectEvents::default());
        let engine = Engine::new(Arc::clone(&store), events.clone(), interaction, services);
        Self {
            target: TempDir::new().unwrap(),
            bundle: TempDir::new().unwrap(),
            store,
            events,
            engine,
        }
    }

    pub fn write(&self, path: &str, text: &str) {
        write(self.target.path(), path, text);
    }

    pub fn read(&self, path: &str) -> String {
        fs::read_to_string(self.target.path().join(path)).unwrap()
    }

    pub fn exists(&self, path: &str) -> bool {
        self.target.path().join(path).exists()
    }

    /// Write a file next to the workflow (scripts, transforms, rules).
    pub fn bundle_file(&self, path: &str, text: &str) {
        write(self.bundle.path(), path, text);
    }

    pub fn workflow(&self, source: &str) -> LoadedWorkflow {
        self.workflow_file("workflow.ts", source)
    }

    pub fn workflow_file(&self, name: &str, source: &str) -> LoadedWorkflow {
        let path = self.bundle.path().join(name);
        fs::write(&path, source).unwrap();
        load_workflow(&path).unwrap_or_else(|e| panic!("load failed: {e}"))
    }

    pub fn start(&self, source: &str, params: Value) -> RunReport {
        self.start_loaded(self.workflow(source), params, false)
    }

    pub fn dry_run(&self, source: &str) -> RunReport {
        self.start_loaded(self.workflow(source), Value::Null, true)
    }

    pub fn start_loaded(
        &self,
        workflow: LoadedWorkflow,
        params: Value,
        dry_run: bool,
    ) -> RunReport {
        self.engine
            .start(StartRequest {
                workflow,
                target: self.target.path().to_path_buf(),
                params,
                name: None,
                dry_run,
            })
            .unwrap()
    }

    /// Optionally change the workflow, then resume the run.
    pub fn resume(&self, report: &RunReport, new_source: Option<&str>) -> RunReport {
        if let Some(source) = new_source {
            fs::write(&report.run.workflow_path, source).unwrap();
        }
        self.engine.resume(&report.run.id).unwrap()
    }
}

pub fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

/// The workflow's return value, or its error.
pub fn output(report: &RunReport) -> Result<Value, String> {
    match &report.ending {
        Ending::Succeeded(value) => Ok(value.clone()),
        Ending::Failed(error) => Err(error.clone()),
        other => Err(format!("{other:?}")),
    }
}
