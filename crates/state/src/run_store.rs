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
    /// Stopped on request (Ctrl-C, `stop {mode: "pause"}`): resume continues it.
    Paused,
    Succeeded,
    Failed,
    /// Canceled (`workflow cancel`, `stop {mode: "cancel"}`): over; resuming
    /// it takes `--force`.
    Canceled,
    /// An approval was rejected: over; earlier steps' changes stay, and
    /// resuming it takes `--force`.
    Rejected,
}

/// Whether a run may be continued. Every rule about which statuses can be
/// resumed lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resumable {
    Yes,
    /// Only when asked to on purpose (`--force`): the run was ended by a person.
    WithForce,
    /// It succeeded: there is nothing left to run.
    No,
}

impl RunStatus {
    /// Whether the run has ended, for good or until it is resumed: it has
    /// an end time.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Canceled | Self::Rejected
        )
    }

    pub fn resumable(self) -> Resumable {
        match self {
            Self::Running | Self::AwaitingApproval | Self::Paused | Self::Failed => Resumable::Yes,
            Self::Canceled | Self::Rejected => Resumable::WithForce,
            Self::Succeeded => Resumable::No,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting-approval",
            Self::Paused => "paused",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Rejected => "rejected",
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
    /// Also kept when the run was paused while the approval waited.
    #[serde(default)]
    pub awaiting: Option<String>,
    /// With `awaiting`: the id of the question waiting (`approval:3`), which
    /// `workflow resume --approve <id>` gives. Kept on a run paused at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// The workflow's return value once it succeeds, or the output a
    /// workflow gave `fail()`.
    #[serde(default)]
    pub output: Option<Value>,
    /// The exit code a workflow gave `fail()`; `None` for every other ending.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// What the run was started with, so resuming it continues the same way.
    #[serde(default)]
    pub settings: RunSettings,
    /// The codemod version that last executed the run. Its step keys are
    /// that version's: function steps are keyed by a hash of the code it
    /// generated. `None` for runs from before versions were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codemod_version: Option<String>,
}

/// A run in the store this version cannot read: a newer (or much older)
/// codemod wrote it. Listed so it does not silently disappear.
#[derive(Debug, Clone, PartialEq)]
pub struct UnreadableRun {
    pub id: String,
    /// The version that wrote it, when the record says.
    pub codemod_version: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    /// Why it cannot be read.
    pub error: String,
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
    /// The run still has to check for uncommitted changes before it first
    /// changes its target: it was started with the check, and has not passed
    /// it yet. A resume checks then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub check_uncommitted: bool,
    /// The workflow is the person's own (a local file or directory they
    /// named), not a package from a registry: an unattended run does what
    /// its steps run and send without `--approve`. Recorded, so a resumed
    /// run of a registry package does not become trusted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trusted: bool,
}

/// How an execution of a run ended, as far as the record goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Succeeded(Value),
    Failed(String),
    /// The workflow called `fail()`: failed, with an output and an exit code.
    FailedWithOutput {
        error: String,
        output: Value,
        exit_code: i32,
    },
    Canceled,
    /// Stopped on request, to be resumed.
    Paused,
    /// An approval was rejected: what it asked.
    Rejected(String),
    /// Waiting for an approval: what is being approved.
    AwaitingApproval(String),
}

/// Why a run cannot be continued.
#[derive(Debug, PartialEq, Eq)]
pub enum NotResumable {
    AlreadySucceeded,
    /// Canceled or rejected, and not forced.
    Ended(RunStatus),
}

impl RunRecord {
    /// Mark the run as running again, to continue it. A run that succeeded
    /// has nothing left to continue; one that was canceled or rejected is
    /// continued only when `force`d (see [`RunStatus::resumable`]).
    pub fn reopen(&mut self, force: bool) -> Result<(), NotResumable> {
        match self.status.resumable() {
            Resumable::No => return Err(NotResumable::AlreadySucceeded),
            Resumable::WithForce if !force => return Err(NotResumable::Ended(self.status)),
            _ => {}
        }
        self.status = RunStatus::Running;
        self.ended_at = None;
        self.awaiting = None;
        self.question = None;
        self.error = None;
        Ok(())
    }

