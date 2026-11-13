//! A run whose body Node runs: a thin binding over [`HostedRun`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use butterflow_state::run_store::{LocalStore, MemoryStore, RunStore, StepRecord};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_workflow::events::CollectEvents;
use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{Ending, Engine, HostedRun, OpenRequest, Services, Summary, TestPlan};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{json, Map, Value};

#[napi(object)]
pub struct OpenOptions {
    /// The workflow file; relative task paths resolve against its directory.
    pub workflow: String,
    pub target: String,
    pub params: Option<Value>,
    pub name: Option<String>,
    pub dry_run: Option<bool>,
    /// Stop after the step with this label (a test run).
    pub until: Option<String>,
    /// Results for steps that should not run, by label (a test run).
    pub given: Option<Map<String, Value>>,
    /// Give every approval the workflow asks for.
    pub approve: Option<bool>,
    /// Capabilities for host tasks' transforms, on top of codemod.yaml's.
    pub capabilities: Option<Vec<String>>,
    /// `"memory"` (the default for tests and dry runs) or `"local"`.
    pub store: Option<String>,
    /// Continue this run instead of starting one (`store: "local"`).
    pub resume: Option<String>,
}

struct Inner {
    run: HostedRun,
    events: Arc<CollectEvents>,
    summary: Arc<Summary>,
}

#[napi]
pub struct Run {
    inner: Arc<Inner>,
}

#[napi]
impl Run {
    #[napi(factory)]
    pub fn open(options: OpenOptions) -> Result<Self> {
        let workflow = PathBuf::from(&options.workflow);
        let workflow_dir = workflow
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // The CLI builds a workflow before running it, and refuses what
        // cannot run in its isolates (a callback that changes an outer
        // variable, `fs` in the body, ...). The same rules hold here, so a
        // workflow that passes its tests also runs with the CLI.
        if workflow.is_file() && codemod_workflow::is_typescript(&workflow) {
            codemod_workflow::load_workflow(&workflow).map_err(to_napi)?;
        }
        let test = if options.until.is_some() || options.given.is_some() {
            Some(TestPlan {
                until: options.until,
                given: options.given.unwrap_or_default(),
            })
        } else {
            None
        };
        let store: Arc<dyn RunStore> = match options.store.as_deref() {
            None | Some("memory") => Arc::new(MemoryStore::default()),
            Some("local") => Arc::new(LocalStore::default_location()),
            Some(other) => {
                return Err(Error::from_reason(format!(
                    "unknown store '{other}'; use \"memory\" or \"local\""
                )))
            }
        };
        let mut capabilities = manifest_capabilities(&workflow_dir);
        for name in options.capabilities.unwrap_or_default() {
            let module = name
                .parse::<LlrtSupportedModules>()
                .map_err(|_| Error::from_reason(format!("unknown capability '{name}'")))?;
            capabilities.insert(module);
        }
        let events = Arc::new(CollectEvents::default());
        let summary = Arc::new(Summary::default());
        let engine = Engine::new(
            store,
            events.clone(),
            Arc::new(NonInteractive {
                approve_all: options.approve.unwrap_or(true),
            }),
            Services {
                capabilities,
                summary: Arc::clone(&summary),
                ..Services::default()
            },
        );
        let run = match options.resume {
            Some(run_id) => engine.open_resumed(&run_id),
            None => engine.open(OpenRequest {
                workflow,
                target: PathBuf::from(&options.target),
                params: options.params.unwrap_or_else(|| json!({})),
                name: options.name,
                dry_run: options.dry_run.unwrap_or(false),
                test,
            }),
        }
        .map_err(to_napi)?;
        Ok(Self {
            inner: Arc::new(Inner {
                run,
                events,
                summary,
            }),
        })
    }

    #[napi(getter)]
    pub fn target(&self) -> String {
        self.inner.run.target().to_string_lossy().into_owned()
    }

    #[napi(getter)]
    pub fn params(&self) -> Value {
        self.inner.run.params().clone()
    }

    #[napi(getter)]
    pub fn dry_run(&self) -> bool {
        self.inner.run.dry_run()
    }

    /// Steps recorded by earlier executions of this run.
    #[napi]
    pub fn history(&self) -> Result<Value> {
        serde_json::to_value(self.inner.run.history()).map_err(to_napi)
    }

    /// `{ until, given }` when the run is a test, else `null`.
    #[napi]
    pub fn test_plan(&self) -> Result<Value> {
        serde_json::to_value(self.inner.run.test_plan()).map_err(to_napi)
    }

    // Values with integers cross as JSON text: napi turns every JS number
    // into a float.

    #[napi]
    pub fn record(&self, entry: String, duration_ms: f64) -> Result<()> {
        let step: StepRecord = serde_json::from_str(&entry).map_err(to_napi)?;
        self.inner
            .run
            .record(&step, duration_ms.max(0.0) as u64)
            .map_err(to_napi)
    }

    #[napi]
    pub fn event(&self, event: String) -> Result<()> {
        self.inner.run.event(&event).map_err(Error::from_reason)
    }

    #[napi]
    pub fn log(&self, position: Option<u32>, line: String) {
        self.inner.run.log(position.map(u64::from), line);
    }

