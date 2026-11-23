//! `forEachFile(selector, callback, reducer, initial)`: each file's output is
//! folded into one value as the file finishes, in an isolate of its own, so
//! the step never holds every file's output at once.
//!
//! Files finish in any order, but the reducer sees their outputs in the
//! order of the step's files (sorted by path), so the result is the same on
//! every run: [`InOrder`] holds outputs that arrive early until those before
//! them are folded, and keeps workers from getting more than [`window`]
//! files ahead of the fold, so what it holds stays bounded too.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use codemod_deps::Registry;
use rquickjs::{async_with, CatchResultExt, Function, Module, Promise};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use super::isolate::{self, module_name, StepIsolates};
use super::{block_on, TaskContext};

/// The `reduce` of a `forEachFile` task: the extracted reducer, the values
/// it captures, and the value the fold starts from.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct Reduce {
    callback: String,
    #[serde(default)]
    captures: Value,
    initial: Value,
}

/// One output to fold: the file's path and what its callback returned, as JSON.
pub(crate) type Entry = (String, String);

/// What the reducer's thread needs, taken from the task before it starts.
pub(crate) struct Fold {
    options: codemod_sandbox::WorkerOptions<codemod_sandbox::sandbox::resolvers::OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
    name: String,
    module: String,
    captures: String,
    initial: String,
    workflow_path: std::path::PathBuf,
}

impl Fold {
    /// The reducer runs with what the step's callbacks may use.
    pub fn new(
        task: &TaskContext<'_>,
        reduce: &Reduce,
        isolates: &StepIsolates,
    ) -> Result<Self, String> {
        let module = task
            .run
            .callbacks
            .get(&reduce.callback)
            .ok_or_else(|| format!("unknown reducer '{}'", reduce.callback))?
            .clone();
        Ok(Self {
            options: isolates.worker_options(task, None),
            registry: isolates.registry.clone(),
            name: module_name(task, &reduce.callback),
            module,
            captures: serde_json::to_string(&reduce.captures).map_err(|e| e.to_string())?,
            initial: serde_json::to_string(&reduce.initial).map_err(|e| e.to_string())?,
            workflow_path: task.run.workflow_path.clone(),
        })
    }

