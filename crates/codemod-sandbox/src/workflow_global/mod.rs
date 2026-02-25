use crate::ast_grep::serde::JsValue;
use dashmap::DashMap;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, prelude::Opt, Ctx, Exception, Object, Result};
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Condvar, LazyLock, Mutex};

static STEP_OUTPUTS_STORE: LazyLock<Mutex<HashMap<String, HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_step_output(
    output_name: &str,
    value: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let step_id = env::var("CODEMOD_STEP_ID").unwrap_or_default();

    {
        let mut store = STEP_OUTPUTS_STORE
            .lock()
            .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;
        store
            .entry(step_id)
            .or_default()
            .insert(output_name.to_string(), value.to_string());
    }

    Ok(())
}

pub fn get_step_output(
    step_id: &str,
    output_name: &str,
) -> std::result::Result<Option<String>, Box<dyn std::error::Error>> {
    let store = STEP_OUTPUTS_STORE
        .lock()
        .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;

    if let Some(outputs) = store.get(step_id) {
        if let Some(value) = outputs.get(output_name) {
            return Ok(Some(value.clone()));
        }
    }

    Ok(None)
}

pub fn get_step_outputs(
    step_id: &str,
) -> std::result::Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let store = STEP_OUTPUTS_STORE
        .lock()
        .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;

    if let Some(outputs) = store.get(step_id) {
        Ok(outputs.clone())
    } else {
        Ok(HashMap::new())
    }
}

/// Get or set step output atomically
/// If the output exists, returns it. If not, sets it to the provided value and returns it.
pub fn get_or_set_step_output(
    step_id: &str,
    output_name: &str,
    default_value: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let mut store = STEP_OUTPUTS_STORE
        .lock()
        .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;

    let outputs = store.entry(step_id.to_string()).or_default();

    if let Some(value) = outputs.get(output_name) {
        // Output already exists, return it
        Ok(value.clone())
    } else {
        // Output doesn't exist, set it and return the new value
        outputs.insert(output_name.to_string(), default_value.to_string());
        drop(store); // Release lock before notifying
        Ok(default_value.to_string())
    }
}

// ---------------------------------------------------------------------------
// SharedStateContext – cross-thread shared state for workflow execution
// ---------------------------------------------------------------------------

/// A single state entry storing a JSON value with an optional persist flag.
#[derive(Debug, Clone)]
pub struct StateEntry {
    pub value: serde_json::Value,
    pub persist: bool,
}

/// Per-key lock that tracks which thread holds it, enabling re-entrant
/// access from `get`/`set`/`unset` when the same thread holds `acquireLock`.
struct KeyLock {
    holder: Mutex<Option<std::thread::ThreadId>>,
    condvar: Condvar,
}

/// Shared state context that can be cloned across threads.
/// Backed by `DashMap` for concurrent reads/writes.
///
/// When a thread holds `acquireLock("key")`, all other threads' calls to
/// `get("key")`, `set("key", ..)`, and `unset("key")` will block until
/// the lock is released. The holder thread's own calls are re-entrant.
#[derive(Clone, Default)]
pub struct SharedStateContext {
    data: Arc<DashMap<String, StateEntry>>,
    /// Tracks keys that have been explicitly unset (for producing Remove diffs).
    removals: Arc<DashMap<String, ()>>,
    key_locks: Arc<DashMap<String, Arc<KeyLock>>>,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for SharedStateContext {
    type Changed<'to> = SharedStateContext;
}

impl SharedStateContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a context pre-populated with initial state (all entries marked persist=true).
    pub fn with_initial_state(initial: HashMap<String, serde_json::Value>) -> Self {
        let data = DashMap::new();
        for (key, value) in initial {
            data.insert(
                key,
                StateEntry {
                    value,
                    persist: true,
                },
            );
        }
        Self {
            data: Arc::new(data),
            removals: Arc::new(DashMap::new()),
            key_locks: Arc::new(DashMap::new()),
        }
    }

