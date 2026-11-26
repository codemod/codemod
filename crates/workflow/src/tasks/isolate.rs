//! What steps that run JavaScript in their own isolates share: `forEachFile`
//! callbacks and function steps both run in sandbox [`Worker`]s with every
//! `codemod:*` module, the safe LLRT modules, and relative or package
//! imports resolved from the workflow's directory, plus
//! `codemod-internal:dependencies` (`registry`, `semver`, the dependency
//! graph) backed by the run's package registry.
//!
//! Callbacks are pure: they change the current file through the file API
//! and read the target through a read-only `fs`; the network, processes,
//! other writes, environment changes and timers fail with the declared step
//! to use instead (`http()`, `shell()`, `sleep()`). Capabilities granted to
//! the run (`--allow-*`, codemod.yaml) no longer reach them.

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
use rquickjs::{async_with, CatchResultExt};
use serde_json::Value;

use super::TaskContext;
use crate::deps_host::{self, DEPENDENCIES_MODULE};
use crate::events::Event;

/// `codemod/workflow` in isolates.
const ISOLATE_WORKFLOW_MODULE: &str = "codemod-internal:isolate-workflow";
const ISOLATE_GIT_MODULE: &str = "codemod-internal:isolate-git";
/// `codemod/assess` in isolates and the workflow body.
pub(crate) const ASSESS_MODULE: &str = "codemod-internal:assess";
/// The globals of steps' and callbacks' isolates (`js/callback_globals.ts`).
const CALLBACK_GLOBALS_MODULE: &str = "codemod-internal:callback-globals";

/// What a write through `fs` in a callback says; `{name}` is the function
/// called. The same words as `unavailable("fs.…", "callback")` in
/// `js/orchestrator.ts`, which the Node.js host throws.
pub(crate) const READ_ONLY_FS: &str = "{name} is not available in workflow callbacks (function steps, forEachFile callbacks and reducers): change the current file with file.write(), file.rename() or file.delete(), and anything else with a step such as shell() or agent()";

/// What the YAML translation's callbacks run jssg transforms with.
pub(crate) const JSSG_COMPAT_MODULE: &str = "codemod-internal:jssg-compat";

/// Codemod 1.x compatibility (js/v1.ts): `codemod:metrics` in runs of a
/// workflow.yaml.
const V1_MODULE: &str = "codemod-internal:v1";
/// `codemod:workflow` in runs of a workflow.yaml: the isolates' module and
/// 1.x's state functions.
const V1_WORKFLOW_MODULE: &str = "codemod-internal:v1-workflow";
const V1_WORKFLOW_SOURCE: &str = "export * from \"codemod-internal:isolate-workflow\";\nexport { getState, setState, unsetState, acquireLock, setStepOutput, getStepOutput, getOrSetStepOutput } from \"codemod-internal:v1\";\n";

/// Shared by every worker of one step.
pub(crate) struct StepIsolates {
    pub resolver: Arc<OxcResolver>,
    pub registry: Option<Arc<dyn Registry>>,
    /// Semantic analysis for roots `parse(file)` makes.
    pub semantic_provider: Option<Arc<dyn SemanticProvider>>,
    /// The run is of a workflow.yaml: its code gets the codemod 1.x
    /// modules (see [`isolate_codemod_modules`]).
    pub v1: bool,
}

