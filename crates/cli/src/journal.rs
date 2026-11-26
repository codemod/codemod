//! Numbers a run's events and keeps them in the run's journal in the run
//! store, which `workflow attach` replays. Every way of running a workflow
//! (text, JSONL, the launcher) goes through a [`Recorder`], so a run's
//! events are numbered the same everywhere, and keep counting up when the
//! run is resumed.

use std::collections::HashSet;
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

#[derive(Default)]
struct State {
    /// The next event's number.
    seq: u64,
    /// How big the journal is.
    bytes: u64,
    /// API notices already reported in this run, including before a resume.
    experimental: HashSet<String>,
}

impl Recorder {
    /// Numbering continues from the run's journal, when it has one.
    pub fn new(store: Arc<dyn RunStore>, run_id: Option<String>, out: Arc<dyn Stamped>) -> Self {
        let state = run_id
            .as_deref()
            .map(|id| resume_point(store.as_ref(), id))
            .unwrap_or_default();
        Self {
            store,
            run_id,
            state: Mutex::new(state),
            out,
        }
    }
}

/// The journal's numbering, size, and notices already sent.
fn resume_point(store: &dyn RunStore, run_id: &str) -> State {
    let Ok((lines, bytes)) = store.read_journal(run_id, 0) else {
        return State::default();
    };
    let seq = lines
        .iter()
        .rev()
        .find_map(|line| {
            serde_json::from_str::<Value>(line)
                .ok()?
                .get("seq")?
                .as_u64()
        })
        .map_or(0, |seq| seq + 1);
    let experimental = lines
        .iter()
        .filter(|line| line.contains("\"event\":\"experimental\""))
        .filter_map(|line| {
            let event = serde_json::from_str::<Value>(line).ok()?;
            if event.get("event")?.as_str()? != "experimental" {
                return None;
            }
            Some(event.get("api")?.as_str()?.to_owned())
        })
        .collect();
    State {
        seq,
        bytes,
        experimental,
    }
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
        if let Event::Experimental { api } = &event {
            if !state.experimental.insert(api.clone()) {
                return;
            }
        }
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
    fn experimental_notices_are_once_per_api_even_across_resume() {
        let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
        run(store.as_ref(), "a");
        let kept = Arc::new(Kept::default());
        let notice = |api: &str| Event::Experimental { api: api.into() };
        let first = Recorder::new(Arc::clone(&store), Some("a".into()), kept.clone());
        first.emit(notice("http()"));
        first.emit(notice("http()"));
        let second = Recorder::new(store, Some("a".into()), kept.clone());
        second.emit(notice("http()"));
        second.emit(notice("sleep()"));
        let events = kept.0.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["api"], "http()");
        assert_eq!(events[1]["api"], "sleep()");
        assert_eq!(events[1]["seq"], 1);
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
