//! What steps that run JavaScript in their own isolates share: `forEachFile`
//! callbacks and function steps both run in sandbox [`Worker`]s with every
//! `codemod:*` module, the LLRT modules the run's capabilities allow, and
//! relative or package imports resolved from the workflow's directory, plus
//! `codemod-internal:dependencies` (`registry`, `semver`, the dependency
//! graph) backed by the run's package registry.

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use butterflow_core::jssg_runner::SemanticMode;
use codemod_deps::Registry;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::sandbox::runtime_module::{RuntimeEvent, RuntimeEventKind};
use codemod_sandbox::SemanticProvider;
use codemod_sandbox::{Worker, WorkerOptions};
use rquickjs::async_with;
use serde_json::Value;

use super::TaskContext;
use crate::deps_host::{self, DEPENDENCIES_MODULE};
use crate::events::Event;
use crate::interaction::{Approval, Decision};

/// `codemod/workflow` in isolates.
const ISOLATE_WORKFLOW_MODULE: &str = "codemod-internal:isolate-workflow";

/// What the YAML translation's callbacks run jssg transforms with.
pub(crate) const JSSG_COMPAT_MODULE: &str = "codemod-internal:jssg-compat";

/// Shared by every worker of one step.
pub(crate) struct StepIsolates {
    pub resolver: Arc<OxcResolver>,
    pub registry: Option<Arc<dyn Registry>>,
    /// What the step's code may use: the run's capabilities, and those the
    /// step asked for and was given.
    pub capabilities: HashSet<LlrtSupportedModules>,
    /// Semantic analysis for roots `parse(file)` makes.
    pub semantic_provider: Option<Arc<dyn SemanticProvider>>,
}

impl StepIsolates {
    pub fn new(task: &TaskContext<'_>) -> Result<Self, String> {
        let resolver = OxcResolver::new(task.run.bundle_dir.clone(), None)
            .map_err(|e| format!("cannot resolve imports from the workflow directory: {e}"))?;
        Ok(Self {
            resolver: Arc::new(resolver),
            registry: task.run.services.package_registry.clone(),
            capabilities: task.run.services.capabilities.clone(),
            semantic_provider: None,
        })
    }

    /// Options for one worker. `console` output and `codemod/runtime`
    /// progress become the step's log lines.
    pub fn worker_options(
        &self,
        task: &TaskContext<'_>,
        default_language: Option<String>,
    ) -> WorkerOptions<OxcResolver> {
        let run = task.run;
        let events = Arc::clone(&run.events);
        let position = task.position;
        WorkerOptions {
            capabilities: self.capabilities.clone(),
            resolver: Arc::clone(&self.resolver),
            // The callback prelude takes its helpers from the orchestrator;
            // `registry` and `semver` come from the dependencies module.
            modules: vec![
                (
                    crate::run::ORCHESTRATOR_MODULE.to_string(),
                    crate::js::orchestrator().to_string(),
                ),
                (
                    crate::run::SEMVER_MODULE.to_string(),
                    crate::js::semver_module().to_string(),
                ),
                (
                    DEPENDENCIES_MODULE.to_string(),
                    crate::js::dependencies_module().to_string(),
                ),
                (
                    ISOLATE_WORKFLOW_MODULE.to_string(),
                    crate::js::isolate_workflow_module().to_string(),
                ),
                (
                    JSSG_COMPAT_MODULE.to_string(),
                    crate::js::jssg_compat_module().to_string(),
                ),
            ],
            // Steps, callbacks and the files they import may import
            // `codemod/deps`, and `codemod/workflow` for helpers they share
            // with the body.
            codemod_modules: vec![
                ("workflow", ISOLATE_WORKFLOW_MODULE.to_string()),
                ("deps", DEPENDENCIES_MODULE.to_string()),
            ],
            llm_request_handler: None,
            runtime_event_callback: Some(Arc::new(move |event: RuntimeEvent| {
                if event.kind != RuntimeEventKind::SetCurrentUnit {
                    events.emit(Event::StepLog {
                        position,
                        line: butterflow_core::jssg_runner::format_runtime_event(&event),
                    });
                }
            })),
            cancel: Arc::clone(&task.cancel),
            dry_run: run.dry_run,
            target_directory: run.target.clone(),
            default_language,
            semantic_provider: self.semantic_provider.clone(),
        }
    }
}

