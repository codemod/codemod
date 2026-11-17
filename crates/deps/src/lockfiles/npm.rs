//! `package-lock.json` and `npm-shrinkwrap.json`. v2 and v3 have the
//! `packages` map keyed by install path. Keys without `node_modules/` are
//! workspaces (`""` is the root); a dependency resolves the way Node does,
//! from the `node_modules` of each directory up to the root. Grepkon's
//! parser read the same map but kept names only. v1 nests the same tree
//! under `dependencies` and is turned into that map first.

use std::collections::HashMap;

use serde_json::Value;

use serde_json::Map;

use super::{npm_sections, split_name_version, ReadFile};
use crate::model::{Ecosystem, Graph, Kind};

pub fn parse(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let v1;
    let packages = match doc.get("packages").and_then(Value::as_object) {
        Some(packages) => packages,
        None => {
            v1 = v1_packages(&doc, read);
            &v1
        }
    };
    let mut graph = Graph::from_lockfile("package-lock.json");

    let mut by_path: HashMap<&str, usize> = HashMap::new();
    for (path, entry) in packages {
        if entry.get("link").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = match path.rsplit_once("node_modules/") {
            Some((_, folder)) => {
                // An aliased install (`"foo": "npm:bar@1"`) is in the
                // folder `foo` but is the package `bar`.
                let name = entry.get("name").and_then(Value::as_str).unwrap_or(folder);
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
                // package-lock.json copies each workspace's package.json version.
                let version = entry
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or(super::UNVERSIONED);
                graph.workspace(Ecosystem::Npm, &name, version, dir)
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
            base = parent_dir(&base);
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

/// lockfileVersion 1 as a v2 `packages` map. Each package lists its ranges
/// in `requires` without saying which are optional, so they are all
/// `dependencies`; the root's ranges and kinds come from package.json.
fn v1_packages(doc: &Value, read: ReadFile) -> Map<String, Value> {
    let root = read("package.json")
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| {
            let field = |key: &str| doc.get(key).cloned().unwrap_or_default();
            serde_json::json!({ "name": field("name"), "version": field("version") })
        });
    let mut packages = Map::from_iter([(String::new(), root)]);
    nest(&mut packages, "", doc);
    packages
}

fn nest(packages: &mut Map<String, Value>, parent: &str, entry: &Value) {
    for (folder, child) in entry
        .get("dependencies")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let path = if parent.is_empty() {
            format!("node_modules/{folder}")
        } else {
            format!("{parent}/node_modules/{folder}")
        };
        let version = child
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut package = Map::new();
        package.insert(
            "dependencies".into(),
            child.get("requires").cloned().unwrap_or_default(),
        );
        if let Some(dir) = version.strip_prefix("file:") {
            // A local directory, linked like a v2 workspace.
            let link = serde_json::json!({ "resolved": dir, "link": true });
            packages.insert(path.clone(), link);
            package.insert("name".into(), folder.as_str().into());
            packages.insert(dir.to_string(), package.into());
        } else {
            // An alias (`npm:bar@1.2.3`) installs `bar` in the folder `foo`.
            let (name, version) = version
                .strip_prefix("npm:")
                .and_then(split_name_version)
                .unwrap_or((folder, version));
            package.insert("name".into(), name.into());
            package.insert("version".into(), version.into());
            packages.insert(path.clone(), package.into());
        }
        nest(packages, &path, child);
    }
}

/// The next directory Node looks in from `dir`: its parent, skipping
/// `node_modules` directories themselves.
fn parent_dir(dir: &str) -> String {
    let mut dir = dir;
    loop {
        dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
        if !(dir == "node_modules" || dir.ends_with("/node_modules")) {
            return dir.to_string();
        }
    }
}
