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

fn get_metric(scope_name: String) -> f64 {
    let scopes = METRIC_SCOPES.lock().unwrap();
    scopes
        .iter()
        .find(|m| m.name == scope_name)
        .map(|m| m.value as f64)
        .unwrap_or(0.0)
}

fn set_metric(scope_name: String, value: f64) {
    let mut scopes = METRIC_SCOPES.lock().unwrap();
    if let Some(s) = scopes.iter().find(|m| m.name == scope_name).cloned() {
        scopes.remove(&s);
        let mut updated = s;
        updated.value = value as usize;
        scopes.insert(updated);
    } else {
        scopes.insert(Metric {
            name: scope_name,
            value: value as usize,
        });
    }
}

fn use_metric(ctx: Ctx<'_>, scope_name: String, initial: Option<f64>) -> Result<Object<'_>> {
    if let Some(v) = initial {
        set_metric(scope_name.clone(), v);
    }

    let obj = Object::new(ctx.clone())?;
    let scope_name_clone = scope_name.clone();
    obj.set(
        "get",
        Func::new(move |_ctx: Ctx<'_>, ()| get_metric(scope_name_clone.clone())),
    )?;
    let scope_name_clone2 = scope_name.clone();
    obj.set(
        "set",
        Func::new(move |_ctx: Ctx<'_>, value: f64| set_metric(scope_name_clone2.clone(), value)),
    )?;
    Ok(obj)
}