    /// Execute `f` while respecting any active `acquireLock` on `name`.
    ///
    /// If another thread holds the lock, blocks until it is released.
    /// If the current thread holds the lock, proceeds immediately (re-entrant).
    ///
    /// Uses `entry()` to atomically create-or-get the `KeyLock`, preventing a
    /// race where `get()` returns `None` but `acquire_lock` creates the entry
    /// before `f()` runs.  The per-key `Mutex` is held during `f` so that no
    /// `acquireLock` can start between our check and the data operation.
    fn with_key_guard<T>(&self, name: &str, f: impl FnOnce() -> T) -> T {
        let lock = self
            .key_locks
            .entry(name.to_string())
            .or_insert_with(|| {
                Arc::new(KeyLock {
                    holder: Mutex::new(None),
                    condvar: Condvar::new(),
                })
            })
            .clone();

        let current = std::thread::current().id();
        let mut holder = lock.holder.lock().unwrap();
        while let Some(tid) = *holder {
            if tid == current {
                break; // re-entrant: we hold the lock
            }
            holder = lock.condvar.wait(holder).unwrap();
        }
        // holder is either None or current thread — safe to proceed.
        // Keep the MutexGuard alive so no acquireLock can start mid-operation.
        let result = f();
        drop(holder);
        result
    }

    pub fn set(&self, name: &str, value: serde_json::Value, persist: bool) {
        self.with_key_guard(name, || {
            self.removals.remove(name);
            self.data
                .insert(name.to_string(), StateEntry { value, persist });
        });
    }

    pub fn get(&self, name: &str) -> Option<serde_json::Value> {
        self.with_key_guard(name, || {
            self.data.get(name).map(|entry| entry.value.clone())
        })
    }

    pub fn unset(&self, name: &str) {
        self.with_key_guard(name, || {
            self.data.remove(name);
            self.removals.insert(name.to_string(), ());
        });
    }

    /// Return all entries where `persist == true`.
    pub fn get_persistable(&self) -> HashMap<String, serde_json::Value> {
        self.data
            .iter()
            .filter(|entry| entry.value().persist)
            .map(|entry| (entry.key().clone(), entry.value().value.clone()))
            .collect()
    }

    /// Return keys that were explicitly unset.
    pub fn get_removals(&self) -> Vec<String> {
        self.removals.iter().map(|e| e.key().clone()).collect()
    }

    /// Acquire a named lock. Blocks until the lock is available.
    ///
    /// While held, other threads' `get`/`set`/`unset` calls on the same key
    /// will block. The holder thread's calls are re-entrant.
    ///
    /// Returns an `Arc<SharedLockGuard>` — call `release()` or let it drop.
    pub fn acquire_lock(&self, name: &str) -> Arc<SharedLockGuard> {
        let key_lock = self
            .key_locks
            .entry(name.to_string())
            .or_insert_with(|| {
                Arc::new(KeyLock {
                    holder: Mutex::new(None),
                    condvar: Condvar::new(),
                })
            })
            .clone();

        let current = std::thread::current().id();
        let mut holder = key_lock.holder.lock().unwrap();
        while let Some(tid) = *holder {
            if tid == current {
                // Re-entrant: same thread already holds this lock.
                // This is a programming error — return a no-op guard rather than deadlocking.
                drop(holder);
                return Arc::new(SharedLockGuard {
                    key_lock,
                    released: std::sync::atomic::AtomicBool::new(true), // already "released" — won't double-release
                });
            }
            holder = key_lock.condvar.wait(holder).unwrap();
        }
        *holder = Some(current);
        drop(holder);

        Arc::new(SharedLockGuard {
            key_lock,
            released: std::sync::atomic::AtomicBool::new(false),
        })
    }
}

