//! Persistence for code-first workflow runs.
//!
//! A run is a [`RunRecord`] plus its step history: one [`StepRecord`] per
//! step, saved as soon as the step finishes. Resuming a run replays that
//! history. The engine only talks to the [`RunStore`] trait, so storage
//! backends are interchangeable.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("run '{0}' not found")]
    NotFound(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Running,
    AwaitingApproval,
    Succeeded,
    Failed,
    Canceled,
}

impl RunStatus {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting-approval",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

/// One workflow run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    /// Display name, e.g. the package name.
    #[serde(default)]
    pub name: Option<String>,
    /// The workflow file (`workflow.ts`, or `workflow.yaml` run through the
    /// compatibility layer).
    pub workflow_path: PathBuf,
    pub target: PathBuf,
    #[serde(default)]
    pub params: Value,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    /// Set while the run waits for a manual approval: what is being approved.
    #[serde(default)]
    pub awaiting: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// The workflow's return value once it succeeds.
    #[serde(default)]
    pub output: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Succeeded,
    Failed,
}

/// The recorded outcome of one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRecord {
    /// Order in which the workflow issued the step.
    pub position: u64,
    pub label: String,
    /// Canonical form of the step's task; a finished step whose key changed
    /// cannot be replayed.
    pub key: String,
    pub status: StepStatus,
    #[serde(default)]
    pub result: Option<Value>,
    /// Workflow state the step set (`setState` / `STATE_OUTPUTS`): a map of
    /// keys to values, where `null` removes a key.
    #[serde(default)]
    pub state: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub error: Option<String>,
    /// When a successful step finished, in milliseconds since the epoch.
    /// The workflow body's clock advances to it, also on replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// The step did not run: a test gave its result.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub given: bool,
}

/// Storage for runs and their step history.
pub trait RunStore: Send + Sync {
    fn create_run(&self, run: &RunRecord) -> StoreResult<()>;
    fn update_run(&self, run: &RunRecord) -> StoreResult<()>;
    fn get_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>>;
    /// Most recent first.
    fn list_runs(&self, limit: usize) -> StoreResult<Vec<RunRecord>>;
    /// Insert or replace the step at `step.position`.
    fn put_step(&self, run_id: &str, step: &StepRecord) -> StoreResult<()>;
    /// Ordered by position.
    fn load_steps(&self, run_id: &str) -> StoreResult<Vec<StepRecord>>;
    fn request_cancel(&self, run_id: &str) -> StoreResult<()>;
    fn cancel_requested(&self, run_id: &str) -> StoreResult<bool>;
}

/// Select a store from `CODEMOD_RUN_STORE` (`local`, the default, or
/// `memory`). `CODEMOD_RUN_STORE_DIR` moves the local store.
pub fn store_from_env() -> StoreResult<Box<dyn RunStore>> {
    match std::env::var("CODEMOD_RUN_STORE").as_deref() {
        Ok("memory") => Ok(Box::new(MemoryStore::default())),
        _ => match std::env::var_os("CODEMOD_RUN_STORE_DIR") {
            Some(dir) => Ok(Box::new(LocalStore::new(dir))),
            None => Ok(Box::new(LocalStore::default_location()?)),
        },
    }
}

fn upsert(steps: &mut Vec<StepRecord>, step: &StepRecord) {
    match steps.iter_mut().find(|s| s.position == step.position) {
        Some(existing) => *existing = step.clone(),
        None => steps.push(step.clone()),
    }
    steps.sort_by_key(|s| s.position);
}

// ---------------------------------------------------------------- memory ---

#[derive(Default)]
struct MemoryRun {
    record: Option<RunRecord>,
    steps: Vec<StepRecord>,
    cancel: bool,
}

/// Keeps everything in memory. For tests, and runs that must not persist.
#[derive(Default)]
pub struct MemoryStore {
    runs: Mutex<BTreeMap<String, MemoryRun>>,
}

impl MemoryStore {
    fn with<T>(&self, f: impl FnOnce(&mut BTreeMap<String, MemoryRun>) -> T) -> T {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut runs)
    }
}

impl RunStore for MemoryStore {
    fn create_run(&self, run: &RunRecord) -> StoreResult<()> {
        self.with(|runs| runs.entry(run.id.clone()).or_default().record = Some(run.clone()));
        Ok(())
    }

    fn update_run(&self, run: &RunRecord) -> StoreResult<()> {
        self.create_run(run)
    }

    fn get_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>> {
        Ok(self.with(|runs| runs.get(run_id).and_then(|r| r.record.clone())))
    }

    fn list_runs(&self, limit: usize) -> StoreResult<Vec<RunRecord>> {
        let mut records: Vec<RunRecord> =
            self.with(|runs| runs.values().filter_map(|r| r.record.clone()).collect());
        records.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        records.truncate(limit);
        Ok(records)
    }

    fn put_step(&self, run_id: &str, step: &StepRecord) -> StoreResult<()> {
        self.with(|runs| upsert(&mut runs.entry(run_id.to_string()).or_default().steps, step));
        Ok(())
    }

    fn load_steps(&self, run_id: &str) -> StoreResult<Vec<StepRecord>> {
        Ok(self.with(|runs| {
            runs.get(run_id)
                .map(|r| r.steps.clone())
                .unwrap_or_default()
        }))
    }

    fn request_cancel(&self, run_id: &str) -> StoreResult<()> {
        self.with(|runs| match runs.get_mut(run_id) {
            Some(run) => {
                run.cancel = true;
                Ok(())
            }
            None => Err(StoreError::NotFound(run_id.to_string())),
        })
    }