    /// Record how this execution ended.
    pub fn conclude(&mut self, outcome: Outcome) {
        self.awaiting = None;
        self.question = None;
        self.error = None;
        self.exit_code = None;
        match outcome {
            Outcome::Succeeded(output) => {
                self.status = RunStatus::Succeeded;
                self.output = Some(output);
            }
            Outcome::Failed(error) => {
                self.status = RunStatus::Failed;
                self.error = Some(error);
                self.output = None;
            }
            Outcome::FailedWithOutput {
                error,
                output,
                exit_code,
            } => {
                self.status = RunStatus::Failed;
                self.error = Some(error);
                self.output = Some(output);
                self.exit_code = Some(exit_code);
            }
            Outcome::Canceled => self.status = RunStatus::Canceled,
            Outcome::Paused => self.status = RunStatus::Paused,
            Outcome::Rejected(message) => {
                self.status = RunStatus::Rejected;
                self.error = Some(format!("rejected: {message}"));
            }
            Outcome::AwaitingApproval(message) => {
                self.status = RunStatus::AwaitingApproval;
                self.awaiting = Some(message);
            }
        }
        self.ended_at = self.status.is_finished().then(Utc::now);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    /// A side-effecting step began, but no final outcome is known.
    Started,
    /// The operator chose to continue without repeating an interrupted step.
    Skipped,
    Succeeded,
    /// The step finished, but some of its files failed; `error` says how
    /// many. It counts as finished: a resume reuses its result.
    Partial,
    Failed,
    /// The run was canceled while the step ran; it runs again on resume.
    Canceled,
    /// The step waits for an approval; it runs again on resume.
    AwaitingApproval,
    /// The approval this step asked for was rejected: the run ended there.
    Rejected,
    /// The step is waiting, for a time or an event (`sleep`, `waitForEvent`).
    /// Recorded when the wait begins, so a resume counts the time already
    /// waited.
    Waiting,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Skipped => "skipped",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::AwaitingApproval => "awaiting-approval",
            Self::Rejected => "rejected",
            Self::Waiting => "waiting",
        }
    }
}

/// The recorded outcome of one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRecord {
    /// The step's place in the run's list of steps: the order steps were
    /// first issued in. It stays the step's when a resume runs it again.
    pub position: u64,
    pub label: String,
    /// Canonical form of the step's label and task. A resume takes a step
    /// for a recorded one when their keys are equal.
    pub key: String,
    pub status: StepStatus,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    /// When a successful or partial step finished, in milliseconds since the epoch.
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
    /// When a `sleep` or `waitForEvent` step began to wait, in milliseconds
    /// since the epoch. Kept while the wait is interrupted and continued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// How many times the step ran, when its `retries` made that more than once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    /// For a step that works on files (`forEachFile`, `astGrep`, a nested
    /// `codemod`): how many of its files failed, of how many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<FileCount>,
    /// How long the step's last run took, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// How many of a step's files failed, of how many it ran on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCount {
    pub failed: usize,
    pub total: usize,
}

impl StepRecord {
    /// How many of the step's files failed, including hard failures.
    /// Records from before the count was kept have it in `error` only
    /// ("2 of 10 files failed").
    pub fn failed_files(&self) -> usize {
        match self.files {
            Some(files) => files.failed,
            None if self.status == StepStatus::Partial => self
                .error
                .as_deref()
                .and_then(|error| error.split(' ').next())
                .and_then(|count| count.parse().ok())
                .unwrap_or(1),
            None => 0,
        }
    }
}

/// Storage for runs and their step history.
pub trait RunStore: Send + Sync {
    /// Insert or replace a run's record.
    fn put_run(&self, run: &RunRecord) -> StoreResult<()>;
    fn get_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>>;
    /// Most recent first. Entries that cannot be read are left out: see
    /// [`RunStore::list_unreadable`].
    fn list_runs(&self, limit: usize) -> StoreResult<Vec<RunRecord>>;
    /// Runs whose record this version cannot read, most recent first.
    fn list_unreadable(&self) -> StoreResult<Vec<UnreadableRun>> {
        Ok(Vec::new())
    }
    /// Insert or replace the step at `step.position`. Only the executor
    /// holding the run lease writes steps.
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

    /// Serialize the brief transitions that publish, reopen, finish or
    /// cancel a run. Never hold this while executing steps or prompting.
    fn lock_run_state(&self, run_id: &str) -> StoreResult<Box<dyn RunLease>>;

    /// Take a target (a repository) for run `run_id` until the lease is
    /// dropped, so two runs never change one repository at once. `Err`
    /// names the run holding it. A lease does not outlive its process: a
    /// lock a killed process left is taken over.
    fn lock_target(
        &self,
        target: &Path,
        run_id: &str,
    ) -> StoreResult<Result<Box<dyn RunLease>, TargetHolder>>;

    /// Ask whatever executes the run to stop.
    fn request_cancel(&self, run_id: &str) -> StoreResult<()>;
    fn cancel_requested(&self, run_id: &str) -> StoreResult<bool>;
    /// Forget a cancel request, when the run is continued.
    fn clear_cancel(&self, run_id: &str) -> StoreResult<()>;

    /// Leave an event for the run; a `waitForEvent(name)` step takes it.
    /// Events of one name are taken in the order they were sent.
    fn send_event(&self, run_id: &str, name: &str, data: &Value) -> StoreResult<()>;
    /// Take the oldest event of that name left for the run, if there is one.
    fn take_event(&self, run_id: &str, name: &str) -> StoreResult<Option<Value>>;