/// Guard returned by `acquire_lock`. Call `release()` to unlock, or it will
/// release automatically on drop.
pub struct SharedLockGuard {
    key_lock: Arc<KeyLock>,
    released: std::sync::atomic::AtomicBool,
}

impl SharedLockGuard {
    pub fn release(&self) {
        if !self
            .released
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let mut holder = self.key_lock.holder.lock().unwrap();
            *holder = None;
            self.key_lock.condvar.notify_all();
        }
    }
}

impl Drop for SharedLockGuard {
    fn drop(&mut self) {
        self.release();
    }
}

// ---------------------------------------------------------------------------
// QuickJS wrapper functions for shared state
// ---------------------------------------------------------------------------

fn set_state_rjs(ctx: Ctx<'_>, name: String, value: JsValue, persist: Opt<bool>) -> Result<()> {
    let shared_state = ctx.userdata::<SharedStateContext>().ok_or_else(|| {
        Exception::throw_message(&ctx, "SharedStateContext not found in runtime userdata")
    })?;
    let persist = persist.0.unwrap_or(true);
    shared_state.set(&name, value.0, persist);
    Ok(())
}

fn get_state_rjs<'js>(ctx: Ctx<'js>, name: String) -> Result<rquickjs::Value<'js>> {
    let value = {
        let shared_state = ctx.userdata::<SharedStateContext>().ok_or_else(|| {
            Exception::throw_message(&ctx, "SharedStateContext not found in runtime userdata")
        })?;
        shared_state.get(&name)
    };
    match value {
        Some(val) => JsValue(val).into_js(&ctx),
        None => Ok(rquickjs::Value::new_undefined(ctx)),
    }
}

fn unset_state_rjs(ctx: Ctx<'_>, name: String) -> Result<()> {
    let shared_state = ctx.userdata::<SharedStateContext>().ok_or_else(|| {
        Exception::throw_message(&ctx, "SharedStateContext not found in runtime userdata")
    })?;
    shared_state.unset(&name);
    Ok(())
}

fn acquire_lock_rjs<'js>(ctx: Ctx<'js>, name: String) -> Result<rquickjs::Function<'js>> {
    let guard = {
        let shared_state = ctx.userdata::<SharedStateContext>().ok_or_else(|| {
            Exception::throw_message(&ctx, "SharedStateContext not found in runtime userdata")
        })?;
        shared_state.acquire_lock(&name)
    };

    // Return a one-shot release function.
    // We use Arc so the guard lives as long as the closure.
    rquickjs::Function::new(ctx, move || {
        guard.release();
    })
}

use rquickjs::IntoJs;

// ---------------------------------------------------------------------------
// Module definition
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) struct WorkflowGlobalModule;

impl ModuleDef for WorkflowGlobalModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("setStepOutput")?;
        declare.declare("getStepOutput")?;
        declare.declare("getOrSetStepOutput")?;
        declare.declare("setState")?;
        declare.declare("getState")?;
        declare.declare("unsetState")?;
        declare.declare("acquireLock")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let default = Object::new(ctx.clone())?;

        default.set("setStepOutput", Func::from(set_step_output_rjs))?;
        default.set("getStepOutput", Func::from(get_step_output_rjs))?;
        default.set("getOrSetStepOutput", Func::from(get_or_set_step_output_rjs))?;
        default.set("setState", Func::from(set_state_rjs))?;
        default.set("getState", Func::from(get_state_rjs))?;
        default.set("unsetState", Func::from(unset_state_rjs))?;
        default.set("acquireLock", Func::from(acquire_lock_rjs))?;

        exports.export("setStepOutput", Func::from(set_step_output_rjs))?;
        exports.export("getStepOutput", Func::from(get_step_output_rjs))?;
        exports.export("getOrSetStepOutput", Func::from(get_or_set_step_output_rjs))?;
        exports.export("setState", Func::from(set_state_rjs))?;
        exports.export("getState", Func::from(get_state_rjs))?;
        exports.export("unsetState", Func::from(unset_state_rjs))?;
        exports.export("acquireLock", Func::from(acquire_lock_rjs))?;

        exports.export("default", default)?;
        Ok(())
    }
}

