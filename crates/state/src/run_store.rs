//! Persistence for code-first workflow runs.
//!
//! A run is a [`RunRecord`] plus its step history: one [`StepRecord`] per
//! step, saved as soon as the step finishes. Resuming a run replays that
//! history. The engine only talks to the [`RunStore`] trait, so storage
//! backends are interchangeable.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
    /// What the run was started with, so resuming it continues the same way.
    #[serde(default)]
    pub settings: RunSettings,
}

/// The choices a run was started with that its steps depend on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSettings {
    /// Runtime capabilities granted to transforms (`fs`, `fetch`, ...).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Whether `branch`, `commit` and `pullRequest` steps touch git.
    #[serde(default)]
    pub managed_git: bool,
    /// The coding agent chosen for `agent(...)` steps.
    #[serde(default)]
    pub agent: Option<String>,
    /// What runs the workflow's JavaScript: `"quickjs"` (the codemod CLI) or
    /// the host that embeds the engine (`"node"`). Each records steps its own
    /// way, so a run is resumed where it started.
    #[serde(default)]
    pub host: Option<String>,
}

/// How an execution of a run ended, as far as the record goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Succeeded(Value),
    Failed(String),
    Canceled,
    /// Waiting for an approval: what is being approved.
    AwaitingApproval(String),
}

impl RunRecord {
    /// Mark the run as running again, to continue it. A run that succeeded
    /// has nothing left to continue.
    pub fn reopen(&mut self) -> Result<(), AlreadySucceeded> {
        if self.status == RunStatus::Succeeded {
            return Err(AlreadySucceeded);
        }
        self.status = RunStatus::Running;
        self.ended_at = None;
        self.awaiting = None;
        self.error = None;
        Ok(())
    }

    /// Record how this execution ended.
    pub fn conclude(&mut self, outcome: Outcome) {
        self.awaiting = None;
        self.error = None;
        match outcome {
            Outcome::Succeeded(output) => {
                self.status = RunStatus::Succeeded;
                self.output = Some(output);
            }
            Outcome::Failed(error) => {
                self.status = RunStatus::Failed;
                self.error = Some(error);
            }
            Outcome::Canceled => self.status = RunStatus::Canceled,
            Outcome::AwaitingApproval(message) => {
                self.status = RunStatus::AwaitingApproval;
                self.awaiting = Some(message);
            }
        }
        self.ended_at = self.status.is_finished().then(Utc::now);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AlreadySucceeded;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Succeeded,
    Failed,
    /// The run was canceled while the step ran; it runs again on resume.
    Canceled,
    /// The step waits for an approval; it runs again on resume.
    AwaitingApproval,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::AwaitingApproval => "awaiting-approval",
        }
    }
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
    /// The order in which the run's steps finished. Replaying a run settles
    /// its steps in this order, so the body sees them as it did the first time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The step did not run: a test gave its result.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub given: bool,
}

/// Storage for runs and their step history.
pub trait RunStore: Send + Sync {
    /// Insert or replace a run's record.
    fn put_run(&self, run: &RunRecord) -> StoreResult<()>;
    fn get_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>>;
    /// Most recent first. Entries that cannot be read are left out.
    fn list_runs(&self, limit: usize) -> StoreResult<Vec<RunRecord>>;
    /// Insert or replace the step at `step.position`.
    fn put_step(&self, run_id: &str, step: &StepRecord) -> StoreResult<()>;
    /// Ordered by position.
    fn load_steps(&self, run_id: &str) -> StoreResult<Vec<StepRecord>>;

    /// Take the run for one executor until the lease is dropped. `None` when
    /// something else is executing it. A lease does not outlive its process,
    /// so a run whose record says running but that nobody holds was
    /// interrupted.
    fn acquire(&self, run_id: &str) -> StoreResult<Option<Box<dyn RunLease>>>;
    /// Whether something is executing the run now.
    fn is_held(&self, run_id: &str) -> StoreResult<bool>;

    /// Ask whatever executes the run to stop.
    fn request_cancel(&self, run_id: &str) -> StoreResult<()>;
    fn cancel_requested(&self, run_id: &str) -> StoreResult<bool>;
    /// Forget a cancel request, when the run is continued.
    fn clear_cancel(&self, run_id: &str) -> StoreResult<()>;
}

/// Held while a run executes; dropping it releases the run.
pub trait RunLease: Send + Sync {}