    /// Add a line (one JSON event, without its line break) to the run's
    /// event journal: what `workflow attach` replays.
    fn append_journal(&self, run_id: &str, line: &str) -> StoreResult<()>;
    /// The journal's complete lines from `cursor` on (`0`: the start), and
    /// the cursor to read on from. Nothing when the run has no journal.
    fn read_journal(&self, run_id: &str, cursor: u64) -> StoreResult<(Vec<String>, u64)>;
}

/// Whether `name` can name an event: letters, digits, `.`, `_` and `-`, up
/// to 100 characters. Names end up in file names.
pub fn valid_event_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Held while a run executes; dropping it releases the run.
pub trait RunLease: Send + Sync {}

/// The run that holds a target ([`RunStore::lock_target`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetHolder {
    pub run_id: String,
    /// Its process, when known.
    #[serde(default)]
    pub pid: Option<u32>,
    /// The target it locked (the repository).
    #[serde(default)]
    pub target: PathBuf,
}

/// Whether process `pid` is still running. Where that cannot be told, it
/// is taken to be.
fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // Signal 0 checks the process exists; EPERM means it does, as
        // someone else's.
        // SAFETY: kill with signal 0 sends nothing.
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// A stable name for a target's lock file (FNV-1a of its path).
fn target_lock_name(target: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in target.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}.lock")
}

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
    events: Vec<(String, Value)>,
    journal: Vec<String>,
}

type MemoryRuns = Arc<Mutex<BTreeMap<String, MemoryRun>>>;

type MemoryTargets = Arc<Mutex<BTreeMap<PathBuf, String>>>;

/// Keeps everything in memory. For tests, and runs that must not persist.
#[derive(Default)]
pub struct MemoryStore {
    runs: MemoryRuns,
    targets: MemoryTargets,
    state: MemoryTargets,
    state_changed: Arc<std::sync::Condvar>,
}

struct MemoryTargetLease {
    targets: MemoryTargets,
    target: PathBuf,
    notify: Option<Arc<std::sync::Condvar>>,
}

impl RunLease for MemoryTargetLease {}

impl Drop for MemoryTargetLease {
    fn drop(&mut self) {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.target);
        if let Some(notify) = &self.notify {
            notify.notify_all();
        }
    }
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

    fn lock_run_state(&self, run_id: &str) -> StoreResult<Box<dyn RunLease>> {
        let path = Path::new(run_id);
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self
            .state_changed
            .wait_while(state, |held| held.contains_key(path))
            .unwrap_or_else(|e| e.into_inner());
        state.insert(path.to_path_buf(), run_id.to_string());
        Ok(Box::new(MemoryTargetLease {
            targets: Arc::clone(&self.state),
            target: path.to_path_buf(),
            notify: Some(Arc::clone(&self.state_changed)),
        }))
    }

    fn lock_target(
        &self,
        target: &Path,
        run_id: &str,
    ) -> StoreResult<Result<Box<dyn RunLease>, TargetHolder>> {
        let mut targets = self.targets.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(holder) = targets.get(target) {
            return Ok(Err(TargetHolder {
                run_id: holder.clone(),
                pid: Some(std::process::id()),
                target: target.to_path_buf(),
            }));
        }
        targets.insert(target.to_path_buf(), run_id.to_string());
        Ok(Ok(Box::new(MemoryTargetLease {
            targets: Arc::clone(&self.targets),
            target: target.to_path_buf(),
            notify: None,
        })))
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

    fn send_event(&self, run_id: &str, name: &str, data: &Value) -> StoreResult<()> {
        self.with_run(run_id, |run| {
            run.events.push((name.to_string(), data.clone()))
        })
    }

    fn take_event(&self, run_id: &str, name: &str) -> StoreResult<Option<Value>> {
        Ok(self.with(|runs| {
            let events = &mut runs.get_mut(run_id)?.events;
            let index = events.iter().position(|(sent, _)| sent == name)?;
            Some(events.remove(index).1)
        }))
    }

    fn append_journal(&self, run_id: &str, line: &str) -> StoreResult<()> {
        self.with_run(run_id, |run| run.journal.push(line.to_string()))
    }

    fn read_journal(&self, run_id: &str, cursor: u64) -> StoreResult<(Vec<String>, u64)> {
        Ok(self.with(|runs| match runs.get(run_id) {
            Some(run) => {
                let lines: Vec<String> =
                    run.journal.iter().skip(cursor as usize).cloned().collect();
                (lines, run.journal.len() as u64)
            }
            None => (Vec::new(), cursor),
        }))
    }
}

// ----------------------------------------------------------------- local ---