fn set_step_output_rjs(ctx: Ctx<'_>, output_name: String, value: String) -> Result<()> {
    let result = set_step_output(&output_name, &value);
    result.map_err(|e| Exception::throw_message(&ctx, &format!("Failed to set step output: {e}")))
}

fn get_step_output_rjs(
    ctx: Ctx<'_>,
    step_id: String,
    output_name: String,
) -> Result<Option<String>> {
    let result = get_step_output(&step_id, &output_name);
    result.map_err(|e| Exception::throw_message(&ctx, &format!("Failed to get step output: {e}")))
}

fn get_or_set_step_output_rjs(
    ctx: Ctx<'_>,
    step_id: String,
    output_name: String,
    default_value: String,
) -> Result<String> {
    let result = get_or_set_step_output(&step_id, &output_name, &default_value);
    result.map_err(|e| {
        Exception::throw_message(&ctx, &format!("Failed to get or set step output: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_get_or_set_step_output_sets_when_empty() {
        let step_id = "test_step_1";
        let output_name = "test_output";
        let default_value = "default_value";

        let result = get_or_set_step_output(step_id, output_name, default_value).unwrap();
        assert_eq!(result, default_value);

        // Verify it was actually set
        let stored = get_step_output(step_id, output_name).unwrap();
        assert_eq!(stored, Some(default_value.to_string()));
    }

    #[test]
    fn test_get_or_set_step_output_gets_existing() {
        let step_id = "test_step_2";
        let output_name = "test_output";
        let initial_value = "initial_value";
        let default_value = "default_value";

        // Temporarily set the step ID for the test
        unsafe {
            std::env::set_var("CODEMOD_STEP_ID", step_id);
        }
        set_step_output(output_name, initial_value).unwrap();
        unsafe {
            std::env::remove_var("CODEMOD_STEP_ID");
        }

        // Try to get or set with different default
        let result = get_or_set_step_output(step_id, output_name, default_value).unwrap();

        // Should return the existing value, not the default
        assert_eq!(result, initial_value);
    }

    #[test]
    fn test_get_or_set_step_output_different_steps() {
        let step_id_1 = "test_step_3a";
        let step_id_2 = "test_step_3b";
        let output_name = "test_output";
        let value_1 = "value_1";
        let value_2 = "value_2";

        let result_1 = get_or_set_step_output(step_id_1, output_name, value_1).unwrap();
        let result_2 = get_or_set_step_output(step_id_2, output_name, value_2).unwrap();

        assert_eq!(result_1, value_1);
        assert_eq!(result_2, value_2);

        // Verify both are stored independently
        assert_eq!(
            get_step_output(step_id_1, output_name).unwrap(),
            Some(value_1.to_string())
        );
        assert_eq!(
            get_step_output(step_id_2, output_name).unwrap(),
            Some(value_2.to_string())
        );
    }

    #[test]
    fn test_get_or_set_step_output_concurrent_access() {
        let step_id = "test_step_4";
        let output_name = "test_output";
        let num_threads = 10;

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let step_id = step_id.to_string();
                let output_name = output_name.to_string();
                let default_value = format!("value_{}", i);

                thread::spawn(move || {
                    get_or_set_step_output(&step_id, &output_name, &default_value).unwrap()
                })
            })
            .collect();

        let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All threads should get the same value (the one set by the first thread to acquire the lock)
        let first_value = &results[0];
        for result in &results {
            assert_eq!(result, first_value);
        }

        // Verify the final stored value matches what all threads got
        let stored = get_step_output(step_id, output_name).unwrap();
        assert_eq!(stored, Some(first_value.clone()));
    }

    #[test]
    fn test_get_or_set_step_output_multiple_outputs_per_step() {
        let step_id = "test_step_5";
        let output_1 = "output_1";
        let output_2 = "output_2";
        let value_1 = "value_1";
        let value_2 = "value_2";

        let result_1 = get_or_set_step_output(step_id, output_1, value_1).unwrap();
        let result_2 = get_or_set_step_output(step_id, output_2, value_2).unwrap();

        assert_eq!(result_1, value_1);
        assert_eq!(result_2, value_2);

        // Verify both outputs are stored
        let all_outputs = get_step_outputs(step_id).unwrap();
        assert_eq!(all_outputs.len(), 2);
        assert_eq!(all_outputs.get(output_1), Some(&value_1.to_string()));
        assert_eq!(all_outputs.get(output_2), Some(&value_2.to_string()));
    }

    #[test]
    fn test_set_and_get_step_output_basic() {
        let output_name = "test_output_basic";
        let value = "test_value";

        unsafe {
            std::env::set_var("CODEMOD_STEP_ID", "test_step_6");
        }

        set_step_output(output_name, value).unwrap();
        let result = get_step_output("test_step_6", output_name).unwrap();

        unsafe {
            std::env::remove_var("CODEMOD_STEP_ID");
        }

        assert_eq!(result, Some(value.to_string()));
    }

    #[test]
    fn test_get_step_output_nonexistent() {
        let result = get_step_output("nonexistent_step", "nonexistent_output").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_get_step_outputs_empty() {
        let result = get_step_outputs("empty_step").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_concurrent_different_outputs() {
        let step_id = "test_step_7";
        let num_threads = 5;

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let step_id = step_id.to_string();
                let output_name = format!("output_{}", i);
                let value = format!("value_{}", i);

                thread::spawn(move || {
                    get_or_set_step_output(&step_id, &output_name, &value).unwrap()
                })
            })
            .collect();

        let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Each thread should have successfully set its own output
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result, &format!("value_{}", i));
        }

        // Verify all outputs are stored
        let all_outputs = get_step_outputs(step_id).unwrap();
        assert_eq!(all_outputs.len(), num_threads);
    }

    #[test]
    fn test_get_or_set_with_complex_json_value() {
        let step_id = "test_step_8";
        let output_name = "json_output";
        let json_value = r#"{"key": "value", "nested": {"array": [1, 2, 3]}}"#;

        let result = get_or_set_step_output(step_id, output_name, json_value).unwrap();
        assert_eq!(result, json_value);

        let stored = get_step_output(step_id, output_name).unwrap();
        assert_eq!(stored, Some(json_value.to_string()));
    }

    // SharedStateContext unit tests

    #[test]
    fn test_shared_state_set_get() {
        let ctx = SharedStateContext::new();
        ctx.set("key1", serde_json::json!("hello"), true);

        let val = ctx.get("key1");
        assert_eq!(val, Some(serde_json::json!("hello")));
    }

    #[test]
    fn test_shared_state_get_nonexistent() {
        let ctx = SharedStateContext::new();
        assert_eq!(ctx.get("missing"), None);
    }

    #[test]
    fn test_shared_state_unset() {
        let ctx = SharedStateContext::new();
        ctx.set("key1", serde_json::json!(42), true);
        ctx.unset("key1");

        assert_eq!(ctx.get("key1"), None);
        assert_eq!(ctx.get_removals(), vec!["key1".to_string()]);
    }

    #[test]
    fn test_shared_state_set_clears_removal() {
        let ctx = SharedStateContext::new();
        ctx.set("key1", serde_json::json!(1), true);
        ctx.unset("key1");
        assert!(ctx.get_removals().contains(&"key1".to_string()));

        ctx.set("key1", serde_json::json!(2), true);
        assert!(!ctx.get_removals().contains(&"key1".to_string()));
        assert_eq!(ctx.get("key1"), Some(serde_json::json!(2)));
    }

    #[test]
    fn test_shared_state_persistable() {
        let ctx = SharedStateContext::new();
        ctx.set("persist_me", serde_json::json!("yes"), true);
        ctx.set("transient", serde_json::json!("no"), false);

        let persistable = ctx.get_persistable();
        assert_eq!(persistable.len(), 1);
        assert_eq!(
            persistable.get("persist_me"),
            Some(&serde_json::json!("yes"))
        );
    }

    #[test]
    fn test_shared_state_with_initial() {
        let mut initial = HashMap::new();
        initial.insert("pre".to_string(), serde_json::json!({"a": 1}));

        let ctx = SharedStateContext::with_initial_state(initial);
        assert_eq!(ctx.get("pre"), Some(serde_json::json!({"a": 1})));

        let persistable = ctx.get_persistable();
        assert!(persistable.contains_key("pre"));
    }

    #[test]
    fn test_shared_state_clone_shares_data() {
        let ctx1 = SharedStateContext::new();
        let ctx2 = ctx1.clone();

        ctx1.set("shared", serde_json::json!("value"), true);
        assert_eq!(ctx2.get("shared"), Some(serde_json::json!("value")));
    }

    #[test]
    fn test_shared_state_concurrent() {
        let ctx = SharedStateContext::new();
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let ctx = ctx.clone();
                thread::spawn(move || {
                    ctx.set(&format!("key_{i}"), serde_json::json!(i), true);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        for i in 0..10 {
            assert_eq!(ctx.get(&format!("key_{i}")), Some(serde_json::json!(i)));
        }
    }

    #[test]
    fn test_shared_state_acquire_lock() {
        let ctx = SharedStateContext::new();
        let guard = ctx.acquire_lock("my_lock");
        guard.release();
        // Should be able to acquire again
        let guard2 = ctx.acquire_lock("my_lock");
        guard2.release();
    }

    #[test]
    fn test_shared_state_lock_contention() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let ctx = SharedStateContext::new();
        let counter = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..5)
            .map(|_| {
                let ctx = ctx.clone();
                let counter = counter.clone();
                thread::spawn(move || {
                    let guard = ctx.acquire_lock("counter_lock");
                    let val = counter.load(Ordering::SeqCst);
                    // Simulate some work
                    thread::sleep(std::time::Duration::from_millis(1));
                    counter.store(val + 1, Ordering::SeqCst);
                    guard.release();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn test_get_blocks_during_acquire_lock() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let ctx = SharedStateContext::new();
        ctx.set("key", serde_json::json!(1), true);

        // Thread 1 acquires lock and updates value after a delay
        let guard = ctx.acquire_lock("key");
        let ctx2 = ctx.clone();
        let read_started = Arc::new(AtomicBool::new(false));
        let read_started_clone = read_started.clone();

        let reader = thread::spawn(move || {
            read_started_clone.store(true, Ordering::SeqCst);
            // This should block until the lock is released
            ctx2.get("key")
        });

        // Give the reader thread time to start and block
        while !read_started.load(Ordering::SeqCst) {
            thread::yield_now();
        }
        thread::sleep(std::time::Duration::from_millis(10));

        // Update value while holding the lock — reader should not see this yet
        ctx.set("key", serde_json::json!(42), true);

        // Release the lock — reader should now see 42
        guard.release();

        let result = reader.join().unwrap();
        assert_eq!(result, Some(serde_json::json!(42)));
    }

    #[test]
    fn test_lock_reentrant_for_holder() {
        let ctx = SharedStateContext::new();
        let _guard = ctx.acquire_lock("key");

        // The holder thread should be able to get/set without deadlock
        ctx.set("key", serde_json::json!("from_holder"), true);
        let val = ctx.get("key");
        assert_eq!(val, Some(serde_json::json!("from_holder")));

        ctx.unset("key");
        assert_eq!(ctx.get("key"), None);
    }
}