    #[napi]
    pub fn progress(&self, position: u32, processed: u32, total: u32) {
        self.inner
            .run
            .progress(position.into(), processed as usize, total as usize);
    }

    #[napi]
    pub fn canceled(&self) -> bool {
        self.inner.run.canceled()
    }

    /// Whether a step about to start must not, because the run was canceled.
    #[napi]
    pub fn refuses_steps(&self) -> bool {
        self.inner.run.refuses_steps()
    }

    #[napi]
    pub fn cancel(&self) {
        self.inner.run.cancel();
    }

    /// The test plan's `until` step has started.
    #[napi]
    pub fn stop(&self) {
        self.inner.run.stop();
    }

    /// Evaluate a workflow.yaml `${{ }}` expression.
    #[napi]
    pub fn evaluate(&self, kind: String, expression: String, scope: String) -> Result<Value> {
        let json = self
            .inner
            .run
            .evaluate(&kind, &expression, &scope)
            .map_err(Error::from_reason)?;
        serde_json::from_str(&json).map_err(to_napi)
    }

    /// Run a host task. Resolves to its outcome; rejects with an error whose
    /// message is the payload JSON (`{ name, message }`).
    #[napi]
    pub async fn run_task(&self, request: String) -> Result<Value> {
        let inner = Arc::clone(&self.inner);
        let json = off_thread(move || inner.run.run_task(&request))
            .await?
            .map_err(Error::from_reason)?;
        serde_json::from_str(&json).map_err(to_napi)
    }

    /// The files a `forEachFile` selector picks: `{ language, files: [[path, text]] }`.
    #[napi]
    pub async fn select_files(&self, selector: String) -> Result<Value> {
        let inner = Arc::clone(&self.inner);
        let selector: Value = serde_json::from_str(&selector).map_err(to_napi)?;
        let (language, files) = off_thread(move || inner.run.select_files(selector))
            .await?
            .map_err(Error::from_reason)?;
        Ok(json!({ "language": language, "files": files }))
    }

    /// Commit what a step's callbacks did to its files. Resolves to `{ changes, failures }`.
    #[napi]
    pub async fn commit_files(&self, position: u32, label: String, files: String) -> Result<Value> {
        let inner = Arc::clone(&self.inner);
        let files: Value = serde_json::from_str(&files).map_err(to_napi)?;
        off_thread(move || inner.run.commit_files(position.into(), label, files))
            .await?
            .map_err(Error::from_reason)
    }

    /// Add metrics a step recorded to the run summary.
    #[napi]
    pub fn add_metrics(&self, metrics: String) -> Result<()> {
        let metrics: Value = serde_json::from_str(&metrics).map_err(to_napi)?;
        self.inner
            .run
            .add_metrics(metrics)
            .map_err(Error::from_reason)
    }

    /// Events since the last call, as the engine reports them.
    #[napi]
    pub fn take_events(&self) -> Result<Value> {
        serde_json::to_value(self.inner.events.take()).map_err(to_napi)
    }

    /// Record how the run ended, given how its body ended:
    /// `{ status: "succeeded", detail: output }`, `{ status: "failed", detail: error }`,
    /// `{ status: "canceled" }`, or `null` when the body cannot end any more
    /// (the steps it waits for will never start). A step that was canceled or
    /// waits for an approval decides the ending whatever the body did.
    /// Returns the report.
    #[napi]
    pub fn finish(&self, body: String) -> Result<Value> {
        let body: Option<Ending> = serde_json::from_str(&body).map_err(to_napi)?;
        let mut report = self.inner.run.finish(body).map_err(to_napi)?.to_json();
        let stats = &self.inner.summary.stats;
        let count = |counter: &std::sync::atomic::AtomicUsize| {
            counter.load(std::sync::atomic::Ordering::Relaxed)
        };
        report["metrics"] = json!(self.inner.summary.metrics());
        report["stats"] = json!({
            "filesModified": count(&stats.files_modified),
            "filesUnmodified": count(&stats.files_unmodified),
            "filesWithErrors": count(&stats.files_with_errors),
        });
        Ok(report)
    }
}

/// Run blocking engine work on its own thread: tasks start their own async
/// runtimes, which they cannot do on one of Node's.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .await
        .map_err(|_| Error::from_reason("the workflow engine stopped unexpectedly"))
}

/// The capabilities `codemod.yaml` in the workflow's directory declares.
fn manifest_capabilities(workflow_dir: &Path) -> HashSet<LlrtSupportedModules> {
    #[derive(Deserialize)]
    struct Manifest {
        #[serde(default)]
        capabilities: Vec<String>,
    }
    let Ok(text) = std::fs::read_to_string(workflow_dir.join("codemod.yaml")) else {
        return HashSet::new();
    };
    let Ok(manifest) = serde_yaml::from_str::<Manifest>(&text) else {
        return HashSet::new();
    };
    manifest
        .capabilities
        .iter()
        .filter_map(|name| name.parse().ok())
        .collect()
}

fn to_napi(error: impl std::fmt::Display) -> Error {
    Error::from_reason(error.to_string())
}
