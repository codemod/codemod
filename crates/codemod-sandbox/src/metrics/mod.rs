use dashmap::DashMap;
use rquickjs::class::Trace;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Opt, Class, Ctx, Exception, JsLifetime, Object, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Type alias for metrics data structure (used for returning data)
pub type MetricsData = HashMap<String, HashMap<String, u64>>;

/// Inner storage using atomics for lock-free increments
type MetricsStorage = DashMap<String, DashMap<String, AtomicU64>>;

/// Metrics context for a codemod execution run.
///
/// Uses lock-free atomic counters for high-performance concurrent increments.
/// This is a simple container that the caller creates and passes into execution.
/// After execution, the caller can retrieve the collected metrics.
///
/// # Example
/// ```ignore
/// let metrics = MetricsContext::new();
///
/// // Pass to execution...
/// execute_codemod_sync(InMemoryExecutionOptions {
///     metrics_context: Some(metrics.clone()),
///     ...
/// });
///
/// // After execution, retrieve metrics
/// let data = metrics.get_all();
/// ```
#[derive(Clone, Default)]
pub struct MetricsContext {
    data: Arc<MetricsStorage>,
}

impl std::fmt::Debug for MetricsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsContext")
            .field("data", &self.get_all())
            .finish()
    }
}

unsafe impl<'js> JsLifetime<'js> for MetricsContext {
    type Changed<'to> = MetricsContext;
}

impl MetricsContext {
    /// Create a new empty metrics context
    pub fn new() -> Self {
        Self {
            data: Arc::new(DashMap::new()),
        }
    }

    /// Increment a metric by a given amount (lock-free atomic operation)
    pub fn increment(&self, metric_name: &str, label: &str, amount: u64) {
        // Get or create the inner map for this metric
        let metric_map = self
            .data
            .entry(metric_name.to_string())
            .or_insert_with(DashMap::new);

        // Get or create the counter for this label and atomically increment
        metric_map
            .entry(label.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(amount, Ordering::Relaxed);
    }

    /// Get a specific metric's values
    pub fn get(&self, metric_name: &str) -> HashMap<String, u64> {
        self.data
            .get(metric_name)
            .map(|metric_map| {
                metric_map
                    .iter()
                    .map(|entry| (entry.key().clone(), entry.value().load(Ordering::Relaxed)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all metrics
    pub fn get_all(&self) -> MetricsData {
        self.data
            .iter()
            .map(|entry| {
                let metric_name = entry.key().clone();
                let labels: HashMap<String, u64> = entry
                    .value()
                    .iter()
                    .map(|label_entry| {
                        (
                            label_entry.key().clone(),
                            label_entry.value().load(Ordering::Relaxed),
                        )
                    })
                    .collect();
                (metric_name, labels)
            })
            .collect()
    }

    /// Check if there are any metrics
    pub fn is_empty(&self) -> bool {
        self.data.is_empty() || self.data.iter().all(|entry| entry.value().is_empty())
    }
}

/// MetricAtom class exposed to JavaScript.
/// Holds a reference to the shared MetricsContext.
#[derive(Clone)]
#[rquickjs::class]
pub struct MetricAtom {
    name: String,
    context: MetricsContext,
}

impl Trace<'_> for MetricAtom {
    fn trace<'a>(&self, _tracer: rquickjs::class::Tracer<'a, '_>) {
        // No JavaScript values to trace
    }
}

unsafe impl<'js> JsLifetime<'js> for MetricAtom {
    type Changed<'to> = MetricAtom;
}

#[rquickjs::methods]
impl MetricAtom {
    /// Increment the metric for a given label
    #[qjs(rename = "increment")]
    pub fn increment<'js>(
        &self,
        _ctx: Ctx<'js>,
        label: String,
        amount: Opt<rquickjs::Value<'js>>,
    ) -> Result<()> {
        let increment_amount = match amount.0 {
            Some(val) if val.is_int() => val.as_int().unwrap_or(1) as u64,
            Some(val) if val.is_float() => val.as_float().unwrap_or(1.0) as u64,
            _ => 1,
        };

        self.context.increment(&self.name, &label, increment_amount);
        Ok(())
    }

    /// Get the current values for this metric
    #[qjs(rename = "getValues")]
    pub fn get_values<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let values = self.context.get(&self.name);
        let obj = Object::new(ctx.clone())?;
        for (label, count) in values {
            obj.set(label, count)?;
        }
        Ok(obj)
    }

    /// Get the metric name
    #[qjs(get, rename = "name")]
    pub fn name(&self) -> String {
        self.name.clone()
    }
}

/// Module definition for metrics.
/// Gets the MetricsContext from runtime userdata when evaluated.
pub struct MetricsModule;

impl ModuleDef for MetricsModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("useMetricAtom")?;
        declare.declare("MetricAtom")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        // Get the metrics context from the runtime's userdata
        let metrics_context: MetricsContext = ctx
            .userdata::<MetricsContext>()
            .ok_or_else(|| {
                Exception::throw_message(ctx, "MetricsContext not found in runtime userdata")
            })?
            .clone();

        let default = Object::new(ctx.clone())?;

        // Create the useMetricAtom function that captures the context
        let context_for_closure = metrics_context.clone();
        let use_metric_atom = rquickjs::Function::new(
            ctx.clone(),
            move |_ctx: Ctx<'_>, name: String| -> Result<MetricAtom> {
                Ok(MetricAtom {
                    name,
                    context: context_for_closure.clone(),
                })
            },
        )?;

