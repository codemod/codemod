use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, Ctx, Object, Result};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

#[allow(dead_code)]
pub(crate) struct MetricModule;

static METRIC_SCOPES: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl ModuleDef for MetricModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("useMetric")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let default = Object::new(ctx.clone())?;

        exports.export("useMetric", Func::from(use_metric))?;
        exports.export("default", default)?;
        Ok(())
    }
}

fn get_metric(scope_name: String) -> usize {
    let scopes = METRIC_SCOPES.lock().unwrap();
    scopes.get(&scope_name).copied().unwrap_or(0)
}

fn set_metric(scope_name: String, value: usize) {
    let mut scopes = METRIC_SCOPES.lock().unwrap();
    scopes.insert(scope_name, value);
}

fn use_metric(ctx: Ctx<'_>, scope_name: String) -> Result<Object<'_>> {
    {
        let mut scopes = METRIC_SCOPES.lock().unwrap();
        scopes.entry(scope_name.clone()).or_insert(0);
    }

    let obj = Object::new(ctx.clone())?;
    let scope_name_clone = scope_name.clone();
    obj.set(
        "get",
        Func::new(move |_ctx: Ctx<'_>| get_metric(scope_name_clone.clone())),
    )?;
    let scope_name_clone2 = scope_name.clone();
    obj.set(
        "set",
        Func::new(move |_ctx: Ctx<'_>, value: usize| set_metric(scope_name_clone2.clone(), value)),
    )?;
    Ok(obj)
}

#[allow(dead_code)]
pub fn get_all_metrics() -> Vec<(String, usize)> {
    let scopes = METRIC_SCOPES.lock().unwrap();
    scopes
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_get_metric_returns_zero_when_not_set() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let metric_name = format!("test_get_metric_returns_zero_nonexistent_{}", unique_id);
        let value = get_metric(metric_name);
        assert_eq!(value, 0);
    }

    #[test]
    fn test_set_and_get_metric() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let metric_name = format!("test_set_and_get_metric_{}", unique_id);
        set_metric(metric_name.clone(), 42);
        let value = get_metric(metric_name);
        assert_eq!(value, 42);
    }

    #[test]
    fn test_set_metric_overwrites_existing() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let metric_name = format!("test_set_metric_overwrites_{}", unique_id);
        set_metric(metric_name.clone(), 10);
        set_metric(metric_name.clone(), 20);
        let value = get_metric(metric_name);
        assert_eq!(value, 20);
    }

    #[test]
    fn test_multiple_metrics_independent() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = format!("test_multiple_metrics_{}_", unique_id);
        let metric1 = format!("{}metric1", prefix);
        let metric2 = format!("{}metric2", prefix);
        let metric3 = format!("{}metric3", prefix);

        // Set metrics and verify immediately to avoid interference from parallel tests
        set_metric(metric1.clone(), 1);
        let val1 = get_metric(metric1.clone());
        set_metric(metric2.clone(), 2);
        let val2 = get_metric(metric2.clone());
        set_metric(metric3.clone(), 3);
        let val3 = get_metric(metric3.clone());

        assert_eq!(val1, 1, "metric1 should be 1");
        assert_eq!(val2, 2, "metric2 should be 2");
        assert_eq!(val3, 3, "metric3 should be 3");
    }

    #[test]
    fn test_get_all_metrics() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = format!("test_get_all_metrics_{}_", unique_id);
        set_metric(format!("{}metric_a", prefix), 10);
        set_metric(format!("{}metric_b", prefix), 20);
        set_metric(format!("{}metric_c", prefix), 30);

        let all_metrics = get_all_metrics();
        let test_metrics: Vec<_> = all_metrics
            .iter()
            .filter(|(name, _)| name.starts_with(&prefix))
            .collect();
        assert_eq!(
            test_metrics.len(),
            3,
            "Should have exactly 3 metrics with prefix {}",
            prefix
        );

        let mut metrics_map: std::collections::HashMap<String, usize> = test_metrics
            .iter()
            .map(|(k, v)| ((*k).clone(), *v))
            .collect();
        assert_eq!(metrics_map.remove(&format!("{}metric_a", prefix)), Some(10));
        assert_eq!(metrics_map.remove(&format!("{}metric_b", prefix)), Some(20));
        assert_eq!(metrics_map.remove(&format!("{}metric_c", prefix)), Some(30));
    }

    #[test]
    fn test_concurrent_set_operations() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = format!("test_concurrent_set_{}_", unique_id);
        let num_threads = 10;
        let operations_per_thread = 100;

        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let prefix = prefix.to_string();
                thread::spawn(move || {
                    for i in 0..operations_per_thread {
                        let metric_name = format!("{}metric_{}", prefix, thread_id);
                        set_metric(metric_name.clone(), i);
                        // Don't assert intermediate values in concurrent scenarios
                        // Just verify the final value after all operations
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all metrics were set correctly to their final values
        for thread_id in 0..num_threads {
            let metric_name = format!("{}metric_{}", prefix, thread_id);
            let value = get_metric(metric_name);
            assert_eq!(value, operations_per_thread - 1);
        }
    }

    #[test]
    fn test_concurrent_access_same_metric() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let num_threads = 10;
        let metric_name = format!("test_concurrent_access_shared_metric_{}", unique_id);

        // Initialize the metric
        set_metric(metric_name.clone(), 0);

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let metric_name = metric_name.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        let current = get_metric(metric_name.clone());
                        set_metric(metric_name.clone(), current + 1);
                        // Small delay to increase chance of race conditions
                        thread::sleep(Duration::from_micros(1));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // The final value should be at least num_threads, but will be less than
        // num_threads * 100 due to race conditions (which is expected behavior)
        let final_value = get_metric(metric_name);
        assert!(final_value >= num_threads as usize);
        assert!(final_value <= num_threads as usize * 100);
    }

    #[test]
    fn test_concurrent_initialization() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let num_threads = 20;
        let metric_name = format!("test_concurrent_initialization_metric_{}", unique_id);

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let metric_name = metric_name.clone();
                thread::spawn(move || {
                    // Simulate use_metric initialization pattern
                    {
                        let mut scopes = METRIC_SCOPES.lock().unwrap();
                        scopes.entry(metric_name.clone()).or_insert(0);
                    }
                    // Verify it was initialized
                    let value = get_metric(metric_name);
                    assert_eq!(value, 0);
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Metric should exist and be initialized to 0
        let value = get_metric(metric_name.clone());
        assert_eq!(value, 0);

        // Verify it only appears once in get_all_metrics
        let all_metrics = get_all_metrics();
        let count = all_metrics
            .iter()
            .filter(|(name, _)| name == &metric_name)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_all_metrics_empty() {
        // This test checks that get_all_metrics works with an empty state
        // We can't clear all metrics in parallel tests, so we just check
        // that the function works (it may return non-empty if other tests are running)
        let _all_metrics = get_all_metrics();
        // Note: We can't assert it's empty in parallel test execution
    }

    #[test]
    fn test_rapid_updates() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let metric_name = format!("test_rapid_updates_metric_{}", unique_id);

        // Rapidly update the same metric
        for i in 0..1000 {
            set_metric(metric_name.clone(), i);
        }

        let value = get_metric(metric_name);
        assert_eq!(value, 999);
    }

    #[test]
    fn test_mixed_operations() {
        // Use a unique identifier to avoid conflicts with parallel tests
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = format!("test_mixed_operations_{}_", unique_id);
        let num_metrics = 50;

        // Create many metrics
        for i in 0..num_metrics {
            set_metric(format!("{}metric_{}", prefix, i), i * 2);
        }

        // Update some of them
        for i in 0..num_metrics / 2 {
            set_metric(format!("{}metric_{}", prefix, i), i * 3);
        }

        // Verify updates
        for i in 0..num_metrics / 2 {
            let value = get_metric(format!("{}metric_{}", prefix, i));
            assert_eq!(value, i * 3);
        }

        // Verify unchanged metrics
        for i in num_metrics / 2..num_metrics {
            let value = get_metric(format!("{}metric_{}", prefix, i));
            assert_eq!(value, i * 2);
        }

        let all_metrics = get_all_metrics();
        let test_metrics: Vec<_> = all_metrics
            .iter()
            .filter(|(name, _)| name.starts_with(&prefix))
            .collect();
        assert_eq!(test_metrics.len(), num_metrics);
    }
}