/// The capabilities a step's code gets: the run's, and the ones it asks for
/// that the run does not grant, when the person running it approves them.
/// What is not approved is logged, and code that needs it fails. In a dry
/// run, workers install neither `fs` nor `child_process` whatever is granted.
pub(crate) fn capabilities(
    task: &TaskContext<'_>,
    requested: &[String],
) -> Result<HashSet<LlrtSupportedModules>, String> {
    let granted = &task.run.services.capabilities;
    let mut result = granted.clone();
    let mut missing = Vec::new();
    for name in requested {
        let module = LlrtSupportedModules::from_str(name).map_err(|_| {
            format!("unknown capability '{name}': use \"fs\", \"fetch\" or \"child_process\"")
        })?;
        if !granted.contains(&module) {
            missing.push((name.clone(), module));
        }
    }
    if missing.is_empty() {
        return Ok(result);
    }
    let names: Vec<String> = missing.iter().map(|(name, _)| name.clone()).collect();
    match task.run.interaction.approve(&Approval::Capabilities {
        modules: names.clone(),
    }) {
        Decision::Approved => result.extend(missing.into_iter().map(|(_, module)| module)),
        Decision::Denied | Decision::Deferred => {
            task.log(format!("capabilities not granted: {}", names.join(", ")))
        }
    }
    Ok(result)
}

/// The semantic analysis a `semanticAnalysis` option asks for: `"file"`
/// (the default), `"workspace"` (the whole target, or `{ mode, root }`), or
/// `"none"`.
pub(crate) fn semantic_mode(
    task: &TaskContext<'_>,
    option: Option<&Value>,
) -> Result<SemanticMode, String> {
    let target = &task.run.target;
    Ok(match option {
        None => SemanticMode::File,
        Some(Value::String(mode)) if mode == "none" => SemanticMode::None,
        Some(Value::String(mode)) if mode == "file" => SemanticMode::File,
        Some(Value::String(mode)) if mode == "workspace" => SemanticMode::Workspace(target.clone()),
        Some(Value::Object(detail)) => match detail.get("mode").and_then(Value::as_str) {
            Some("none") => SemanticMode::None,
            Some("file") => SemanticMode::File,
            Some("workspace") => SemanticMode::Workspace(
                detail
                    .get("root")
                    .and_then(Value::as_str)
                    .map(|root| target.join(root))
                    .unwrap_or_else(|| target.clone()),
            ),
            _ => return Err("semanticAnalysis.mode must be 'none', 'file' or 'workspace'".into()),
        },
        Some(other) => {
            return Err(format!(
                "semanticAnalysis must be 'file', 'workspace', 'none' or an object, not {other}"
            ))
        }
    })
}

/// A name for an in-memory module that resolves relative imports against
/// the workflow's directory.
pub(crate) fn module_name(task: &TaskContext<'_>, id: &str) -> String {
    task.run
        .bundle_dir
        .join(format!("__{id}.js"))
        .to_string_lossy()
        .into_owned()
}

/// Start one isolate, with `__dependencyHost` installed for
/// `codemod-internal:dependencies`.
pub(crate) async fn start(
    options: WorkerOptions<OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
) -> Result<Worker, String> {
    let cancel = Arc::clone(&options.cancel);
    let worker = Worker::new(options)
        .await
        .map_err(|e| format!("could not start the step's runtime: {e}"))?;
    async_with!(worker.context => |ctx| {
        deps_host::install(&ctx, registry, cancel).map_err(|e| e.to_string())
    })
    .await?;
    Ok(worker)
}