        Class::<MetricAtom>::define(&default)?;
        default.set("useMetricAtom", use_metric_atom.clone())?;

        exports.export("useMetricAtom", use_metric_atom)?;
        exports.export("MetricAtom", Class::<MetricAtom>::create_constructor(ctx)?)?;
        exports.export("default", default)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_context_basic() {
        let ctx = MetricsContext::new();

        ctx.increment("test-metric", "label1", 1);
        ctx.increment("test-metric", "label1", 1);
        ctx.increment("test-metric", "label2", 5);

        let values = ctx.get("test-metric");
        assert_eq!(values.get("label1"), Some(&2));
        assert_eq!(values.get("label2"), Some(&5));
    }

    #[test]
    fn test_metrics_context_multiple_metrics() {
        let ctx = MetricsContext::new();

        ctx.increment("metric1", "a", 1);
        ctx.increment("metric2", "b", 2);
        ctx.increment("metric1", "a", 3);

        assert_eq!(ctx.get("metric1").get("a"), Some(&4));
        assert_eq!(ctx.get("metric2").get("b"), Some(&2));
    }

    #[test]
    fn test_get_all_metrics() {
        let ctx = MetricsContext::new();

        ctx.increment("prop-usage", "title", 10);
        ctx.increment("prop-usage", "placeholder", 5);
        ctx.increment("component-count", "Button", 100);

        let all = ctx.get_all();

        assert_eq!(all.len(), 2);
        assert!(all.contains_key("prop-usage"));
        assert!(all.contains_key("component-count"));
    }

    #[test]
    fn test_is_empty() {
        let ctx = MetricsContext::new();
        assert!(ctx.is_empty());

        ctx.increment("test", "label", 1);
        assert!(!ctx.is_empty());
    }

    #[test]
    fn test_clone_shares_data() {
        let ctx1 = MetricsContext::new();
        let ctx2 = ctx1.clone();

        ctx1.increment("shared", "label", 10);

        // Both should see the same data
        assert_eq!(ctx1.get("shared").get("label"), Some(&10));
        assert_eq!(ctx2.get("shared").get("label"), Some(&10));

        ctx2.increment("shared", "label", 5);
        assert_eq!(ctx1.get("shared").get("label"), Some(&15));
    }

    #[test]
    fn test_concurrent_increments() {
        use std::thread;

        let ctx = MetricsContext::new();

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let ctx_clone = ctx.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        ctx_clone.increment("concurrent-test", &format!("thread-{}", i), 1);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let all = ctx.get_all();
        let concurrent_test = all.get("concurrent-test").unwrap();

        let total: u64 = concurrent_test.values().sum();
        assert_eq!(total, 1000);
    }

    #[test]
    fn test_concurrent_same_label() {
        use std::thread;

        let ctx = MetricsContext::new();

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let ctx_clone = ctx.clone();
                thread::spawn(move || {
                    for _ in 0..1000 {
                        ctx_clone.increment("concurrent-test", "same-label", 1);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let values = ctx.get("concurrent-test");
        assert_eq!(values.get("same-label"), Some(&10000));
    }
}
