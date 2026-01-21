use dashmap::DashMap;
use rquickjs::class::Trace;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Opt, Class, Ctx, Exception, JsLifetime, Object, Result};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Represents a set of cardinality dimensions for a metric.
/// Cardinality allows tracking metrics with multiple dimensions,
/// e.g., {propName: "className", propValue: "container"}.
///
/// The inner representation is a sorted Vec of key-value pairs to ensure
/// consistent hashing regardless of insertion order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cardinality(Vec<(String, String)>);

impl Cardinality {
    /// Create a new cardinality from key-value pairs.
    /// Pairs are automatically sorted by key for consistent hashing.
    pub fn new(mut pairs: Vec<(String, String)>) -> Self {
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        Self(pairs)
    }

    /// Create cardinality from a HashMap
    pub fn from_hashmap(map: HashMap<String, String>) -> Self {
        Self::new(map.into_iter().collect())
    }

    /// Convert to HashMap for external use
    pub fn to_hashmap(&self) -> HashMap<String, String> {
        self.0.iter().cloned().collect()
    }

    /// Get the inner pairs
    pub fn pairs(&self) -> &[(String, String)] {
        &self.0
    }
}

impl Hash for Cardinality {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Since pairs are sorted, this produces consistent hashes
        self.0.hash(state);
    }
}

impl From<HashMap<String, String>> for Cardinality {
    fn from(map: HashMap<String, String>) -> Self {
        Self::from_hashmap(map)
    }
}

/// A single metric entry with its cardinality dimensions and count
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MetricEntry {
    pub cardinality: HashMap<String, String>,
    pub count: u64,
}

/// Type alias for metrics data structure (used for returning data)
/// Maps metric_name -> list of (cardinality, count) entries
pub type MetricsData = HashMap<String, Vec<MetricEntry>>;

