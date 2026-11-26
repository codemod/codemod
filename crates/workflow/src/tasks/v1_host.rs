//! `__codemodV1Host`: what a codemod 1.x jssg transform shares with the
//! other files of its step, for the 1.x compatibility modules
//! (`js/v1.ts`, `codemod:workflow`'s state functions and `codemod:metrics`).
//!
//! Only runs of a workflow.yaml (a 1.x package) get it; see
//! [`crate::run::RunContext::v1_compat`]. In 1.x, transforms shared state
//! and metrics across the threads of a step, and later steps read the state.
//! Here they share them across the step's workers through this host, and
//! what they set and counted is returned with the step's result (`v1`), from
//! where the translated workflow body copies it into its `state` and adds up
//! the counts. So the side effect stays inside the step, and the body sees it
//! as the step's recorded result, which replays on resume.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rquickjs::{Ctx, Exception, Function, Object};
use serde_json::{json, Map, Value};

/// By metric name, then by cardinality (as canonical JSON): the cardinality
/// and its count.
type Metrics = BTreeMap<String, BTreeMap<String, (Value, f64)>>;

/// A step's shared 1.x state and metrics.
#[derive(Default)]
pub(crate) struct V1Host {
    /// What the step's transforms set (`Some`) or unset (`None`), and whether
    /// it outlives the step (`setState(name, value, persist)`).
    state: Mutex<BTreeMap<String, (Option<Value>, bool)>>,
    /// `acquireLock` names held now.
    held: Mutex<HashSet<String>>,
    released: Condvar,
    metrics: Mutex<Metrics>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl V1Host {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// `{ value }` or `{ unset: true }` when a transform of this step set or
    /// unset `name`; `None` when none did (the state from earlier steps
    /// applies).
    pub fn get(&self, name: &str) -> Option<Value> {
        lock(&self.state).get(name).map(|(value, _)| match value {
            Some(value) => json!({ "value": value }),
            None => json!({ "unset": true }),
        })
    }

    pub fn set(&self, name: &str, value: Option<Value>, persist: bool) {
        lock(&self.state).insert(name.to_string(), (value, persist));
    }

    /// Wait until no other holder has `name`, then take it. Gives up when the
    /// run is canceled.
    pub fn acquire(&self, name: &str, cancel: &AtomicBool) -> Result<(), String> {
        let mut held = lock(&self.held);
        while held.contains(name) {
            if cancel.load(Ordering::SeqCst) {
                return Err("canceled".into());
            }
            held = self
                .released
                .wait_timeout(held, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        held.insert(name.to_string());
        Ok(())
    }

    pub fn release(&self, name: &str) {
        lock(&self.held).remove(name);
        self.released.notify_all();
    }

    pub fn increment(&self, name: &str, cardinality: Value, amount: f64) {
        let key = cardinality.to_string();
        let mut metrics = lock(&self.metrics);
        let entry = metrics
            .entry(name.to_string())
            .or_default()
            .entry(key)
            .or_insert((cardinality, 0.0));
        entry.1 += amount;
    }

    /// `getEntries()`: this step's counts for `name`.
    pub fn entries(&self, name: &str) -> Value {
        entries_of(&lock(&self.metrics), name)
    }

    /// What the step set and counted, for its result; `None` when nothing.
    pub fn finish(&self) -> Option<Value> {
        let mut out = Map::new();
        let state = lock(&self.state);
        let mut set = Map::new();
        let mut unset = Vec::new();
        for (name, (value, persist)) in state.iter() {
            if !persist {
                continue;
            }
            match value {
                Some(value) => {
                    set.insert(name.clone(), value.clone());
                }
                None => unset.push(Value::String(name.clone())),
            }
        }
        if !set.is_empty() || !unset.is_empty() {
            out.insert("state".into(), json!({ "set": set, "unset": unset }));
        }
        let metrics = lock(&self.metrics);
        if !metrics.is_empty() {
            let metrics: Map<String, Value> = metrics
                .keys()
                .map(|name| (name.clone(), entries_of(&metrics, name)))
                .collect();
            out.insert("metrics".into(), Value::Object(metrics));
        }
        (!out.is_empty()).then_some(Value::Object(out))
    }
}

/// A metric's entries as `getEntries()` gives them: whole counts as integers.
fn entries_of(metrics: &Metrics, name: &str) -> Value {
    let count = |count: f64| {
        if count.fract() == 0.0 && count.abs() < 9_007_199_254_740_992.0 {
            json!(count as i64)
        } else {
            json!(count)
        }
    };
    Value::Array(
        metrics
            .get(name)
            .map(|entries| {
                entries
                    .values()
                    .map(|(cardinality, n)| json!({ "cardinality": cardinality, "count": count(*n) }))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Install `__codemodV1Host` in one worker's isolate. The locks this worker
/// holds are released by `releaseAll`, which the jssg adapter calls after
/// each file, so a transform that throws while holding one does not stall
/// the others.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    host: Arc<V1Host>,
    cancel: Arc<AtomicBool>,
) -> rquickjs::Result<()> {
    let object = Object::new(ctx.clone())?;
    let mine: Arc<Mutex<HashSet<String>>> = Arc::default();
    let parse = |ctx: &Ctx<'js>, text: &str| -> rquickjs::Result<Value> {
        serde_json::from_str(text).map_err(|e| Exception::throw_message(ctx, &e.to_string()))
    };

    let h = Arc::clone(&host);
    object.set(
        "get",
        Function::new(ctx.clone(), move |name: String| -> Option<String> {
            h.get(&name).map(|value| value.to_string())
        })?,
    )?;
    let h = Arc::clone(&host);
    object.set(
        "set",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, name: String, value: String, persist: bool| {
                let value = parse(&ctx, &value)?;
                h.set(&name, Some(value), persist);
                rquickjs::Result::Ok(())
            },
        )?,
    )?;
    let h = Arc::clone(&host);
    object.set(
        "unset",
        Function::new(ctx.clone(), move |name: String| h.set(&name, None, true))?,
    )?;
    let (h, held, stop) = (Arc::clone(&host), Arc::clone(&mine), Arc::clone(&cancel));
    object.set(
        "lock",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String| {
            if lock(&held).contains(&name) {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!("acquireLock('{name}'): this file holds it already; release it first"),
                ));
            }
            h.acquire(&name, &stop)
                .map_err(|e| Exception::throw_message(&ctx, &e))?;
            lock(&held).insert(name);
            Ok(())
        })?,
    )?;
    let (h, held) = (Arc::clone(&host), Arc::clone(&mine));
    object.set(
        "unlock",
        Function::new(ctx.clone(), move |name: String| {
            if lock(&held).remove(&name) {
                h.release(&name);
            }
        })?,
    )?;
    let (h, held) = (Arc::clone(&host), Arc::clone(&mine));
    object.set(
        "releaseAll",
        Function::new(ctx.clone(), move || {
            for name in std::mem::take(&mut *lock(&held)) {
                h.release(&name);
            }
        })?,
    )?;
    let h = Arc::clone(&host);
    object.set(
        "increment",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, name: String, cardinality: String, amount: f64| {
                let cardinality = parse(&ctx, &cardinality)?;
                h.increment(&name, cardinality, amount);
                rquickjs::Result::Ok(())
            },
        )?,
    )?;
    let h = Arc::clone(&host);
    object.set(
        "entries",
        Function::new(ctx.clone(), move |name: String| {
            h.entries(&name).to_string()
        })?,
    )?;
    ctx.globals().set("__codemodV1Host", object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_reports_persisted_state_and_metrics() {
        let host = V1Host::new();
        assert!(host.finish().is_none());
        host.set("a", Some(json!([1])), true);
        host.set("tmp", Some(json!(1)), false);
        host.set("gone", None, true);
        host.increment("m", json!({ "k": "x" }), 1.0);
        host.increment("m", json!({ "k": "x" }), 2.0);
        host.increment("m", json!({}), 1.0);
        assert_eq!(host.get("a"), Some(json!({ "value": [1] })));
        assert_eq!(host.get("gone"), Some(json!({ "unset": true })));
        assert_eq!(host.get("other"), None);
        assert_eq!(
            host.finish().unwrap(),
            json!({
                "state": { "set": { "a": [1] }, "unset": ["gone"] },
                "metrics": { "m": [
                    { "cardinality": { "k": "x" }, "count": 3 },
                    { "cardinality": {}, "count": 1 },
                ] },
            })
        );
    }

    #[test]
    fn a_lock_waits_for_its_holder() {
        let host = V1Host::new();
        let cancel = AtomicBool::new(false);
        host.acquire("k", &cancel).unwrap();
        let other = Arc::clone(&host);
        let waiter = std::thread::spawn(move || {
            let cancel = AtomicBool::new(false);
            other.acquire("k", &cancel).unwrap();
            other.release("k");
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiter.is_finished());
        host.release("k");
        waiter.join().unwrap();
    }

    #[test]
    fn a_canceled_run_stops_waiting() {
        let host = V1Host::new();
        host.acquire("k", &AtomicBool::new(false)).unwrap();
        assert!(host.acquire("k", &AtomicBool::new(true)).is_err());
    }
}