    fn cancel_requested(&self, run_id: &str) -> StoreResult<bool> {
        Ok(self.with(|runs| runs.get(run_id).is_some_and(|r| r.cancel)))
    }
}

// ----------------------------------------------------------------- local ---

/// JSON files on disk: `<root>/<run_id>/run.json`, `steps.json` and a
/// `cancel` marker.
pub struct LocalStore {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl LocalStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            write_lock: Mutex::new(()),
        }
    }

    /// `<data dir>/codemod/runs`, e.g. `~/Library/Application Support/codemod/runs`.
    pub fn default_location() -> StoreResult<Self> {
        let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
        Ok(Self::new(base.join("codemod").join("runs")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join(run_id)
    }

    fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> StoreResult<Option<T>> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|source| StoreError::Json {
                    path: path.to_path_buf(),
                    source,
                }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StoreError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Write through a temporary file so a crash never leaves half a file.
    fn write_json<T: Serialize>(path: &Path, value: &T) -> StoreResult<()> {
        let io = |source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let text = serde_json::to_string_pretty(value).map_err(|source| StoreError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text).map_err(io)?;
        fs::rename(&temporary, path).map_err(io)
    }
}

impl RunStore for LocalStore {
    fn create_run(&self, run: &RunRecord) -> StoreResult<()> {
        self.update_run(run)
    }

    fn update_run(&self, run: &RunRecord) -> StoreResult<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        Self::write_json(&self.run_dir(&run.id).join("run.json"), run)
    }

    fn get_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>> {
        Self::read_json(&self.run_dir(run_id).join("run.json"))
    }

    fn list_runs(&self, limit: usize) -> StoreResult<Vec<RunRecord>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(StoreError::Io {
                    path: self.root.clone(),
                    source,
                })
            }
        };
        let mut records = Vec::new();
        for entry in entries.flatten() {
            if let Some(record) = Self::read_json::<RunRecord>(&entry.path().join("run.json"))? {
                records.push(record);
            }
        }
        records.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        records.truncate(limit);
        Ok(records)
    }

    fn put_step(&self, run_id: &str, step: &StepRecord) -> StoreResult<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.run_dir(run_id).join("steps.json");
        let mut steps: Vec<StepRecord> = Self::read_json(&path)?.unwrap_or_default();
        upsert(&mut steps, step);
        Self::write_json(&path, &steps)
    }

    fn load_steps(&self, run_id: &str) -> StoreResult<Vec<StepRecord>> {
        Ok(Self::read_json(&self.run_dir(run_id).join("steps.json"))?.unwrap_or_default())
    }

    fn request_cancel(&self, run_id: &str) -> StoreResult<()> {
        let dir = self.run_dir(run_id);
        if !dir.join("run.json").exists() {
            return Err(StoreError::NotFound(run_id.to_string()));
        }
        let path = dir.join("cancel");
        fs::write(&path, b"").map_err(|source| StoreError::Io { path, source })
    }

    fn cancel_requested(&self, run_id: &str) -> StoreResult<bool> {
        Ok(self.run_dir(run_id).join("cancel").exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, minutes: i64) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            name: Some("demo".into()),
            workflow_path: "workflow.ts".into(),
            target: ".".into(),
            params: Value::Null,
            status: RunStatus::Running,
            started_at: DateTime::from_timestamp(minutes * 60, 0).unwrap(),
            ended_at: None,
            awaiting: None,
            error: None,
            output: None,
        }
    }

    fn step(position: u64, status: StepStatus) -> StepRecord {
        StepRecord {
            position,
            label: format!("step {position}"),
            key: "{}".into(),
            status,
            result: Some(Value::from(position)),
            state: None,
            error: None,
            finished_at: None,
            given: false,
        }
    }

    /// The contract every adapter must meet.
    fn contract(store: &dyn RunStore) {
        assert!(store.get_run("a").unwrap().is_none());
        store.create_run(&record("a", 1)).unwrap();
        store.create_run(&record("b", 2)).unwrap();

        let mut a = record("a", 1);
        a.status = RunStatus::Failed;
        store.update_run(&a).unwrap();
        assert_eq!(
            store.get_run("a").unwrap().unwrap().status,
            RunStatus::Failed
        );

        let listed: Vec<String> = store
            .list_runs(10)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(listed, ["b", "a"]);
        assert_eq!(store.list_runs(1).unwrap().len(), 1);

        store
            .put_step("a", &step(1, StepStatus::Succeeded))
            .unwrap();
        store.put_step("a", &step(0, StepStatus::Failed)).unwrap();
        store
            .put_step("a", &step(0, StepStatus::Succeeded))
            .unwrap();
        let steps = store.load_steps("a").unwrap();
        assert_eq!(steps.iter().map(|s| s.position).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(steps[0].status, StepStatus::Succeeded);
        assert!(store.load_steps("b").unwrap().is_empty());

        assert!(!store.cancel_requested("a").unwrap());
        store.request_cancel("a").unwrap();
        assert!(store.cancel_requested("a").unwrap());
        assert!(matches!(
            store.request_cancel("missing"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn memory_store_meets_the_contract() {
        contract(&MemoryStore::default());
    }

    #[test]
    fn local_store_meets_the_contract() {
        let dir = tempfile::TempDir::new().unwrap();
        contract(&LocalStore::new(dir.path()));
    }
}