impl StepIsolates {
    pub fn new(task: &TaskContext<'_>) -> Result<Self, String> {
        let resolver = OxcResolver::new(task.run.bundle_dir.clone(), None)
            .map_err(|e| format!("cannot resolve imports from the workflow directory: {e}"))?;
        Ok(Self {
            resolver: Arc::new(resolver),
            registry: task.run.services.package_registry.clone(),
            semantic_provider: None,
            v1: task.run.v1_compat(),
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
        let notices = Arc::clone(run);
        let at = task.at();
        WorkerOptions {
            // Callbacks get no unsafe LLRT module, whatever the run grants.
            capabilities: HashSet::new(),
            resolver: Arc::clone(&self.resolver),
            modules: isolate_modules(),
            codemod_modules: isolate_codemod_modules(self.v1),
            llm_request_handler: None,
            runtime_event_callback: Some(Arc::new(move |event: RuntimeEvent| match event.kind {
                RuntimeEventKind::Experimental(api) => notices.experimental(api),
                RuntimeEventKind::SetCurrentUnit => {}
                _ => notices.events.emit(Event::StepLog {
                    at: at.clone(),
                    line: butterflow_core::jssg_runner::format_runtime_event(&event),
                }),
            })),
            cancel: Arc::clone(&task.cancel),
            dry_run: run.dry_run,
            target_directory: run.target.clone(),
            default_language,
            semantic_provider: self.semantic_provider.clone(),
            // In a dry run, `jssgTransform` starts from what earlier steps
            // would have left.
            pending_files: run
                .overlay
                .as_ref()
                .map(super::overlay::Overlay::pending_files),
            read_only_fs: Some(READ_ONLY_FS.to_string()),
        }
    }
}

/// The in-memory modules every isolate has. The callback prelude takes its
/// helpers from the orchestrator; `registry` and `semver` come from the
/// dependencies module.
pub(crate) fn isolate_modules() -> Vec<(String, String)> {
    vec![
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
            ISOLATE_GIT_MODULE.to_string(),
            crate::js::isolate_git_module().to_string(),
        ),
        (
            JSSG_COMPAT_MODULE.to_string(),
            crate::js::jssg_compat_module().to_string(),
        ),
        (
            ASSESS_MODULE.to_string(),
            crate::js::assess_module().to_string(),
        ),
        (
            CALLBACK_GLOBALS_MODULE.to_string(),
            crate::js::callback_globals_module().to_string(),
        ),
        (
            "child_process".to_string(),
            crate::js::child_process_module().to_string(),
        ),
        (
            "node:child_process".to_string(),
            crate::js::child_process_module().to_string(),
        ),
        (V1_MODULE.to_string(), crate::js::v1_module().to_string()),
        (
            V1_WORKFLOW_MODULE.to_string(),
            V1_WORKFLOW_SOURCE.to_string(),
        ),
    ]
}

/// Steps, callbacks and the files they import may import `codemod/deps`
/// and `codemod/assess`, and `codemod/workflow` and `codemod/git` for
/// helpers they share with the body. With `v1` (a run of a workflow.yaml,
/// a codemod 1.x package), also what 1.x transforms imported that 2.0
/// removed: `codemod:metrics`, and the state functions of
/// `codemod:workflow`. TypeScript workflows never get them.
pub(crate) fn isolate_codemod_modules(v1: bool) -> Vec<(&'static str, String)> {
    let workflow = if v1 {
        V1_WORKFLOW_MODULE
    } else {
        ISOLATE_WORKFLOW_MODULE
    };
    let mut modules = vec![
        ("workflow", workflow.to_string()),
        ("git", ISOLATE_GIT_MODULE.to_string()),
        ("deps", DEPENDENCIES_MODULE.to_string()),
        ("assess", ASSESS_MODULE.to_string()),
    ];
    if v1 {
        modules.push(("metrics", V1_MODULE.to_string()));
    }
    modules
}

/// Capabilities a `forEachFile` selector (or a translated workflow.yaml
/// step) asks for: callbacks no longer get them, which the step's log says.
/// An unknown name is still an error.
pub(crate) fn ignore_capabilities(
    task: &TaskContext<'_>,
    requested: &[String],
) -> Result<(), String> {
    for name in requested {
        LlrtSupportedModules::from_str(name).map_err(|_| {
            format!("unknown capability '{name}': use \"fs\", \"fetch\" or \"child_process\"")
        })?;
    }
    if !requested.is_empty() {
        task.log(format!(
            "capabilities ({}) are no longer given to callbacks: they read the target through a read-only fs; use step(http(...)) for the network and step(shell(...)) for commands",
            requested.join(", ")
        ));
    }
    Ok(())
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

/// How long an isolate waits, once its code is done, for work the code left
/// running (an unawaited promise; timers are cleared) before it is freed;
/// see [`Worker::shutdown`].
pub(crate) const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Start one isolate, with `__dependencyHost` installed for
/// `codemod-internal:dependencies`, and `__assessHost` for `codemod/assess`
/// when the step may assess (`forEachFile` callbacks and function steps).
pub(crate) async fn start(
    options: WorkerOptions<OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
    assess: Option<crate::assess::AssessHandle>,
) -> Result<Worker, String> {
    let cancel = Arc::clone(&options.cancel);
    let worker = Worker::new(options)
        .await
        .map_err(|e| format!("could not start the step's runtime: {e}"))?;
    async_with!(worker.context => |ctx| {
        deps_host::install(&ctx, registry, cancel).map_err(|e| e.to_string())?;
        crate::assess::install_notice(&ctx).map_err(|e| e.to_string())?;
        if let Some(handle) = assess {
            crate::assess::install(&ctx, handle.run, handle.position, handle.cancel)
                .map_err(|e| e.to_string())?;
        }
        // Before any of the step's code: what it may not use fails.
        let pure = async {
            let promise = rquickjs::Module::import(&ctx, CALLBACK_GLOBALS_MODULE)?;
            promise.into_future::<rquickjs::Value>().await.map(|_| ())
        };
        pure.await
            .catch(&ctx)
            .map_err(|e| format!("could not start the step's runtime: {e}"))
    })
    .await?;
    Ok(worker)
}
