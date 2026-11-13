//! Runs workflows as tests: in a directory the test prepared, with steps
//! stopped at (`until`) or given a result (`given`), and nothing persisted.
//! Backs `codemod workflow test` and, through the hidden `workflow harness`
//! command, the `codemod/testing` helpers in the npm package.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use butterflow_core::registry::RegistryClient;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::MetricsData;
use codemod_workflow::events::{CollectEvents, Event};
use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{
    load_workflow, Ending, Engine, MemoryStore, RunReport, Services, StartRequest, Summary,
    TestPlan,
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
    /// Why the run did not finish, for messages.
    pub fn problem(&self) -> Option<String> {
        match &self.report.ending {
            Ending::Succeeded(_) | Ending::Stopped(_) => None,
            Ending::Failed(error) => Some(format!("the run failed: {error}")),
            Ending::Canceled => Some("the run was canceled".into()),
            Ending::AwaitingApproval(message) => {
                Some(format!("the run is waiting for an approval: {message}"))
            }
        }
    }

    /// The result the `codemod/testing` helpers read: the run's report, plus
    /// what the run logged and measured.
    pub fn to_json(&self) -> Value {
        let mut report = self.report.to_json();
        report["logs"] = json!(self.logs);
        report["metrics"] = json!(self.metrics);
        report
    }
}

// ------------------------------------------------------------- trees ---

/// Every file under `dir`, by path relative to it (with `/`), as text.
/// Line endings are normalized to `\n`.
pub fn read_tree(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut tree = BTreeMap::new();
    if dir.exists() {
        read_into(dir, dir, &mut tree)?;
    }
    Ok(tree)
}

fn read_into(root: &Path, dir: &Path, tree: &mut BTreeMap<String, String>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            read_into(root, &path, tree)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(&path)?;
        tree.insert(relative, normalize(&String::from_utf8_lossy(&bytes)));
    }
    Ok(())
}

pub fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Copy a directory, or a single file into `to` under its own name.
pub fn copy_into(from: &Path, to: &Path) -> Result<()> {
    if from.is_file() {
        let name = from
            .file_name()
            .ok_or_else(|| anyhow!("{} has no file name", from.display()))?;
        std::fs::copy(from, to.join(name))?;
        return Ok(());
    }
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_into(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Replace the contents of `dir` with `tree`.
pub fn write_tree(dir: &Path, tree: &BTreeMap<String, String>) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    for (path, text) in tree {
        let path = dir.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
    }
    Ok(())
}
