use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, Ctx, Object, Result};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

#[derive(Clone, Eq, Hash, PartialEq)]
struct Metric {
    name: String,
    value: usize,
}

#[allow(dead_code)]
pub(crate) struct MetricModule;

static METRIC_SCOPES: LazyLock<Arc<Mutex<HashSet<Metric>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashSet::new())));

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
    scopes
        .iter()
        .find(|m| m.name == scope_name)
        .map(|m| m.value)
        .unwrap_or(0)
}

fn set_metric(scope_name: String, value: usize) {
    let mut scopes = METRIC_SCOPES.lock().unwrap();
    if let Some(s) = scopes.iter().find(|m| m.name == scope_name).cloned() {
        scopes.remove(&s);
        let updated = Metric {
            name: scope_name,
            value,
        };
        scopes.insert(updated);
    } else {
        scopes.insert(Metric {
            name: scope_name,
            value,
        });
    }
}

fn use_metric(ctx: Ctx<'_>, scope_name: String) -> Result<Object<'_>> {
    let scopes = METRIC_SCOPES.lock().unwrap();
    if !scopes.iter().any(|s| s.name == scope_name) {
        drop(scopes);
        set_metric(scope_name.clone(), 0);
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
    scopes.iter().map(|m| (m.name.clone(), m.value)).collect()
}