/// JSON files on disk: `<root>/<run_id>/run.json` and `steps.jsonl`, a
/// `cancel` marker, a `lock` file its executor keeps locked, one file per
/// undelivered event under `events/`, and the event journal,
/// `journal.jsonl`.
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

    fn run_dir(&self, run_id: &str) -> StoreResult<PathBuf> {
        Self::validate_component(run_id)?;
        let path = self.root.join(run_id);
        if path.is_symlink() {
            return Err(StoreError::NotFound(run_id.into()));
        }
        Ok(path)
    }

    fn validate_component(value: &str) -> StoreResult<()> {
        if value.is_empty()
            || value.starts_with('.')
            || value.ends_with('.')
            || value.contains(['/', '\\', ':'])
            || value.chars().any(char::is_control)
        {
            return Err(StoreError::NotFound(value.into()));
        }
        Ok(())
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
            Self::create_dir_durable(parent).map_err(io)?;
        }
        let text = serde_json::to_string_pretty(value).map_err(|source| StoreError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        Self::write_atomic(path, text.as_bytes())
    }

    fn write_atomic(path: &Path, bytes: &[u8]) -> StoreResult<()> {
        use std::io::Write as _;
        let io = |source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        };
        let parent = path.parent().expect("run-store files have a directory");
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
        temporary.write_all(bytes).map_err(io)?;
        temporary.as_file().sync_all().map_err(io)?;
        temporary.persist(path).map_err(|error| io(error.error))?;
        Self::sync_parent(path).map_err(io)
    }

    fn private_file_options() -> fs::OpenOptions {
        let mut options = fs::OpenOptions::new();
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        // Windows does not need a Unix mode, but shares the rest of the options.
        options.create(true);
        options
    }

    fn sync_parent(path: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            fs::File::open(parent)?.sync_all()?;
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }

    fn create_dir_durable(path: &Path) -> std::io::Result<()> {
        if path.as_os_str().is_empty() || path.is_dir() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            Self::create_dir_durable(parent)?;
        }
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(false);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut dirs, 0o700);
        match dirs.create(path) {
            Ok(()) => Self::sync_parent(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// A writer killed mid-append may leave an uncommitted final line, with
    /// no newline. Drop that tail before starting another append. The common
    /// case reads only the last block, not the history.
    fn trim_log_tail(file: &mut fs::File) -> std::io::Result<()> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let len = file.metadata()?.len();
        if len != 0 {
            let mut last = [0];
            file.seek(SeekFrom::End(-1))?;
            file.read_exact(&mut last)?;
            if last[0] == b'\n' {
                return Ok(());
            }
        }
        let mut end = len;
        let mut buf = [0; 4096];
        while end > 0 {
            let start = end.saturating_sub(buf.len() as u64);
            let block = &mut buf[..(end - start) as usize];
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(block)?;
            if let Some(last) = block.iter().rposition(|&byte| byte == b'\n') {
                let committed = start + last as u64 + 1;
                if committed != len {
                    file.set_len(committed)?;
                }
                return Ok(());
            }
            end = start;
        }
        if len != 0 {
            file.set_len(0)?;
        }
        Ok(())
    }

    /// The run's executor is the only writer. Use write access, not
    /// append-only access, so truncating a crash tail also works on Windows.
    fn open_append_log(path: &Path) -> std::io::Result<fs::File> {
        use std::io::{Seek as _, SeekFrom};
        let mut file = Self::private_file_options()
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        Self::trim_log_tail(&mut file)?;
        file.seek(SeekFrom::End(0))?;
        Ok(file)
    }

    /// The run's lock file, opened; `None` when the run does not exist.
    fn lock_file(&self, run_id: &str) -> StoreResult<Option<(PathBuf, fs::File)>> {
        let dir = self.run_dir(run_id)?;
        if !dir.join("run.json").exists() {
            return Ok(None);
        }
        let path = dir.join("lock");
        match Self::private_file_options()
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

/// A target's lock file where the file system has no locks: it names the
/// holder's process, and goes when the lease does. One a killed process
/// left is taken over (its process is gone).
struct PidLease {
    path: PathBuf,
}

impl RunLease for PidLease {}

impl Drop for PidLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl LocalStore {
    /// `<root>/.targets/<hash>.lock`: one per target, outside the runs.
    fn target_lock_path(&self, target: &Path) -> PathBuf {
        self.root.join(".targets").join(target_lock_name(target))
    }

    /// Take a target where file locks are not supported: by the process id
    /// written in its lock file.
    fn lock_target_by_pid(
        path: PathBuf,
        holder: TargetHolder,
    ) -> StoreResult<Result<Box<dyn RunLease>, TargetHolder>> {
        let current: Option<TargetHolder> = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        if let Some(current) = current {
            if current.pid.is_some_and(process_alive) {
                return Ok(Err(current));
            }
        }
        Self::write_json(&path, &holder)?;
        Ok(Ok(Box::new(PidLease { path })))
    }
}

impl RunStore for LocalStore {
    fn put_run(&self, run: &RunRecord) -> StoreResult<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        Self::write_json(&self.run_dir(&run.id)?.join("run.json"), run)
    }

    fn get_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>> {
        Self::read_json(&self.run_dir(run_id)?.join("run.json"))
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
        // list (`list_unreadable` has the latter); one of them must not hide
        // the rest.
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

    fn list_unreadable(&self) -> StoreResult<Vec<UnreadableRun>> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(Vec::new());
        };
        let mut unreadable: Vec<UnreadableRun> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path().join("run.json");
                if !path.is_file() {
                    return None;
                }
                let error = match Self::read_json::<RunRecord>(&path) {
                    Ok(_) => return None,
                    Err(error) => error.to_string(),
                };
                // What can still be made out of it.
                let raw: Option<Value> = fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok());
                let field = |name: &str| {
                    raw.as_ref()
                        .and_then(|raw| raw.get(name))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                };
                Some(UnreadableRun {
                    id: field("id").unwrap_or_else(|| entry.file_name().to_string_lossy().into()),
                    codemod_version: field("codemodVersion"),
                    started_at: field("startedAt").and_then(|at| at.parse().ok()),
                    error,
                })
            })
            .collect();
        unreadable.sort_by_key(|run| std::cmp::Reverse(run.started_at));
        Ok(unreadable)
    }

    fn put_step(&self, run_id: &str, step: &StepRecord) -> StoreResult<()> {
        use std::io::Write as _;
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.run_dir(run_id)?;
        let path = dir.join("steps.jsonl");
        let io = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        let mut line = serde_json::to_vec(step).map_err(|source| StoreError::Json {
            path: path.clone(),
            source,
        })?;
        line.push(b'\n');
        // Only the executor holding the run writes steps. Status readers
        // may run concurrently; a newline is the commit boundary.
        Self::create_dir_durable(&dir).map_err(io)?;
        let mut file = Self::open_append_log(&path).map_err(io)?;
        let first = file.metadata().map_err(io)?.len() == 0;
        file.write_all(&line).map_err(io)?;
        file.sync_all().map_err(io)?;
        if first {
            Self::sync_parent(&path).map_err(io)?;
        }
        Ok(())
    }

    fn load_steps(&self, run_id: &str) -> StoreResult<Vec<StepRecord>> {
        use std::io::BufRead as _;
        let dir = self.run_dir(run_id)?;
        let legacy: Option<Vec<StepRecord>> = Self::read_json(&dir.join("steps.json"))?;
        let mut steps: std::collections::BTreeMap<_, _> = legacy
            .unwrap_or_default()
            .into_iter()
            .map(|step| (step.position, step))
            .collect();
        let path = dir.join("steps.jsonl");
        let io = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(steps.into_values().collect());
            }
            Err(source) => return Err(io(source)),
        };
        let mut reader = std::io::BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            reader.read_until(b'\n', &mut line).map_err(io)?;
            if line.last() != Some(&b'\n') {
                break;
            }
            let step: StepRecord =
                serde_json::from_slice(&line).map_err(|source| StoreError::Json {
                    path: path.clone(),
                    source,
                })?;
            steps.insert(step.position, step);
        }
        Ok(steps.into_values().collect())
    }

    fn acquire(&self, run_id: &str) -> StoreResult<Option<Box<dyn RunLease>>> {
        Ok(self
            .try_lock(run_id)?
            .map(|file| Box::new(LocalLease { _file: file }) as Box<dyn RunLease>))
    }

    #[allow(clippy::incompatible_msrv)]
    fn lock_target(
        &self,
        target: &Path,
        run_id: &str,
    ) -> StoreResult<Result<Box<dyn RunLease>, TargetHolder>> {
        use std::io::{Read as _, Seek as _, Write as _};
        let path = self.target_lock_path(target);
        let io = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        if let Some(parent) = path.parent() {
            Self::create_dir_durable(parent).map_err(io)?;
        }
        let holder = TargetHolder {
            run_id: run_id.to_string(),
            pid: Some(std::process::id()),
            target: target.to_path_buf(),
        };
        let mut file = Self::private_file_options()
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(io)?;
        match file.try_lock() {
            Ok(()) => {
                // What a run before left here (also one that was killed) is
                // not a holder any more.
                file.set_len(0).map_err(io)?;
                file.seek(std::io::SeekFrom::Start(0)).map_err(io)?;
                file.write_all(
                    serde_json::to_string(&holder)
                        .unwrap_or_default()
                        .as_bytes(),
                )
                .map_err(io)?;
                file.flush().map_err(io)?;
                Ok(Ok(Box::new(LocalLease { _file: file })))
            }
            Err(fs::TryLockError::WouldBlock) => {
                let mut text = String::new();
                let _ = file.read_to_string(&mut text);
                let held = serde_json::from_str(&text).unwrap_or(TargetHolder {
                    run_id: "unknown".to_string(),
                    pid: None,
                    target: target.to_path_buf(),
                });
                Ok(Err(held))
            }
            // No file locks here (some network file systems): by process id.
            Err(fs::TryLockError::Error(_)) => {
                drop(file);
                Self::lock_target_by_pid(path, holder)
            }
        }
    }

    fn is_held(&self, run_id: &str) -> StoreResult<bool> {
        match self.try_lock(run_id) {
            // Ours for a moment: dropping the file releases it again.
            Ok(Some(_)) | Err(StoreError::NotFound(_)) => Ok(false),
            Ok(None) => Ok(true),
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::incompatible_msrv)]
    fn lock_run_state(&self, run_id: &str) -> StoreResult<Box<dyn RunLease>> {
        let dir = self.run_dir(run_id)?;
        let path = dir.join("state.lock");
        let io = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        Self::create_dir_durable(&dir).map_err(io)?;
        let file = Self::private_file_options()
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io)?;
        file.lock().map_err(io)?;
        Ok(Box::new(LocalLease { _file: file }))
    }

    fn request_cancel(&self, run_id: &str) -> StoreResult<()> {
        let dir = self.run_dir(run_id)?;
        if !dir.join("run.json").exists() {
            return Err(StoreError::NotFound(run_id.to_string()));
        }
        let path = dir.join("cancel");
        Self::write_atomic(&path, b"")
    }

    fn cancel_requested(&self, run_id: &str) -> StoreResult<bool> {
        Ok(self.run_dir(run_id)?.join("cancel").exists())
    }

    fn clear_cancel(&self, run_id: &str) -> StoreResult<()> {
        let path = self.run_dir(run_id)?.join("cancel");
        match fs::remove_file(&path) {
            Ok(()) => Self::sync_parent(&path).map_err(|source| StoreError::Io { path, source }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    fn send_event(&self, run_id: &str, name: &str, data: &Value) -> StoreResult<()> {
        let dir = self.run_dir(run_id)?;
        if !dir.join("run.json").exists() {
            return Err(StoreError::NotFound(run_id.to_string()));
        }
        // Named so that sorting the directory gives the order sent, also
        // between processes sending at the same moment.
        static SENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sent = Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let count = SENT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !valid_event_name(name) {
            return Err(StoreError::NotFound(name.into()));
        }
        let file = format!("{name}@{sent:020}-{}-{count:06}.json", std::process::id());
        Self::write_json(&dir.join("events").join(file), data)
    }

    fn append_journal(&self, run_id: &str, line: &str) -> StoreResult<()> {
        use std::io::Write as _;
        let dir = self.run_dir(run_id)?;
        if !dir.join("run.json").exists() {
            return Err(StoreError::NotFound(run_id.to_string()));
        }
        let path = dir.join("journal.jsonl");
        let io = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        let mut file = Self::private_file_options()
            .append(true)
            .open(&path)
            .map_err(io)?;
        file.write_all(format!("{line}\n").as_bytes()).map_err(io)
    }

    fn read_journal(&self, run_id: &str, cursor: u64) -> StoreResult<(Vec<String>, u64)> {
        use std::io::{Read as _, Seek as _};
        let path = self.run_dir(run_id)?.join("journal.jsonl");
        let io = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), cursor))
            }
            Err(source) => return Err(io(source)),
        };
        file.seek(std::io::SeekFrom::Start(cursor)).map_err(io)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io)?;
        // A line still being written is read next time.
        let complete = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |at| at + 1);
        let lines = String::from_utf8_lossy(&bytes[..complete])
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        Ok((lines, cursor + complete as u64))
    }

    fn take_event(&self, run_id: &str, name: &str) -> StoreResult<Option<Value>> {
        let dir = self.run_dir(run_id)?.join("events");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(StoreError::Io { path: dir, source }),
        };
        let prefix = format!("{name}@");
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|e| e == "json")
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
            })
            .collect();
        files.sort();
        for path in files {
            // Whoever removes the file has the event; another taker moves on.
            let Some(data) = Self::read_json::<Value>(&path)? else {
                continue;
            };
            match fs::remove_file(&path) {
                Ok(()) => return Ok(Some(data)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(StoreError::Io { path, source }),
            }
        }
        Ok(None)
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
            question: None,
            error: None,
            output: None,
            exit_code: None,
            settings: RunSettings::default(),
            codemod_version: None,
        }
    }

    fn step(position: u64, status: StepStatus) -> StepRecord {
        StepRecord {
            position,
            label: format!("step {position}"),
            key: "{}".into(),
            status,
            result: Some(Value::from(position)),
            error: None,
            finished_at: None,
            seq: None,
            given: false,
            since: None,
            attempts: None,
            files: None,
            duration_ms: None,
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

        assert!(store.take_event("a", "ready").unwrap().is_none());
        store.send_event("a", "ready", &Value::from(1)).unwrap();
        store.send_event("a", "other", &Value::Null).unwrap();
        store.send_event("a", "ready", &Value::from(2)).unwrap();
        assert!(store.take_event("b", "ready").unwrap().is_none());
        assert_eq!(
            store.take_event("a", "ready").unwrap(),
            Some(Value::from(1))
        );
        assert_eq!(
            store.take_event("a", "ready").unwrap(),
            Some(Value::from(2))
        );
        assert!(store.take_event("a", "ready").unwrap().is_none());
        assert_eq!(store.take_event("a", "other").unwrap(), Some(Value::Null));
        assert!(matches!(
            store.send_event("missing", "ready", &Value::Null),
            Err(StoreError::NotFound(_))
        ));

        let (lines, end) = store.read_journal("a", 0).unwrap();
        assert!(lines.is_empty());
        store.append_journal("a", r#"{"seq":0}"#).unwrap();
        store.append_journal("a", r#"{"seq":1}"#).unwrap();
        let (lines, cursor) = store.read_journal("a", end).unwrap();
        assert_eq!(lines, [r#"{"seq":0}"#, r#"{"seq":1}"#]);
        store.append_journal("a", r#"{"seq":2}"#).unwrap();
        let (lines, _) = store.read_journal("a", cursor).unwrap();
        assert_eq!(lines, [r#"{"seq":2}"#]);
        assert!(store.read_journal("b", 0).unwrap().0.is_empty());
        assert!(matches!(
            store.append_journal("missing", "{}"),
            Err(StoreError::NotFound(_))
        ));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let run = store.run_dir("a").unwrap();
            assert_eq!(run.metadata().unwrap().permissions().mode() & 0o077, 0);
            for name in ["run.json", "steps.jsonl", "journal.jsonl"] {
                let metadata = run.join(name).metadata().unwrap();
                assert_eq!(metadata.permissions().mode() & 0o077, 0, "{name}");
            }
        }

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

        run.reopen(false).unwrap();
        assert_eq!(run.status, RunStatus::Running);
        assert!(run.awaiting.is_none());

        run.conclude(Outcome::Failed("boom".into()));
        assert_eq!(run.error.as_deref(), Some("boom"));
        assert!(run.ended_at.is_some());

        run.reopen(false).unwrap();
        assert!(run.error.is_none() && run.ended_at.is_none());
        run.conclude(Outcome::Paused);
        assert_eq!(run.status, RunStatus::Paused);
        assert!(run.ended_at.is_none());
        run.reopen(false).unwrap();
        run.conclude(Outcome::Succeeded(Value::from(1)));
        assert_eq!(run.reopen(true), Err(NotResumable::AlreadySucceeded));
    }

    #[test]
    fn local_store_recovers_an_interrupted_write_and_keeps_legacy_steps() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::new(dir.path());
        store.put_run(&record("a", 1)).unwrap();
        let run_dir = store.run_dir("a");
        LocalStore::write_json(
            &run_dir.join("steps.json"),
            &[step(0, StepStatus::Failed), step(2, StepStatus::Partial)],
        )
        .unwrap();
        store
            .put_step("a", &step(0, StepStatus::Succeeded))
            .unwrap();

        // Bytes left by a writer that stops halfway through a record, or
        // before publishing a new run.json, must not replace saved data.
        let mut writer = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "run_store::tests::interrupted_writer",
            ])
            .env("CODEMOD_TEST_WRITER_DIR", &run_dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !run_dir.join("writer-ready").exists() && Instant::now() < deadline {
            assert!(writer.try_wait().unwrap().is_none(), "writer exited early");
            std::thread::sleep(Duration::from_millis(10));
        }
        writer.kill().unwrap();
        assert!(!writer.wait().unwrap().success());
        assert!(run_dir.join("writer-ready").is_file(), "writer never began");
        let reopened = LocalStore::new(dir.path());
        let _state = reopened.lock_run_state("a").unwrap();
        assert_eq!(reopened.get_run("a").unwrap().unwrap().id, "a");
        assert_eq!(
            reopened.load_steps("a").unwrap(),
            [step(0, StepStatus::Succeeded), step(2, StepStatus::Partial),]
        );

        reopened
            .put_step("a", &step(1, StepStatus::Succeeded))
            .unwrap();
        assert_eq!(
            reopened.load_steps("a").unwrap(),
            [
                step(0, StepStatus::Succeeded),
                step(1, StepStatus::Succeeded),
                step(2, StepStatus::Partial),
            ]
        );
    }

    #[test]
    #[ignore = "subprocess fixture for local_store_recovers_an_interrupted_write_and_keeps_legacy_steps"]
    fn interrupted_writer() {
        use std::io::Write as _;
        let Some(dir) = std::env::var_os("CODEMOD_TEST_WRITER_DIR").map(PathBuf::from) else {
            return;
        };
        let store = LocalStore::new(dir.parent().unwrap());
        let _state = store.lock_run_state("a").unwrap();
        let _run = store.acquire("a").unwrap().unwrap();
        let mut log = fs::OpenOptions::new()
            .append(true)
            .open(dir.join("steps.jsonl"))
            .unwrap();
        log.write_all(b"{\"position\":1,\"label\":\"unfinished")
            .unwrap();
        log.sync_all().unwrap();
        fs::write(dir.join("run.123-0.tmp"), b"{\"status\":").unwrap();
        fs::write(dir.join("writer-ready"), b"").unwrap();
        std::thread::park();
    }

    #[test]
    fn canceled_and_rejected_runs_are_resumed_only_when_forced() {
        for (outcome, status) in [
            (Outcome::Canceled, RunStatus::Canceled),
            (Outcome::Rejected("ship?".into()), RunStatus::Rejected),
        ] {
            let mut run = record("a", 1);
            run.conclude(outcome);
            assert_eq!(run.status, status);
            assert!(run.ended_at.is_some());
            assert_eq!(run.reopen(false), Err(NotResumable::Ended(status)));
            assert_eq!(run.status, status);
            run.reopen(true).unwrap();
            assert_eq!(run.status, RunStatus::Running);
        }
        let mut run = record("a", 1);
        run.conclude(Outcome::Rejected("ship?".into()));
        assert_eq!(run.error.as_deref(), Some("rejected: ship?"));
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
        let unreadable = store.list_unreadable().unwrap();
        assert_eq!(unreadable.len(), 1, "{unreadable:?}");
        assert_eq!(unreadable[0].id, "broken");
        assert_eq!(unreadable[0].codemod_version, None);
    }

    #[test]
    fn a_record_a_newer_version_wrote_is_listed_as_unreadable_with_its_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = LocalStore::new(dir.path());
        fs::create_dir(dir.path().join("new")).unwrap();
        fs::write(
            dir.path().join("new").join("run.json"),
            br#"{"id":"new","status":"teleported","codemodVersion":"9.0.0","startedAt":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(store.list_runs(10).unwrap().is_empty());
        let unreadable = store.list_unreadable().unwrap();
        assert_eq!(unreadable[0].id, "new");
        assert_eq!(unreadable[0].codemod_version.as_deref(), Some("9.0.0"));
        assert!(unreadable[0].started_at.is_some());
    }

    fn targets_are_locked_one_run_at_a_time(store: &dyn RunStore) {
        let target = Path::new("/work/app");
        let first = store.lock_target(target, "a").unwrap().ok().unwrap();
        let held = store.lock_target(target, "b").unwrap().err().unwrap();
        assert_eq!(held.run_id, "a");
        assert_eq!(held.pid, Some(std::process::id()));
        // Another target is free.
        let other = store.lock_target(Path::new("/work/lib"), "b").unwrap();
        assert!(other.is_ok());
        drop(first);
        assert!(store.lock_target(target, "b").unwrap().is_ok());
    }

    #[test]
    fn memory_store_locks_targets() {
        targets_are_locked_one_run_at_a_time(&MemoryStore::default());
    }

    #[test]
    fn local_store_locks_targets_and_takes_over_a_killed_runs_lock() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = LocalStore::new(dir.path());
        targets_are_locked_one_run_at_a_time(&store);
        // A run that was killed left its lock file behind: unlocked, it is
        // not a holder.
        let target = Path::new("/work/killed");
        let path = store.target_lock_path(target);
        fs::write(
            &path,
            r#"{"runId":"gone","pid":999999,"target":"/work/killed"}"#,
        )
        .unwrap();
        let lease = store.lock_target(target, "c").unwrap();
        assert!(lease.is_ok());
        assert!(fs::read_to_string(&path).unwrap().contains("\"c\""));
    }

    #[test]
    fn without_file_locks_a_target_is_held_while_its_process_lives() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("t.lock");
        let holder = |run: &str, pid: u32| TargetHolder {
            run_id: run.into(),
            pid: Some(pid),
            target: "/work/app".into(),
        };
        // A process that is gone: its lock is cleared.
        fs::write(
            &path,
            serde_json::to_string(&holder("gone", 999_999)).unwrap(),
        )
        .unwrap();
        let lease = LocalStore::lock_target_by_pid(path.clone(), holder("a", std::process::id()))
            .unwrap()
            .ok()
            .unwrap();
        // One that lives: held.
        let held = LocalStore::lock_target_by_pid(path.clone(), holder("b", 1)).unwrap();
        assert_eq!(held.err().unwrap().run_id, "a");
        drop(lease);
        assert!(!path.exists());
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
