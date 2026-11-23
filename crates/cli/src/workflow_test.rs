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
    /// what the run logged.
    pub fn to_json(&self) -> Value {
        let mut report = self.report.to_json();
        report["logs"] = json!(self.logs);
        report
    }
}

/// One `codemod/testing` `runCallback` case: a forEachFile callback file run
/// on every file of `target` in `language`, as `codemod jssg test --callback`
/// runs it, with what it does applied to `target`.
pub struct CallbackCaseRequest {
    pub callback: PathBuf,
    pub target: PathBuf,
    pub language: String,
    /// `"file"` (the default), `"workspace"` or `"none"`.
    pub semantic_analysis: Option<String>,
    pub capabilities: HashSet<LlrtSupportedModules>,
}

/// Run a callback case. The result has the shape of a workflow run's (with
/// no steps), plus `outputs` (`[{ file, data }]`) and `failures`
/// (`[{ path, error }]`); it failed when any file did.
pub async fn run_callback_case(request: CallbackCaseRequest) -> Result<Value> {
    use codemod_sandbox::sandbox::engine::language_data::get_extensions_for_language;
    use codemod_sandbox::sandbox::resolvers::OxcResolver;
    use codemod_sandbox::utils::project_discovery::find_tsconfig;
    use codemod_sandbox::CodemodLang;
    use language_core::SemanticProvider;
    use semantic_factory::LazySemanticProvider;
    use std::sync::Mutex;

    let language: CodemodLang = request.language.parse().map_err(|e: String| anyhow!(e))?;
    let extensions = get_extensions_for_language(language);
    let target = dunce::canonicalize(&request.target)?;
    let mut files: Vec<(String, PathBuf)> = walkdir::WalkDir::new(&target)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy();
            extensions.iter().any(|extension| name.ends_with(extension))
        })
        .filter_map(|entry| {
            let path = entry.path().strip_prefix(&target).ok()?;
            Some((
                path.to_string_lossy().replace('\\', "/"),
                entry.path().to_path_buf(),
            ))
        })
        .collect();
    files.sort();

    let semantic_provider: Option<Arc<dyn SemanticProvider>> = match request
        .semantic_analysis
        .as_deref()
    {
        None | Some("file") => Some(Arc::new(LazySemanticProvider::file_scope())),
        Some("none") => None,
        Some("workspace") => {
            let provider: Arc<dyn SemanticProvider> =
                Arc::new(LazySemanticProvider::workspace_scope(target.clone()));
            for (_, file) in &files {
                if let Ok(text) = std::fs::read_to_string(file) {
                    let _ = provider.notify_file_processed(file, &text);
                }
            }
            Some(provider)
        }
        Some(other) => {
            anyhow::bail!("semanticAnalysis must be 'file', 'workspace' or 'none', not '{other}'")
        }
    };
    let script_dir = request
        .callback
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let tsconfig = find_tsconfig(&script_dir);
    let resolver = Arc::new(OxcResolver::new(script_dir, tsconfig)?);
    let logs = Arc::new(Mutex::new(Vec::<LogLine>::new()));

    let mut outputs = Vec::new();
    let mut failures = Vec::new();
    let mut moves = Vec::new();
    for (path, file) in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let sink = Arc::clone(&logs);
        let run = codemod_workflow::run_callback_file(codemod_workflow::CallbackFileRequest {
            callback: request.callback.clone(),
            resolver: Arc::clone(&resolver),
            target: target.clone(),
            path: path.clone(),
            text,
            language: Some(request.language.clone()),
            semantic_provider: semantic_provider.clone(),
            capabilities: request.capabilities.clone(),
            runtime_event_callback: Some(Arc::new(move |event| {
                if event.kind
                    != codemod_sandbox::sandbox::runtime_module::RuntimeEventKind::SetCurrentUnit
                {
                    if let Ok(mut logs) = sink.lock() {
                        logs.push(LogLine {
                            step: None,
                            line: butterflow_core::jssg_runner::format_runtime_event(&event),
                        });
                    }
                }
            })),
        })
        .await;
        match run {
            Err(error) => failures.push(json!({ "path": path, "error": error })),
            Ok(run) => {
                if let Some(data) = run.output {
                    outputs.push(json!({ "file": path, "data": data }));
                }
                if run.deleted {
                    std::fs::remove_file(&file)?;
                    continue;
                }
                if let Some(text) = run.written {
                    std::fs::write(&file, text)?;
                }
                if let Some(to) = run.renamed {
                    moves.push((file, target.join(to)));
                }
            }
        }
    }
    // After every file ran, so none sees another's new name.
    for (from, to) in moves {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(from, to)?;
    }

    let error = failures.first().map(|failure| {
        format!(
            "the callback failed on {}: {}",
            failure["path"].as_str().unwrap_or_default(),
            failure["error"].as_str().unwrap_or_default()
        )
    });
    let logs = std::mem::take(&mut *logs.lock().map_err(|_| anyhow!("log lock poisoned"))?);
    Ok(json!({
        "status": if error.is_some() { "failed" } else { "succeeded" },
        "output": null,
        "error": error,
        "awaiting": null,
        "stoppedAt": null,
        "steps": [],
        "logs": logs,
        "outputs": outputs,
        "failures": failures,
    }))
}
