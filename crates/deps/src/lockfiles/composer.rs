//! `composer.lock`, with the `composer.json` beside it for the project's own
//! ranges. Package names are lowercased, as Composer compares them.
//!
//! - `packages` are installed for production and `packages-dev` for
//!   development; each keeps its `require` with constraints, which become
//!   prod edges. A locked package's `require-dev` is not installed.
//! - A package from a path repository (`dist.type` "path") is one of the
//!   repository's own packages, in that directory.
//! - The project itself is in `composer.json`: `require` gives prod edges and
//!   `require-dev` dev edges. Without a `name` it is `<.>`.
//! - Platform requirements (`php`, `ext-*`, `lib-*`, `composer-plugin-api`
//!   ...) name no package and are skipped.

use std::collections::HashMap;

use serde_json::Value;

use super::{ReadFile, UNVERSIONED};
use crate::model::{Ecosystem, Graph, Kind};

pub fn parse(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !doc.is_object() {
        return Err("not a composer.lock document".into());
    }
    let mut graph = Graph::from_lockfile("composer.lock");
    let packages: Vec<&Value> = ["packages", "packages-dev"]
        .iter()
        .filter_map(|section| doc.get(section)?.as_array())
        .flatten()
        .collect();
    let mut by_name = HashMap::new();
    let mut ids = Vec::new();
    for package in &packages {
        let name = str_of(package.get("name")).to_lowercase();
        let version = str_of(package.get("version"));
        let dist = package.get("dist");
        let id = match dist.and_then(|d| d.get("type")).and_then(Value::as_str) {
            Some("path") => {
                let dir = str_of(dist.and_then(|d| d.get("url")));
                graph.workspace(Ecosystem::Packagist, &name, version, dir)
            }
            _ => graph.node(Ecosystem::Packagist, &name, version),
        };
        by_name.insert(name, id);
        ids.push(id);
    }
    // A package that replaces or provides another (`psr/log-implementation`)
    // stands in for it when nothing of that name is installed.
    for (package, &id) in packages.iter().zip(&ids) {
        for field in ["replace", "provide"] {
            for (name, _) in requirements(package.get(field)) {
                by_name.entry(name).or_insert(id);
            }
        }
    }

    for (package, from) in packages.iter().zip(ids) {
        for (name, range) in requirements(package.get("require")) {
            let to = by_name.get(&name).copied();
            graph.edge(from, to, &name, Some(range), Kind::Prod);
        }
    }

    if let Some(manifest) =
        read("composer.json").and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        let name = manifest
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| "<.>".to_string(), str::to_lowercase);
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or(UNVERSIONED);
        let root = graph.workspace(Ecosystem::Packagist, &name, version, ".");
        for (field, kind) in [("require", Kind::Prod), ("require-dev", Kind::Dev)] {
            for (name, range) in requirements(manifest.get(field)) {
                let to = by_name.get(&name).copied();
                graph.edge(root, to, &name, Some(range), kind);
            }
        }
    }
    Ok(graph)
}

/// A `require`-shaped object's packages, lowercased, with their constraints.
/// Platform requirements are left out: every Composer package is
/// `vendor/name`, and no platform requirement has a `/`.
fn requirements(value: Option<&Value>) -> Vec<(String, String)> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(name, _)| name.contains('/'))
        .map(|(name, range)| {
            let range = range.as_str().unwrap_or("*").to_string();
            (name.to_lowercase(), range)
        })
        .collect()
}

fn str_of(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or_default()
}