    /// Fold every entry that arrives until the sender is dropped; the
    /// result, or why the reducer failed. Returning early drops `entries`,
    /// which tells [`InOrder`] to stop.
    ///
    /// `cancel` is the step's flag: a reducer whose promise is still
    /// pending when it is set stops waiting for it, so a step that times out
    /// or is canceled does not hang on a reducer that never settles.
    pub fn run(self, entries: Receiver<Entry>, cancel: &AtomicBool) -> Result<Value, String> {
        let Fold {
            options,
            registry,
            name,
            module,
            captures,
            initial,
            workflow_path,
        } = self;
        let json = block_on(async move {
            let sandbox = isolate::start(options, registry).await?;
            let json = async_with!(sandbox.context => |ctx| {
                let run = async {
                    let declared = Module::declare(ctx.clone(), name.as_str(), module.as_str())?;
                    crate::run::set_import_meta(&declared, &workflow_path)?;
                    let (evaluated, promise) = declared.eval()?;
                    promise.into_future::<()>().await?;
                    let init: Function = evaluated.get("__init")?;
                    init.call::<_, ()>((captures.as_str(), None::<&str>))?;
                    let start: Function = evaluated.get("__reduceStart")?;
                    start.call::<_, ()>((initial.as_str(),))?;
                    let one: Function = evaluated.get("__reduceOne")?;
                    while let Ok((path, output)) = entries.recv() {
                        let promise: Promise = one.call((output.as_str(), path.as_str()))?;
                        tokio::select! {
                            settled = promise.into_future::<()>() => settled?,
                            () = canceled(cancel) => {
                                return Err(rquickjs::Exception::throw_message(&ctx, "the step was canceled"));
                            }
                        }
                    }
                    let end: Function = evaluated.get("__reduceEnd")?;
                    end.call::<_, String>(())
                };
                run.await.catch(&ctx).map_err(|e| format!("the reducer failed: {e}"))
            })
            .await;
            // A `failStep` the reducer caught still fails the step.
            let failure = sandbox.take_side_effects().failure;
            sandbox.shutdown(SHUTDOWN_GRACE).await;
            if let Some(failure) = failure {
                return Err(format!("the reducer failed: {}", failure.message));
            }
            json
        })?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

/// Resolves once `cancel` is set.
async fn canceled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// How long the reducer's isolate waits, once the fold is done, for work it
/// left running.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How many files workers may get ahead of the fold: enough that they never
/// wait for it while it keeps up, few enough that the outputs held for it
/// stay small.
pub(crate) fn window(workers: usize, concurrency: usize) -> usize {
    (workers * concurrency * 4).max(1024)
}

/// Gives the fold each file's output in the order of the step's files,
/// whatever order they finish in.
pub(crate) struct InOrder {
    state: Mutex<InOrderState>,
    moved: Condvar,
    window: usize,
}

struct InOrderState {
    /// The first file not given on yet.
    next: usize,
    /// Files that finished before `next`'s: their outputs, if any.
    early: BTreeMap<usize, Option<Entry>>,
    /// `None` once the fold is over: it failed, or a worker did.
    sender: Option<SyncSender<Entry>>,
}

impl InOrder {
    pub fn new(sender: SyncSender<Entry>, window: usize) -> Self {
        Self {
            state: Mutex::new(InOrderState {
                next: 0,
                early: BTreeMap::new(),
                sender: Some(sender),
            }),
            moved: Condvar::new(),
            window,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, InOrderState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wait until file `index` is within the window of the fold. `false`
    /// when the fold is over or the step was canceled: process no more.
    pub fn wait_turn(&self, index: usize, cancel: &AtomicBool) -> bool {
        let mut state = self.lock();
        loop {
            if state.sender.is_none() || cancel.load(Ordering::SeqCst) {
                return false;
            }
            if index < state.next + self.window {
                return true;
            }
            state = self
                .moved
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// File `index` is done: `entry` is its output, `None` when it has none
    /// (it returned nothing, failed, or was not one of the step's files).
    pub fn put(&self, index: usize, entry: Option<Entry>) {
        let mut state = self.lock();
        if state.sender.is_none() {
            return;
        }
        state.early.insert(index, entry);
        loop {
            let next = state.next;
            let Some(entry) = state.early.remove(&next) else {
                break;
            };
            state.next += 1;
            let Some(entry) = entry else {
                continue;
            };
            // Blocks while the fold is behind; fails once it has stopped.
            let sent = state.sender.as_ref().is_some_and(|s| s.send(entry).is_ok());
            if !sent {
                state.sender = None;
                state.early.clear();
                break;
            }
        }
        drop(state);
        self.moved.notify_all();
    }

    /// End the fold: every file is done, or the step stops.
    pub fn finish(&self) {
        let mut state = self.lock();
        state.sender = None;
        state.early.clear();
        drop(state);
        self.moved.notify_all();
    }
}

/// Ends the fold when a worker stops before its files are done (it failed
/// or panicked), so other workers do not wait for files it will never give.
pub(crate) struct FinishOnDrop<'a>(pub Option<&'a InOrder>);

impl FinishOnDrop<'_> {
    pub fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for FinishOnDrop<'_> {
    fn drop(&mut self) {
        if let Some(in_order) = self.0 {
            in_order.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn outputs_go_on_in_file_order() {
        let (sender, receiver) = mpsc::sync_channel(64);
        let in_order = InOrder::new(sender, 1024);
        in_order.put(2, Some(("c".into(), "3".into())));
        in_order.put(1, None);
        assert!(receiver.try_recv().is_err());
        in_order.put(0, Some(("a".into(), "1".into())));
        in_order.put(3, Some(("d".into(), "4".into())));
        in_order.finish();
        let got: Vec<Entry> = receiver.iter().collect();
        assert_eq!(
            got,
            vec![
                ("a".to_string(), "1".to_string()),
                ("c".to_string(), "3".to_string()),
                ("d".to_string(), "4".to_string())
            ]
        );
    }

    #[test]
    fn workers_wait_for_the_window_and_stop_with_the_fold() {
        let (sender, receiver) = mpsc::sync_channel(64);
        let in_order = InOrder::new(sender, 2);
        let cancel = AtomicBool::new(false);
        assert!(in_order.wait_turn(1, &cancel));
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| in_order.wait_turn(2, &cancel));
            // File 2 is outside the window until file 0 is folded.
            std::thread::sleep(Duration::from_millis(200));
            assert!(!waiting.is_finished());
            in_order.put(0, None);
            assert!(waiting.join().unwrap());
        });
        // The fold stopped: nobody waits, nothing more is sent.
        drop(receiver);
        in_order.put(1, Some(("b".into(), "2".into())));
        assert!(!in_order.wait_turn(100, &cancel));
    }
}
