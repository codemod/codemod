//! `pnpm-lock.yaml`, v6 to v9. Importers are the workspaces; v9 keeps
//! resolved edges in `snapshots` (keys carry a `(peer)` suffix) and declared
//! ranges and peers in `packages`; v6 keeps both in `packages`.

use std::collections::HashMap;

use serde::Deserialize;
use serde_yaml::Value;

use super::{package_json, split_name_version, yaml_pairs, yaml_string, ReadFile};
use crate::model::{join, Ecosystem, Graph, Kind};

const SECTIONS: [(&str, Kind); 3] = [
    ("dependencies", Kind::Prod),
    ("devDependencies", Kind::Dev),
    ("optionalDependencies", Kind::Optional),
];

pub fn parse(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc = project_document(text)?;
    let mut graph = Graph::from_lockfile("pnpm-lock.yaml");
    let mapping = |field: &str| {
        doc.get(field)
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default()
    };
    let importers = mapping("importers");
    let snapshots = mapping("snapshots");
    let packages = mapping("packages");

    // Workspaces first, so `link:` versions resolve to them.
    let mut workspaces: HashMap<String, usize> = HashMap::new();
    for path in importers.keys().filter_map(yaml_string) {
        let name = read(&manifest(&path))
            .and_then(|text| package_json(&text).0)
            .unwrap_or_else(|| format!("<{path}>"));
        let id = graph.workspace(Ecosystem::Npm, &name, "workspace", &path);
        workspaces.insert(path, id);
    }
    for key in snapshots
        .keys()
        .chain(packages.keys())
        .filter_map(yaml_string)
    {
        if let Some((name, version)) = split_name_version(&key) {
            graph.node(Ecosystem::Npm, name, version);
        }
    }

    let target = |graph: &Graph, from_dir: &str, name: &str, version: &str| -> Option<usize> {
        if let Some(link) = version.strip_prefix("link:") {
            return workspaces.get(&join(from_dir, link)).copied();
        }
        let version = version.split('(').next().unwrap_or(version);
        // An alias resolves to "realname@version".
        match split_name_version(version) {
            Some((real, version)) if version.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                graph.find(Ecosystem::Npm, real, version)
            }
            _ => graph.find(Ecosystem::Npm, name, version),
        }
    };

    for (path, sections) in &importers {
        let path = yaml_string(path).unwrap_or_default();
        let from = workspaces[&path];
        for (field, kind) in SECTIONS {
            for (name, spec) in sections
                .get(field)
                .and_then(Value::as_mapping)
                .into_iter()
                .flatten()
            {
                let name = yaml_string(name).unwrap_or_default();
                let range = spec.get("specifier").and_then(yaml_string);
                let version = spec
                    .get("version")
                    .and_then(yaml_string)
                    .unwrap_or_default();
                let to = target(&graph, &path, &name, &version);
                graph.edge(from, to, &name, range, kind);
            }
        }
        // Importers do not record peer ranges; the workspace's package.json does.
        if let Some(text) = read(&manifest(&path)) {
            for (name, range, kind) in package_json(&text).1 {
                if kind == Kind::Peer {
                    graph.edge(from, None, &name, Some(range), Kind::Peer);
                }
            }
        }
    }

    // v9: resolved edges in snapshots, declared ranges in packages.
    // v6: `packages` holds resolved versions only, so the range stays unknown.
    let resolved_edges = if snapshots.is_empty() {
        &packages
    } else {
        &snapshots
    };
    for (key, entry) in resolved_edges {
        let key = yaml_string(key).unwrap_or_default();
        let Some((name, version)) = split_name_version(&key) else {
            continue;
        };
        let Some(from) = graph.find(Ecosystem::Npm, name, version) else {
            continue;
        };
        let declared: HashMap<String, String> = if snapshots.is_empty() {
            HashMap::new()
        } else {
            packages
                .get(Value::String(format!("{name}@{version}")))
                .map(|package| {
                    yaml_pairs(package.get("dependencies"))
                        .into_iter()
                        .chain(yaml_pairs(package.get("optionalDependencies")))
                        .collect()
                })
                .unwrap_or_default()
        };
        for (field, kind) in [
            ("dependencies", Kind::Prod),
            ("optionalDependencies", Kind::Optional),
        ] {
            for (dependency, version) in yaml_pairs(entry.get(field)) {
                let to = target(&graph, ".", &dependency, &version);
                let range = declared.get(&dependency).cloned();
                graph.edge(from, to, &dependency, range, kind);
            }
        }
    }
    for (key, entry) in &packages {
        let key = yaml_string(key).unwrap_or_default();
        let Some(from) =
            split_name_version(&key).and_then(|(n, v)| graph.find(Ecosystem::Npm, n, v))
        else {
            continue;
        };
        for (name, range) in yaml_pairs(entry.get("peerDependencies")) {
            graph.edge(from, None, &name, Some(range), Kind::Peer);
        }
    }
    Ok(graph)
}

fn manifest(dir: &str) -> String {
    join(dir, "package.json")
}

/// pnpm 10 can prepend a document for its own package-manager dependencies;
/// the project lock is the last document whose importers declare
/// dependencies (Grepkon's rule), or the last document.
fn project_document(text: &str) -> Result<Value, String> {
    let mut documents = Vec::new();
    for document in serde_yaml::Deserializer::from_str(text) {
        documents.push(Value::deserialize(document).map_err(|e| e.to_string())?);
    }
    let declares = |doc: &Value| {
        doc.get("importers")
            .and_then(Value::as_mapping)
            .is_some_and(|importers| {
                importers.values().any(|importer| {
                    SECTIONS
                        .iter()
                        .any(|(section, _)| importer.get(*section).is_some())
                })
            })
    };
    let index = documents
        .iter()
        .rposition(declares)
        .or_else(|| documents.len().checked_sub(1))
        .ok_or("the file is empty")?;
    Ok(documents.swap_remove(index))
}
