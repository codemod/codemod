use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, Ctx, Object, Result};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

pub mod types;

#[cfg(feature = "native")]
pub mod native;

#[cfg(feature = "wasm")]
pub mod wasm;

#[allow(dead_code)]
pub(crate) struct MetricModule;

#[derive(Eq, Hash, PartialEq)]
struct Metric {
    name: String,
    value: usize,
}

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

        exports.export("useMetric", Func::from(useMetric))?;
        exports.export("default", default)?;
        Ok(())
    }
}

fn getMetric(scope_name: String) {}

fn setMetric(ctx: Ctx<'_>, value: f64) {}

fn useMetric(ctx: Ctx<'_>, scope_name: String, value: Option<f64>) -> Result<Object<'_>> {
    let mut scopes = METRIC_SCOPES.lock().unwrap();
    if !scopes.iter().any(|m| m.name == scope_name) {
        scopes.insert(Metric {
            name: scope_name,
            value: value.unwrap_or(0.0) as usize,
        });
    }
    drop(scopes);

    let obj = Object::new(ctx.clone())?;
    obj.set("get", Func::from(getMetric))?;
    obj.set("set", Func::from(setMetric))?;
    Ok(obj)
}
