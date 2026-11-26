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
            // A test asks the decision model only when it is set up in the
            // environment; a missing key fails the call that needs it.
            decision: crate::assess_preflight::test_decision_model(&workflow.path),
            ..Services::default()
        },
    );
    let report = engine
        .start(StartRequest {
            run_id: None,
            workflow,
            target: request.target,
            params: Value::Object(request.params),
            name: None,
            dry_run: false,
            test: Some(request.plan),
            check_uncommitted: false,
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
    // A nested codemod's steps log as their parent step's.
    for event in events.into_iter().filter_map(Event::top_level) {
        match event {
            Event::StepStarted { at, label, .. } => {
                labels.insert(at.position, label);
            }
            Event::StepLog { at, line } => lines.push(LogLine {
                step: labels.get(&at.position).cloned(),
                line,
            }),
            Event::RunLog { line, .. } => lines.push(LogLine { step: None, line }),
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

    if !request.callback.is_file() {
        anyhow::bail!("no callback file at {}", request.callback.display());
    }
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
    // What each file's callback did, applied once every file has run, so
    // none sees another's changes or new name.
    let mut planned: Vec<(String, PathBuf, codemod_workflow::CallbackFileRun)> = Vec::new();
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
                if let Some(data) = &run.output {
                    outputs.push(json!({ "file": path, "data": data }));
                }
                planned.push((path, file, run));
            }
        }
    }
    // Every rename must land inside the target, also through symlinked
    // directories, before anything is changed.
    let mut refused = Vec::new();
    for (path, _, run) in &planned {
        if let Some(to) = &run.renamed {
            if let Err(error) = inside_target(&target, &target.join(to)) {
                refused.push(
                    json!({ "path": path, "error": format!("cannot rename to '{to}': {error}") }),
                );
            }
        }
    }
    if refused.is_empty() {
        let mut moves = Vec::new();
        for (path, file, run) in &planned {
            let applied = (|| -> std::io::Result<()> {
                if run.deleted {
                    return std::fs::remove_file(file);
                }
                if let Some(text) = &run.written {
                    std::fs::write(file, text)?;
                }
                if let Some(to) = &run.renamed {
                    moves.push((path, file, target.join(to)));
                }
                Ok(())
            })();
            if let Err(error) = applied {
                failures.push(
                    json!({ "path": path, "error": format!("cannot apply its changes: {error}") }),
                );
            }
        }
        for (path, from, to) in moves {
            let moved = to
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::rename(from, &to));
            if let Err(error) = moved {
                failures
                    .push(json!({ "path": path, "error": format!("cannot rename it: {error}") }));
            }
        }
    } else {
        failures.extend(refused);
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

/// Whether `path` is inside `target` once symlinks are followed: its nearest
/// existing ancestor is, and is a directory.
fn inside_target(target: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    let mut existing = path.parent();
    while let Some(dir) = existing {
        if dir.exists() {
            break;
        }
        existing = dir.parent();
    }
    let Some(dir) = existing else {
        return Err("the path is outside the target".into());
    };
    let resolved = dunce::canonicalize(dir).map_err(|e| e.to_string())?;
    if !resolved.starts_with(target) {
        return Err("the path is outside the target".into());
    }
    if !resolved.is_dir() {
        return Err(format!("'{}' is a file", dir.display()));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::inside_target;

    #[test]
    fn a_rename_through_a_symlinked_directory_is_outside_the_target() {
        let target = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let target_dir = dunce::canonicalize(target.path()).unwrap();
        std::os::unix::fs::symlink(outside.path(), target_dir.join("out")).unwrap();
        std::fs::write(target_dir.join("file"), "x").unwrap();
        assert!(inside_target(&target_dir, &target_dir.join("lib/new.ts")).is_ok());
        assert!(inside_target(&target_dir, &target_dir.join("out/new.ts")).is_err());
        assert!(inside_target(&target_dir, &target_dir.join("out/deeper/new.ts")).is_err());
        let error = inside_target(&target_dir, &target_dir.join("file/new.ts")).unwrap_err();
        assert!(error.contains("is a file"), "{error}");
    }
}
