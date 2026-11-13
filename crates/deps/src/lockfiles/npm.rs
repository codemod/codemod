//! `package-lock.json` v2 and v3: the `packages` map keyed by install path.
//! Keys without `node_modules/` are workspaces (`""` is the root); a
//! dependency resolves the way Node does, from the nearest `node_modules`
//! up. Grepkon's parser read the same map but kept names only.

use std::collections::HashMap;

use serde_json::Value;

use super::npm_sections;
use crate::model::{Ecosystem, Graph, Kind};

pub fn parse(text: &str) -> Result<Graph, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let packages = doc
        .get("packages")
        .and_then(Value::as_object)
        .ok_or("only lockfileVersion 2 and 3 (with a `packages` map) are read")?;
    let mut graph = Graph::from_lockfile("package-lock.json");

    let mut by_path: HashMap<&str, usize> = HashMap::new();
    for (path, entry) in packages {
        if entry.get("link").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = match path.rsplit_once("node_modules/") {
            Some((_, name)) => {
                let version = entry
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                graph.node(Ecosystem::Npm, name, version)
            }
            None => {
                let dir = if path.is_empty() { "." } else { path.as_str() };
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("<{dir}>"), String::from);
                graph.workspace(Ecosystem::Npm, &name, "workspace", dir)
            }
        };
        by_path.insert(path, id);
    }
    // `node_modules/x` linked to a workspace points at that workspace's path.
    let links: HashMap<&str, &str> = packages
        .iter()
        .filter_map(|(path, entry)| Some((path.as_str(), entry.get("resolved")?.as_str()?)))
        .filter(|(path, _)| packages[*path].get("link").and_then(Value::as_bool) == Some(true))
        .collect();

    let resolve = |from: &str, name: &str| -> Option<usize> {
        let mut base = from.to_string();
        loop {
            let candidate = if base.is_empty() {
                format!("node_modules/{name}")
            } else {
                format!("{base}/node_modules/{name}")
            };
            if let Some(target) = links.get(candidate.as_str()) {
                return by_path.get(target).copied();
            }
            if let Some(&id) = by_path.get(candidate.as_str()) {
                return Some(id);
            }
            if base.is_empty() {
                return None;
            }
            base = match base.rfind("/node_modules/") {
                Some(at) => base[..at].to_string(),
                None if base.starts_with("node_modules/") => String::new(),
                // A workspace looks in its own node_modules, then the root's.
                None => String::new(),
            };
        }
    };

    for (path, entry) in packages {
        let Some(&from) = by_path.get(path.as_str()) else {
            continue;
        };
        for (name, range, kind) in npm_sections(entry) {
            let to = if kind == Kind::Peer {
                None
            } else {
                resolve(path, &name)
            };
            graph.edge(from, to, &name, Some(range), kind);
        }
    }
    Ok(graph)
}
