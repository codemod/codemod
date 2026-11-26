//! Numbers a run's events and keeps them in the run's journal in the run
//! store, which `workflow attach` replays. Every way of running a workflow
//! (text, JSONL, the launcher) goes through a [`Recorder`], so a run's
//! events are numbered the same everywhere, and keep counting up when the
//! run is resumed.

use std::sync::{Arc, Mutex};

use butterflow_state::run_store::RunStore;
use codemod_workflow::events::{Event, EventSink};
use serde_json::Value;

/// Where a [`Recorder`] sends events once numbered.
pub(crate) trait Stamped: Send + Sync {
    /// `record` is `event` as JSON, with its number (`seq`) and time (`ts`).
    fn emit(&self, event: Event, record: &Value);
}

/// Past this size a journal keeps only the events that say how the run goes
/// (steps starting and finishing, questions, the run's end), not logs,
/// progress, agent activity or diffs; attaching still sees how it ends.
pub(crate) const JOURNAL_LIMIT: u64 = 64 * 1024 * 1024;

pub(crate) struct Recorder {
    store: Arc<dyn RunStore>,
    /// The run whose journal this writes; `None` for dry runs, which are
    /// not saved.
    run_id: Option<String>,
    state: Mutex<State>,
    out: Arc<dyn Stamped>,
}

struct State {
    /// The next event's number.
    seq: u64,
    /// How big the journal is.
    bytes: u64,
}

impl Recorder {
    /// Numbering continues from the run's journal, when it has one.
    pub fn new(store: Arc<dyn RunStore>, run_id: Option<String>, out: Arc<dyn Stamped>) -> Self {
        let (seq, bytes) = run_id
            .as_deref()
            .map(|id| resume_point(store.as_ref(), id))
            .unwrap_or((0, 0));
        Self {
            store,
            run_id,
            state: Mutex::new(State { seq, bytes }),
            out,
        }
    }
}

/// The number after the journal's last event, and the journal's size.
fn resume_point(store: &dyn RunStore, run_id: &str) -> (u64, u64) {
    let Ok((lines, size)) = store.read_journal(run_id, 0) else {
        return (0, 0);
    };
    let next = lines
        .iter()
        .rev()
        .find_map(|line| {
            serde_json::from_str::<Value>(line)
                .ok()?
                .get("seq")?
                .as_u64()
        })
        .map_or(0, |seq| seq + 1);
    (next, size)
}

/// Events a full journal leaves out.
fn bulky(event: &Event) -> bool {
    matches!(
        event,
        Event::StepLog { .. }
            | Event::StepProgress { .. }
            | Event::StepAgent { .. }
            | Event::RunLog { .. }
            | Event::StepDiff { .. }
            | Event::RunDiff { .. }
    )
}

impl EventSink for Recorder {
    fn emit(&self, event: Event) {
        // Held while passing the event on, so events come out in `seq`
        // order even when several worker threads report at once.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut record = serde_json::to_value(&event).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut record {
            map.insert("seq".into(), state.seq.into());
            map.insert("ts".into(), chrono::Utc::now().to_rfc3339().into());
        }
        state.seq += 1;
        if let Some(run_id) = &self.run_id {
            if state.bytes <= JOURNAL_LIMIT || !bulky(&event) {
                let line = record.to_string();
                // A journal that cannot be written loses attach, not the run.
                if self.store.append_journal(run_id, &line).is_ok() {
                    state.bytes += line.len() as u64 + 1;
                }
            }
        }
        self.out.emit(event, &record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterflow_state::run_store::{MemoryStore, RunRecord, RunSettings, RunStatus};

    #[derive(Default)]
    struct Kept(Mutex<Vec<Value>>);

    impl Stamped for Kept {
        fn emit(&self, _event: Event, record: &Value) {
            self.0.lock().unwrap().push(record.clone());
        }
    }

    fn run(store: &dyn RunStore, id: &str) {
        store
            .put_run(&RunRecord {
                id: id.into(),
                name: None,
                workflow_path: "workflow.ts".into(),
                target: ".".into(),
                params: Value::Null,
                status: RunStatus::Running,
                started_at: chrono::Utc::now(),
                ended_at: None,
                awaiting: None,
                question: None,
                error: None,
                output: None,
                exit_code: None,
                settings: RunSettings::default(),
                codemod_version: None,
            })
            .unwrap();
    }

    fn log(line: &str) -> Event {
        Event::RunLog {
            line: line.into(),
            step: None,
        }
    }

    #[test]
    fn numbering_goes_on_where_the_journal_left_off() {
        let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
        run(store.as_ref(), "a");
        let kept = Arc::new(Kept::default());
        let first = Recorder::new(Arc::clone(&store), Some("a".into()), kept.clone());
        first.emit(log("one"));
        first.emit(log("two"));
        let second = Recorder::new(Arc::clone(&store), Some("a".into()), kept.clone());
        second.emit(log("three"));

        let seqs: Vec<u64> = kept
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|record| record["seq"].as_u64().unwrap())
            .collect();
        assert_eq!(seqs, [0, 1, 2]);
        let (journal, _) = store.read_journal("a", 0).unwrap();
        assert_eq!(journal.len(), 3);
        let last: Value = serde_json::from_str(&journal[2]).unwrap();
        assert_eq!(last["line"], "three");
        assert_eq!(last["seq"], 2);
    }

    #[test]
    fn dry_runs_keep_no_journal() {
        let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
        run(store.as_ref(), "a");
        let kept = Arc::new(Kept::default());
        Recorder::new(Arc::clone(&store), None, kept.clone()).emit(log("one"));
        assert_eq!(kept.0.lock().unwrap()[0]["seq"], 0);
        assert!(store.read_journal("a", 0).unwrap().0.is_empty());
    }
}