/// Select a store from `CODEMOD_RUN_STORE` (`local`, the default, or
/// `memory`). `CODEMOD_RUN_STORE_DIR` moves the local store.
pub fn store_from_env() -> Box<dyn RunStore> {
    match std::env::var("CODEMOD_RUN_STORE").as_deref() {
        Ok("memory") => Box::new(MemoryStore::default()),
        _ => match std::env::var_os("CODEMOD_RUN_STORE_DIR") {
            Some(dir) => Box::new(LocalStore::new(dir)),
            None => Box::new(LocalStore::default_location()),
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
    held: bool,
}

type MemoryRuns = Arc<Mutex<BTreeMap<String, MemoryRun>>>;

/// Keeps everything in memory. For tests, and runs that must not persist.
#[derive(Default)]
pub struct MemoryStore {
    runs: MemoryRuns,
}

fn with_runs<T>(runs: &MemoryRuns, f: impl FnOnce(&mut BTreeMap<String, MemoryRun>) -> T) -> T {
    f(&mut runs.lock().unwrap_or_else(|e| e.into_inner()))
}

impl MemoryStore {
    fn with<T>(&self, f: impl FnOnce(&mut BTreeMap<String, MemoryRun>) -> T) -> T {
        with_runs(&self.runs, f)
    }

    fn with_run<T>(&self, run_id: &str, f: impl FnOnce(&mut MemoryRun) -> T) -> StoreResult<T> {
        self.with(|runs| match runs.get_mut(run_id) {
            Some(run) => Ok(f(run)),
            None => Err(StoreError::NotFound(run_id.to_string())),
        })
    }
}

struct MemoryLease {
    runs: MemoryRuns,
    run_id: String,
}

impl RunLease for MemoryLease {}

impl Drop for MemoryLease {
    fn drop(&mut self) {
        with_runs(&self.runs, |runs| {
            if let Some(run) = runs.get_mut(&self.run_id) {
                run.held = false;
            }
        });
    }
}

impl RunStore for MemoryStore {
    fn put_run(&self, run: &RunRecord) -> StoreResult<()> {
        self.with(|runs| runs.entry(run.id.clone()).or_default().record = Some(run.clone()));
        Ok(())
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

    fn acquire(&self, run_id: &str) -> StoreResult<Option<Box<dyn RunLease>>> {
        let taken = self.with_run(run_id, |run| !std::mem::replace(&mut run.held, true))?;
        Ok(taken.then(|| {
            Box::new(MemoryLease {
                runs: Arc::clone(&self.runs),
                run_id: run_id.to_string(),
            }) as Box<dyn RunLease>
        }))
    }

    fn is_held(&self, run_id: &str) -> StoreResult<bool> {
        Ok(self.with(|runs| runs.get(run_id).is_some_and(|r| r.held)))
    }

    fn request_cancel(&self, run_id: &str) -> StoreResult<()> {
        self.with_run(run_id, |run| run.cancel = true)
    }

    fn cancel_requested(&self, run_id: &str) -> StoreResult<bool> {
        Ok(self.with(|runs| runs.get(run_id).is_some_and(|r| r.cancel)))
    }

    fn clear_cancel(&self, run_id: &str) -> StoreResult<()> {
        self.with(|runs| {
            if let Some(run) = runs.get_mut(run_id) {
                run.cancel = false;
            }
        });
        Ok(())
    }
}

// ----------------------------------------------------------------- local ---

/// JSON files on disk: `<root>/<run_id>/run.json` and `steps.json`, a
/// `cancel` marker, and a `lock` file its executor keeps locked.
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
    pub fn default_location() -> Self {
        let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
        Self::new(base.join("codemod").join("runs"))
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
        // Named after the process: `workflow cancel` may write the record of a
        // run another process has just picked up.
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        fs::write(&temporary, text).map_err(io)?;
        fs::rename(&temporary, path).map_err(io)
    }

    /// The run's lock file, opened; `None` when the run does not exist.
    fn lock_file(&self, run_id: &str) -> StoreResult<Option<(PathBuf, fs::File)>> {
        let dir = self.run_dir(run_id);
        if !dir.join("run.json").exists() {
            return Ok(None);
        }
        let path = dir.join("lock");
        match fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
        {
            Ok(file) => Ok(Some((path, file))),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    /// Lock the run's lock file: `Some` when nothing else holds it.
    // std's file locks are newer than the workspace's declared `rust-version`;
    // the workspace builds with the toolchain `rust-toolchain.toml` pins.
    #[allow(clippy::incompatible_msrv)]
    fn try_lock(&self, run_id: &str) -> StoreResult<Option<fs::File>> {
        let Some((path, file)) = self.lock_file(run_id)? else {
            return Err(StoreError::NotFound(run_id.to_string()));
        };
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(source)) => Err(StoreError::Io { path, source }),
        }
    }
}

/// The run's lock file, locked. The operating system releases the lock when
/// the file closes, also when the process dies.
struct LocalLease {
    _file: fs::File,
}

impl RunLease for LocalLease {}

impl RunStore for LocalStore {
    fn put_run(&self, run: &RunRecord) -> StoreResult<()> {
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
        // Stray files and records this version cannot read are not runs to
        // list; one of them must not hide the rest.
        let mut records: Vec<RunRecord> = entries
            .flatten()
            .filter_map(|entry| {
                Self::read_json(&entry.path().join("run.json"))
                    .ok()
                    .flatten()
            })
            .collect();
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

    fn acquire(&self, run_id: &str) -> StoreResult<Option<Box<dyn RunLease>>> {
        Ok(self
            .try_lock(run_id)?
            .map(|file| Box::new(LocalLease { _file: file }) as Box<dyn RunLease>))
    }

    fn is_held(&self, run_id: &str) -> StoreResult<bool> {
        match self.try_lock(run_id) {
            // Ours for a moment: dropping the file releases it again.
            Ok(Some(_)) | Err(StoreError::NotFound(_)) => Ok(false),
            Ok(None) => Ok(true),
            Err(error) => Err(error),
        }
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

    fn clear_cancel(&self, run_id: &str) -> StoreResult<()> {
        let path = self.run_dir(run_id).join("cancel");
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StoreError::Io { path, source }),
        }
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
            settings: RunSettings::default(),
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
            seq: None,
            given: false,
        }
    }

    /// The contract every adapter must meet.
    fn contract(store: &dyn RunStore) {
        assert!(store.get_run("a").unwrap().is_none());
        store.put_run(&record("a", 1)).unwrap();
        store.put_run(&record("b", 2)).unwrap();

        let mut a = record("a", 1);
        a.status = RunStatus::Failed;
        store.put_run(&a).unwrap();
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
        store.clear_cancel("a").unwrap();
        assert!(!store.cancel_requested("a").unwrap());

        assert!(!store.is_held("a").unwrap());
        let lease = store.acquire("a").unwrap().expect("nothing holds the run");
        assert!(store.is_held("a").unwrap());
        assert!(store.acquire("a").unwrap().is_none());
        assert!(!store.is_held("b").unwrap());
        drop(lease);
        assert!(!store.is_held("a").unwrap());
        assert!(store.acquire("a").unwrap().is_some());
        assert!(matches!(
            store.acquire("missing"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn a_record_follows_its_run() {
        let mut run = record("a", 1);
        run.conclude(Outcome::AwaitingApproval("ship?".into()));
        assert_eq!(run.status, RunStatus::AwaitingApproval);
        assert_eq!(run.awaiting.as_deref(), Some("ship?"));
        assert!(run.ended_at.is_none());

        run.reopen().unwrap();
        assert_eq!(run.status, RunStatus::Running);
        assert!(run.awaiting.is_none());

        run.conclude(Outcome::Failed("boom".into()));
        assert_eq!(run.error.as_deref(), Some("boom"));
        assert!(run.ended_at.is_some());

        run.reopen().unwrap();
        assert!(run.error.is_none() && run.ended_at.is_none());
        run.conclude(Outcome::Succeeded(Value::from(1)));
        assert_eq!(run.reopen(), Err(AlreadySucceeded));
    }

    #[test]
    fn listing_skips_what_is_not_a_run() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = LocalStore::new(dir.path());
        store.put_run(&record("a", 1)).unwrap();
        fs::write(dir.path().join(".DS_Store"), b"x").unwrap();
        fs::create_dir(dir.path().join("broken")).unwrap();
        fs::write(dir.path().join("broken").join("run.json"), b"{ not json").unwrap();
        let listed: Vec<String> = store
            .list_runs(10)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(listed, ["a"]);
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
