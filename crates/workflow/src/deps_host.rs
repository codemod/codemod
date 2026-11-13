//! Host side of `registry` and `semver`: the `semver` host function of the
//! workflow body, and `globalThis.__dependencyHost` in isolates, where
//! `js/dependencies.js` (`codemod:workflow/dependencies`) runs.

use std::collections::BTreeMap;
use std::sync::Arc;

use codemod_deps::registry::describe;
use codemod_deps::{semver as versions, Ecosystem, Registry};
use rquickjs::prelude::Async;
use rquickjs::{Ctx, Function, Object};

/// Imported by steps and callbacks that use `registry` or `semver`; the
/// build points their imports here.
pub const DEPENDENCIES_MODULE: &str = "codemod:workflow/dependencies";
pub(crate) const DEPENDENCIES_JS: &str = include_str!("js/dependencies.js");

pub(crate) fn ecosystem(name: &str) -> Result<Ecosystem, String> {
    Ecosystem::parse(name).ok_or_else(|| format!("unknown ecosystem '{name}'; use npm or cargo"))
}

/// Each named package's releases, as the JSON `registry.npm(name)` resolves to.
pub(crate) fn releases(
    registry: Option<&dyn Registry>,
    ecosystem_name: &str,
    names: &[String],
) -> Result<BTreeMap<String, serde_json::Value>, String> {
    let ecosystem = ecosystem(ecosystem_name)?;
    let registry = registry.ok_or("no package registry is available to this run")?;
    let found = registry.releases(ecosystem, names)?;
    Ok(names
        .iter()
        .map(|name| {
            let releases = found.get(name).map(Vec::as_slice).unwrap_or_default();
            (name.clone(), describe(ecosystem, name, releases))
        })
        .collect())
}

/// `semver.*` operations; returns JSON.
pub(crate) fn semver(
    op: String,
    ecosystem_name: String,
    a: String,
    b: String,
) -> rquickjs::Result<String> {
    let ecosystem = ecosystem(&ecosystem_name)
        .map_err(|e| rquickjs::Error::new_from_js_message("string", "ecosystem", e))?;
    let value = match op.as_str() {
        "satisfies" => serde_json::json!(versions::satisfies(ecosystem, &b, &a)),
        "compare" => serde_json::json!(versions::compare(ecosystem, &a, &b) as i8),
        "breaking" => serde_json::json!(versions::is_breaking(&a, &b)),
        "prerelease" => serde_json::json!(versions::is_prerelease(ecosystem, &a)),
        _ => serde_json::Value::Null,
    };
    Ok(value.to_string())
}

/// Install `__dependencyHost` in an isolate:
///   releases(ecosystem, namesJson): Promise<string>  one batch of lookups:
///                                    `{ ok, found: { name: releases } }` or `{ ok: false, error }`
///   semver(op, ecosystem, a, b): string
pub(crate) fn install(ctx: &Ctx<'_>, registry: Option<Arc<dyn Registry>>) -> rquickjs::Result<()> {
    let host = Object::new(ctx.clone())?;
    host.set(
        "releases",
        Function::new(
            ctx.clone(),
            Async(move |ecosystem: String, names: String| {
                let registry = registry.clone();
                async move {
                    let lookup = tokio::task::spawn_blocking(move || {
                        let names: Vec<String> = serde_json::from_str(&names)
                            .map_err(|e| format!("bad registry lookup: {e}"))?;
                        releases(registry.as_deref(), &ecosystem, &names)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("the registry lookup stopped: {e}")));
                    let answer = match lookup {
                        Ok(found) => serde_json::json!({ "ok": true, "found": found }),
                        Err(error) => serde_json::json!({ "ok": false, "error": error }),
                    };
                    rquickjs::Result::Ok(answer.to_string())
                }
            }),
        )?,
    )?;
    host.set("semver", Function::new(ctx.clone(), semver)?)?;
    ctx.globals().set("__dependencyHost", host)
}
