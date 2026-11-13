//! What steps that run JavaScript in their own isolates share: `forEachFile`
//! callbacks and function steps both run in sandbox [`Worker`]s with every
//! `codemod:*` module, the LLRT modules the run's capabilities allow, and
//! relative or package imports resolved from the workflow's directory, plus
//! `codemod:workflow/dependencies` (`registry`, `semver`, the dependency
//! graph) backed by the run's package registry.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use codemod_deps::Registry;
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::sandbox::runtime_module::{RuntimeEvent, RuntimeEventKind};
use codemod_sandbox::{MetricsContext, SharedStateContext, Worker, WorkerOptions};
use rquickjs::async_with;
use serde_json::Value;

use super::TaskContext;
use crate::deps_host::{self, DEPENDENCIES_JS, DEPENDENCIES_MODULE};
use crate::events::Event;
use crate::run::{SEMVER_JS, SEMVER_MODULE};

/// Shared by every worker of one step.
pub(crate) struct StepIsolates {
    pub resolver: Arc<OxcResolver>,
    pub metrics: MetricsContext,
    pub shared_state: SharedStateContext,
    pub registry: Option<Arc<dyn Registry>>,
    initial_state: HashMap<String, Value>,
}

impl StepIsolates {
    pub fn new(task: &TaskContext<'_>) -> Result<Self, String> {
        let resolver = OxcResolver::new(task.run.bundle_dir.clone(), None)
            .map_err(|e| format!("cannot resolve imports from the workflow directory: {e}"))?;
        let initial_state: HashMap<String, Value> = task
            .state
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(Self {
            resolver: Arc::new(resolver),
            metrics: MetricsContext::new(),
            shared_state: SharedStateContext::with_initial_state(initial_state.clone()),
            registry: task.run.services.package_registry.clone(),
            initial_state,
        })
    }

    /// Options for one worker. `console` output and `codemod:runtime`
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
            modules: vec![
                (SEMVER_MODULE.to_string(), SEMVER_JS.to_string()),
                (DEPENDENCIES_MODULE.to_string(), DEPENDENCIES_JS.to_string()),
            ],
            metrics: self.metrics.clone(),
            shared_state: self.shared_state.clone(),
            llm_request_handler: None,
            runtime_event_callback: Some(Arc::new(move |event: RuntimeEvent| {
                if event.kind != RuntimeEventKind::SetCurrentUnit {
                    events.emit(Event::StepLog {
                        position,
                        line: event.message,
                    });
                }
            })),
            cancel: Arc::clone(&run.cancel),
            dry_run: run.dry_run,
            target_directory: run.target.clone(),
            default_language,
        }
    }

    /// State the step's code set or removed (`null`), compared with the
    /// state the step started with.
    pub fn state_changes(&self) -> BTreeMap<String, Value> {
        let mut changes = BTreeMap::new();
        for (key, value) in self.shared_state.get_persistable() {
            if self.initial_state.get(&key) != Some(&value) {
                changes.insert(key, value);
            }
        }
        for key in self.shared_state.get_removals() {
            changes.insert(key, Value::Null);
        }
        changes
    }

    /// Metrics the step's code recorded, also added to the run summary.
    pub fn metrics(&self, task: &TaskContext<'_>) -> Value {
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
/// `codemod:workflow/dependencies`.
pub(crate) async fn start(
    options: WorkerOptions<OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
) -> Result<Worker, String> {
    let worker = Worker::new(options)
        .await
        .map_err(|e| format!("could not start the step's runtime: {e}"))?;
    async_with!(worker.context => |ctx| {
        deps_host::install(&ctx, registry).map_err(|e| e.to_string())
    })
    .await?;
    Ok(worker)
}
