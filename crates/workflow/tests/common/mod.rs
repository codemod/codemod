#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use codemod_workflow::events::CollectEvents;
use codemod_workflow::interaction::{Approval, Decision, Interaction, NonInteractive};
use codemod_workflow::{
    load_workflow, Ending, Engine, LoadedWorkflow, MemoryStore, RunReport, RunStore, Services,
    StartRequest, TestPlan,
};
use serde_json::Value;
use tempfile::TempDir;

/// A target directory, a directory for the workflow file, and an engine.
pub struct Harness {
    pub summary: Arc<codemod_workflow::Summary>,
    pub target: TempDir,
    pub bundle: TempDir,
    pub store: Arc<dyn RunStore>,
    pub events: Arc<CollectEvents>,
    pub engine: Engine,
}

/// These fixtures are ours: what their steps run and send is approved
/// (shell commands, agents, requests, pull requests). Manual approvals stay
/// unanswered, so tests can pause and resume at them. Tests of what an
/// unattended run refuses use [`Harness::unattended`].
pub struct FixtureInteraction;

impl Interaction for FixtureInteraction {
    fn is_interactive(&self) -> bool {
        false
    }

    fn approve(&self, approval: &Approval) -> Decision {
        match approval {
            Approval::ShellCommand { .. }
            | Approval::PullRequest { .. }
            | Approval::Agent { .. }
            | Approval::HttpRequest { .. } => Decision::Approved,
            _ => approval.unanswered(),
        }
    }
}

impl Harness {
    pub fn new() -> Self {
        Self::with(
            Arc::new(MemoryStore::default()),
            Arc::new(FixtureInteraction),
        )
    }

    /// A run nobody answers for, without `--approve`.
    pub fn unattended() -> Self {
        Self::with(
            Arc::new(MemoryStore::default()),
            Arc::new(NonInteractive { approve_all: false }),
        )
    }

    /// As [`Self::new`], with every approval given (`--approve`): for steps
    /// that ask before they run (`shell`, `agent`, `http`, `pullRequest`).
    pub fn approving() -> Self {
        Self::with(
            Arc::new(MemoryStore::default()),
            Arc::new(NonInteractive { approve_all: true }),
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
        let summary = Arc::clone(&services.summary);
        let engine = Engine::new(Arc::clone(&store), events.clone(), interaction, services);
        Self {
            summary,
            target: TempDir::new().unwrap(),
            bundle: TempDir::new().unwrap(),
            store,
            events,
            engine,
        }
    }

    pub fn engine_summary_diffs(&self) -> usize {
        self.summary.diffs().len()
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
                run_id: None,
                workflow,
                target: self.target.path().to_path_buf(),
                params,
                name: None,
                dry_run,
                test: None,
                check_uncommitted: false,
            })
            .unwrap()
    }

    /// Run as a test, with steps stopped at or stubbed by `plan`.
    pub fn test(&self, source: &str, plan: TestPlan) -> RunReport {
        self.engine
            .start(StartRequest {
                run_id: None,
                workflow: self.workflow(source),
                target: self.target.path().to_path_buf(),
                params: Value::Null,
                name: None,
                dry_run: false,
                test: Some(plan),
                check_uncommitted: false,
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
