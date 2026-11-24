//! `package-lock.json` and `npm-shrinkwrap.json`. Versions 2 and 3 keep a
//! `packages` map keyed by install path: keys without `node_modules/` are
//! workspaces (`""` is the root); a dependency resolves the way Node does,
//! from the `node_modules` of each directory up to the root. Grepkon's
//! parser read the same map but kept names only. Version 1 keeps a tree of
//! `dependencies` instead, which is turned into that map; its root's ranges
//! come from package.json.

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::{npm_sections, split_name_version, ReadFile};
use crate::model::{Ecosystem, Graph, Kind};

pub fn parse(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let (packages, unknown_root_ranges) = match doc.get("packages").and_then(Value::as_object) {
        Some(packages) => (packages.clone(), false),
        None => v1_packages(&doc, read).ok_or(
            "neither a `packages` map (lockfileVersion 2 and 3) nor a `dependencies` tree (lockfileVersion 1)",
        )?,
    };
    let packages = &packages;
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
            let range = (!(unknown_root_ranges && path.is_empty())).then_some(range);
            graph.edge(from, to, &name, range, kind);
        }
    }
    Ok(graph)
}

/// A version 1 lockfile's `dependencies` tree as a version 2 `packages`
/// map, and whether the root's ranges are unknown (without a package.json,
/// the root declares what the tree's top level has, with no ranges).
fn v1_packages(doc: &Value, read: ReadFile) -> Option<(Map<String, Value>, bool)> {
    let tree = doc.get("dependencies")?.as_object()?;
    let mut packages = Map::new();
    add_v1(&mut packages, "", tree);
    let manifest = read("package.json").and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let (mut root, unknown) = match manifest {
        Some(Value::Object(manifest)) => (manifest, false),
        _ => {
            let mut root = Map::new();
            for (name, entry) in tree {
                let section = if entry.get("dev").and_then(Value::as_bool) == Some(true) {
                    "devDependencies"
                } else if entry.get("optional").and_then(Value::as_bool) == Some(true) {
                    "optionalDependencies"
                } else {
                    "dependencies"
                };
                root.entry(section)
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()?
                    .insert(name.clone(), Value::String(String::new()));
            }
            (root, true)
        }
    };
    for field in ["name", "version"] {
        if !root.contains_key(field) {
            if let Some(value) = doc.get(field) {
                root.insert(field.into(), value.clone());
            }
        }
    }
    packages.insert(String::new(), Value::Object(root));
    Some((packages, unknown))
}

fn add_v1(packages: &mut Map<String, Value>, parent: &str, tree: &Map<String, Value>) {
    for (folder, entry) in tree {
        let path = if parent.is_empty() {
            format!("node_modules/{folder}")
        } else {
            format!("{parent}/node_modules/{folder}")
        };
        let mut package = Map::new();
        let version = entry
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // An aliased install (`"foo": "npm:bar@1"`) records `npm:bar@1.2.0`.
        match version.strip_prefix("npm:").and_then(split_name_version) {
            Some((name, version)) => {
                package.insert("name".into(), name.into());
                package.insert("version".into(), version.into());
            }
            None => {
                package.insert("version".into(), version.into());
            }
        }
        if let Some(requires) = entry.get("requires").filter(|r| r.is_object()) {
            package.insert("dependencies".into(), requires.clone());
        }
        packages.insert(path.clone(), Value::Object(package));
        if let Some(nested) = entry.get("dependencies").and_then(Value::as_object) {
            add_v1(packages, &path, nested);
        }
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
