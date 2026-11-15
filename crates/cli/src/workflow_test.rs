//! Runs workflows as tests: in a directory the test prepared, with steps
//! stopped at (`until`) or given a result (`given`), and nothing persisted.
//! Backs the hidden `workflow harness` command, which the `codemod/testing`
//! helpers in the npm package run for `host: "quickjs"` and YAML workflows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use butterflow_core::registry::RegistryClient;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::MetricsData;
use codemod_workflow::events::{CollectEvents, Event};
use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{
    load_workflow, Engine, MemoryStore, RunReport, Services, StartRequest, Summary, TestPlan,
};
use serde::Serialize;
use serde_json::{json, Map, Value};

pub struct CaseRequest {
    pub workflow: PathBuf,
    /// The directory the run changes.
    pub target: PathBuf,
    pub params: Map<String, Value>,
    pub plan: TestPlan,
    /// Give every approval the workflow asks for.
    pub approve: bool,
    pub capabilities: HashSet<LlrtSupportedModules>,
    pub registry: Option<RegistryClient>,
}

pub struct CaseRun {
    pub report: RunReport,
    pub logs: Vec<LogLine>,
    pub metrics: MetricsData,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    /// Label of the step that logged it; `None` for the workflow body.
    pub step: Option<String>,
    pub line: String,
}

/// Run one test case. The engine runs on its own thread, since its tasks
/// start their own async runtimes.
pub async fn run_case(request: CaseRequest) -> Result<CaseRun> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(run_case_blocking(request));
    });
    receiver
        .await
        .map_err(|_| anyhow!("the workflow engine stopped unexpectedly"))?
}

fn run_case_blocking(request: CaseRequest) -> Result<CaseRun> {
    let workflow = load_workflow(&request.workflow).map_err(|e| anyhow!("{e}"))?;
    let events = Arc::new(CollectEvents::default());
    let summary = Arc::new(Summary::default());
    let engine = Engine::new(
        Arc::new(MemoryStore::default()),
        events.clone(),
        Arc::new(NonInteractive {
            approve_all: request.approve,
        }),
        Services {
            capabilities: request.capabilities,
            registry: request.registry,
            summary: Arc::clone(&summary),
            ..Services::default()
        },
    );
    let report = engine
        .start(StartRequest {
            workflow,
            target: request.target,
            params: Value::Object(request.params),
            name: None,
            dry_run: false,
            test: Some(request.plan),
        })
        .map_err(|e| anyhow!("{e}"))?;
    Ok(CaseRun {
        report,
        logs: logs(events.take()),
        metrics: summary.metrics(),
    })
}

fn logs(events: Vec<Event>) -> Vec<LogLine> {
    let mut labels = HashMap::new();
    let mut lines = Vec::new();
    for event in events {
        match event {
            Event::StepStarted { position, label } => {
                labels.insert(position, label);
            }
            Event::StepLog { position, line } => lines.push(LogLine {
                step: labels.get(&position).cloned(),
                line,
            }),
            Event::RunLog { line } => lines.push(LogLine { step: None, line }),
            _ => {}
        }
    }
    lines
}

impl CaseRun {
    /// The result the `codemod/testing` helpers read: the run's report, plus
    /// what the run logged and measured.
    pub fn to_json(&self) -> Value {
        let mut report = self.report.to_json();
        report["logs"] = json!(self.logs);
        report["metrics"] = json!(self.metrics);
        report
    }
}