/// Inner storage using atomics for lock-free increments
type MetricsStorage = DashMap<String, DashMap<Cardinality, AtomicU64>>;

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

    /// Increment a metric by a given amount with cardinality dimensions (lock-free atomic operation)
    pub fn increment(&self, metric_name: &str, cardinality: Cardinality, amount: u64) {
        // Get or create the inner map for this metric
        let metric_map = self
            .data
            .entry(metric_name.to_string())
            .or_insert_with(DashMap::new);

        // Get or create the counter for this cardinality and atomically increment
        metric_map
            .entry(cardinality)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(amount, Ordering::Relaxed);
    }

    /// Get a specific metric's entries
    pub fn get(&self, metric_name: &str) -> Vec<MetricEntry> {
        self.data
            .get(metric_name)
            .map(|metric_map| {
                metric_map
                    .iter()
                    .map(|entry| MetricEntry {
                        cardinality: entry.key().to_hashmap(),
                        count: entry.value().load(Ordering::Relaxed),
                    })
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
                let entries: Vec<MetricEntry> = entry
                    .value()
                    .iter()
                    .map(|cardinality_entry| MetricEntry {
                        cardinality: cardinality_entry.key().to_hashmap(),
                        count: cardinality_entry.value().load(Ordering::Relaxed),
                    })
                    .collect();
                (metric_name, entries)
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
    /// Increment the metric with optional cardinality dimensions.
    ///
    /// Example:
    /// ```js
    /// metricAtom.increment(); // increment with empty cardinality
    /// metricAtom.increment({ propName: "className", propValue: "container" }); // with cardinality
    /// metricAtom.increment({ propName: "className" }, 5); // with cardinality and amount
    /// ```
    #[qjs(rename = "increment")]
    pub fn increment<'js>(
        &self,
        ctx: Ctx<'js>,
        cardinality: Opt<rquickjs::Value<'js>>,
        amount: Opt<rquickjs::Value<'js>>,
    ) -> Result<()> {
        let increment_amount = match amount.0 {
            Some(val) if val.is_int() => val.as_int().unwrap_or(1) as u64,
            Some(val) if val.is_float() => val.as_float().unwrap_or(1.0) as u64,
            _ => 1,
        };

        let pairs = match cardinality.0 {
            Some(val) if val.is_object() && !val.is_null() && !val.is_undefined() => {
                let obj = val
                    .as_object()
                    .ok_or_else(|| Exception::throw_message(&ctx, "Invalid cardinality object"))?;
                let mut pairs: Vec<(String, String)> = Vec::new();
                for key in obj.keys::<String>() {
                    let key = key?;
                    let value: rquickjs::Value = obj.get(&key)?;
                    // Skip undefined or null values
                    if value.is_undefined() || value.is_null() {
                        continue;
                    }
                    let value_str: String = value.get()?;
                    pairs.push((key, value_str));
                }
                pairs
            }
            _ => Vec::new(), // No cardinality or undefined/null
        };

        self.context
            .increment(&self.name, Cardinality::new(pairs), increment_amount);
        Ok(())
    }

    /// Get the current entries for this metric as an array of {cardinality, count} objects
    #[qjs(rename = "getEntries")]
    pub fn get_entries<'js>(&self, ctx: Ctx<'js>) -> Result<rquickjs::Array<'js>> {
        let entries = self.context.get(&self.name);
        let arr = rquickjs::Array::new(ctx.clone())?;

        for (i, entry) in entries.iter().enumerate() {
            let obj = Object::new(ctx.clone())?;
            let cardinality_obj = Object::new(ctx.clone())?;
            for (key, value) in &entry.cardinality {
                cardinality_obj.set(key.as_str(), value.as_str())?;
            }
            obj.set("cardinality", cardinality_obj)?;
            obj.set("count", entry.count)?;
            arr.set(i, obj)?;
        }

        Ok(arr)
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
    fn test_cardinality_ordering() {
        // Cardinality should be order-independent
        let c1 = Cardinality::new(vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ]);
        let c2 = Cardinality::new(vec![
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
        ]);
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_cardinality_hash_consistency() {
        use std::collections::hash_map::DefaultHasher;

        let c1 = Cardinality::new(vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ]);
        let c2 = Cardinality::new(vec![
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
        ]);

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        c1.hash(&mut h1);
        c2.hash(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn test_metrics_context_basic() {
        let ctx = MetricsContext::new();

        ctx.increment(
            "test-metric",
            Cardinality::new(vec![("label".to_string(), "label1".to_string())]),
            1,
        );
        ctx.increment(
            "test-metric",
            Cardinality::new(vec![("label".to_string(), "label1".to_string())]),
            1,
        );
        ctx.increment(
            "test-metric",
            Cardinality::new(vec![("label".to_string(), "label2".to_string())]),
            5,
        );

        let entries = ctx.get("test-metric");
        assert_eq!(entries.len(), 2);

        let label1 = entries
            .iter()
            .find(|e| e.cardinality.get("label") == Some(&"label1".to_string()))
            .unwrap();
        assert_eq!(label1.count, 2);

        let label2 = entries
            .iter()
            .find(|e| e.cardinality.get("label") == Some(&"label2".to_string()))
            .unwrap();
        assert_eq!(label2.count, 5);
    }

    #[test]
    fn test_metrics_with_cardinality() {
        let ctx = MetricsContext::new();

        // Track JSX prop usages with cardinality
        ctx.increment(
            "jsx-prop-usage",
            Cardinality::new(vec![
                ("propName".to_string(), "className".to_string()),
                ("propValue".to_string(), "container".to_string()),
            ]),
            3,
        );
        ctx.increment(
            "jsx-prop-usage",
            Cardinality::new(vec![
                ("propName".to_string(), "className".to_string()),
                ("propValue".to_string(), "header".to_string()),
            ]),
            2,
        );
        ctx.increment(
            "jsx-prop-usage",
            Cardinality::new(vec![
                ("propName".to_string(), "onClick".to_string()),
                ("propValue".to_string(), "handleClick".to_string()),
            ]),
            5,
        );

        let entries = ctx.get("jsx-prop-usage");
        assert_eq!(entries.len(), 3);

        // Find specific entries
        let container_entry = entries
            .iter()
            .find(|e| {
                e.cardinality.get("propName") == Some(&"className".to_string())
                    && e.cardinality.get("propValue") == Some(&"container".to_string())
            })
            .unwrap();
        assert_eq!(container_entry.count, 3);

        let onclick_entry = entries
            .iter()
            .find(|e| e.cardinality.get("propName") == Some(&"onClick".to_string()))
            .unwrap();
        assert_eq!(onclick_entry.count, 5);
    }

    #[test]
    fn test_get_all_metrics() {
        let ctx = MetricsContext::new();

        ctx.increment(
            "prop-usage",
            Cardinality::new(vec![("name".to_string(), "title".to_string())]),
            10,
        );
        ctx.increment(
            "prop-usage",
            Cardinality::new(vec![("name".to_string(), "placeholder".to_string())]),
            5,
        );
        ctx.increment(
            "component-count",
            Cardinality::new(vec![("component".to_string(), "Button".to_string())]),
            100,
        );

        let all = ctx.get_all();

        assert_eq!(all.len(), 2);
        assert!(all.contains_key("prop-usage"));
        assert!(all.contains_key("component-count"));
    }

    #[test]
    fn test_is_empty() {
        let ctx = MetricsContext::new();
        assert!(ctx.is_empty());

        ctx.increment(
            "test",
            Cardinality::new(vec![("k".to_string(), "v".to_string())]),
            1,
        );
        assert!(!ctx.is_empty());
    }

    #[test]
    fn test_clone_shares_data() {
        let ctx1 = MetricsContext::new();
        let ctx2 = ctx1.clone();

        ctx1.increment(
            "shared",
            Cardinality::new(vec![("label".to_string(), "test".to_string())]),
            10,
        );

        // Both should see the same data
        let entries1 = ctx1.get("shared");
        let entries2 = ctx2.get("shared");
        assert_eq!(entries1.len(), 1);
        assert_eq!(entries2.len(), 1);
        assert_eq!(entries1[0].count, 10);
        assert_eq!(entries2[0].count, 10);

        ctx2.increment(
            "shared",
            Cardinality::new(vec![("label".to_string(), "test".to_string())]),
            5,
        );
        assert_eq!(ctx1.get("shared")[0].count, 15);
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
                        ctx_clone.increment(
                            "concurrent-test",
                            Cardinality::new(vec![("thread".to_string(), format!("{}", i))]),
                            1,
                        );
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let all = ctx.get_all();
        let concurrent_test = all.get("concurrent-test").unwrap();

        let total: u64 = concurrent_test.iter().map(|e| e.count).sum();
        assert_eq!(total, 1000);
    }

    #[test]
    fn test_concurrent_same_cardinality() {
        use std::thread;

        let ctx = MetricsContext::new();

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let ctx_clone = ctx.clone();
                thread::spawn(move || {
                    for _ in 0..1000 {
                        ctx_clone.increment(
                            "concurrent-test",
                            Cardinality::new(vec![("label".to_string(), "same".to_string())]),
                            1,
                        );
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let entries = ctx.get("concurrent-test");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].count, 10000);
    }

    #[test]
    fn test_concurrent_multi_dimension_cardinality() {
        use std::thread;

        let ctx = MetricsContext::new();

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let ctx_clone = ctx.clone();
                thread::spawn(move || {
                    for j in 0..100 {
                        ctx_clone.increment(
                            "concurrent-cardinality",
                            Cardinality::new(vec![
                                ("thread".to_string(), format!("{}", i)),
                                ("iteration".to_string(), format!("{}", j % 10)),
                            ]),
                            1,
                        );
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let all = ctx.get_all();
        let entries = all.get("concurrent-cardinality").unwrap();

        // 10 threads * 10 unique iterations = 100 unique cardinalities
        assert_eq!(entries.len(), 100);

        let total: u64 = entries.iter().map(|e| e.count).sum();
        assert_eq!(total, 1000);
    }
}
