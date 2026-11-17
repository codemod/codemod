//! What steps that run JavaScript in their own isolates share: `forEachFile`
//! callbacks and function steps both run in sandbox [`Worker`]s with every
//! `codemod:*` module, the LLRT modules the run's capabilities allow, and
//! relative or package imports resolved from the workflow's directory, plus
//! `codemod-internal:dependencies` (`registry`, `semver`, the dependency
//! graph) backed by the run's package registry.

use std::sync::Arc;

use codemod_deps::Registry;
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::sandbox::runtime_module::{RuntimeEvent, RuntimeEventKind};
use codemod_sandbox::{MetricsContext, Worker, WorkerOptions};
use rquickjs::async_with;
use serde_json::Value;

use super::TaskContext;
use crate::deps_host::{self, DEPENDENCIES_MODULE};
use crate::events::Event;

/// `codemod/workflow` in isolates.
const ISOLATE_WORKFLOW_MODULE: &str = "codemod-internal:isolate-workflow";

/// Shared by every worker of one step.
pub(crate) struct StepIsolates {
    pub resolver: Arc<OxcResolver>,
    pub metrics: MetricsContext,
    pub registry: Option<Arc<dyn Registry>>,
}

impl StepIsolates {
    pub fn new(task: &TaskContext<'_>) -> Result<Self, String> {
        let resolver = OxcResolver::new(task.run.bundle_dir.clone(), None)
            .map_err(|e| format!("cannot resolve imports from the workflow directory: {e}"))?;
        Ok(Self {
            resolver: Arc::new(resolver),
            metrics: MetricsContext::new(),
            registry: task.run.services.package_registry.clone(),
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
            capabilities: run.services.capabilities.clone(),
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
            ],
            // Steps, callbacks and the files they import may import
            // `codemod/deps`, and `codemod/workflow` for helpers they share
            // with the body.
            codemod_modules: vec![
                ("workflow", ISOLATE_WORKFLOW_MODULE.to_string()),
                ("deps", DEPENDENCIES_MODULE.to_string()),
            ],
            metrics: self.metrics.clone(),
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
        }
    }

    /// What the step's transforms and callbacks returned as `data`, added up;
    /// also added to the run summary.
    pub fn data(&self, task: &TaskContext<'_>) -> Value {
        let metrics = self.metrics.get_all();
        if !metrics.is_empty() {
            task.run.services.summary.merge_metrics(metrics.clone());
        }
        serde_json::to_value(&metrics).unwrap_or(Value::Null)
    }
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
