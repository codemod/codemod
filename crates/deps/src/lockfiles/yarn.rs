//! Yarn Berry (v2+) `yarn.lock`, which is YAML. Each entry lists the
//! descriptors it satisfies, its resolution, its dependencies and its peer
//! ranges; the repository's workspaces are entries resolved `@workspace:`.

use std::collections::HashMap;

use serde_yaml::Value;

use super::{package_json, versions_by_name, yaml_pairs, yaml_string, ReadFile};
use crate::model::{Ecosystem, Graph, Kind};
use crate::semver::satisfies;

pub fn parse(text: &str, read: ReadFile) -> Result<Graph, String> {
    if text.contains("# yarn lockfile v1") {
        return Err("Yarn v1 lockfiles are not read yet".into());
    }
    let doc: Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    let entries = doc.as_mapping().ok_or("not a YAML mapping")?;
    let mut graph = Graph::from_lockfile("yarn.lock");
    let mut by_descriptor: HashMap<String, usize> = HashMap::new();
    let mut parsed = Vec::new();
    for (key, entry) in entries {
        let key = yaml_string(key).unwrap_or_default();
        let Some(resolution) = entry.get("resolution").and_then(yaml_string) else {
            continue;
        };
        let name = name_of(&resolution);
        let version = entry
            .get("version")
            .and_then(yaml_string)
            .unwrap_or_default();
        let id = match resolution.split("@workspace:").nth(1) {
            Some(location) => graph.workspace(Ecosystem::Npm, name, &version, location),
            None => graph.node(Ecosystem::Npm, name, &version),
        };
        for descriptor in key.split(", ") {
            by_descriptor.insert(descriptor.trim_matches('"').to_string(), id);
        }
        parsed.push((id, entry));
    }

    let by_name = versions_by_name(&graph);
    let resolve = |graph: &Graph, name: &str, range: &str| -> Option<usize> {
        let descriptor = if range.contains(':') {
            format!("{name}@{range}")
        } else {
            format!("{name}@npm:{range}")
        };
        by_descriptor.get(&descriptor).copied().or_else(|| {
            // `resolutions` rewrite a descriptor (often to `patch:`), so the
            // declared range has no lock key: take the installed version
            // that satisfies it, or the only one.
            let candidates = by_name.get(name)?;
            candidates
                .iter()
                .find(|&&id| {
                    satisfies(Ecosystem::Npm, range, &graph.nodes[id].version) == Some(true)
                })
                .or((candidates.len() == 1).then(|| &candidates[0]))
                .copied()
        })
    };

    for (id, entry) in parsed {
        if let Some(location) = graph.nodes[id].workspace.clone() {
            // The lockfile merges a workspace's dependencies and
            // devDependencies; its package.json says which is which.
            let manifest = if location == "." {
                "package.json".to_string()
            } else {
                format!("{location}/package.json")
            };
            if let Some(text) = read(&manifest) {
                for (name, range, kind) in package_json(&text).1 {
                    let to = if kind == Kind::Peer {
                        None
                    } else {
                        resolve(&graph, &name, &range)
                    };
                    graph.edge(id, to, &name, Some(range), kind);
                }
                continue;
            }
        }
        let optional: Vec<String> = entry
            .get("dependenciesMeta")
            .and_then(Value::as_mapping)
            .into_iter()
            .flatten()
            .filter(|(_, meta)| meta.get("optional").and_then(Value::as_bool) == Some(true))
            .filter_map(|(name, _)| yaml_string(name))
            .collect();
        for (name, range) in yaml_pairs(entry.get("dependencies")) {
            let kind = if optional.contains(&name) {
                Kind::Optional
            } else {
                Kind::Prod
            };
            let to = resolve(&graph, &name, &range);
            graph.edge(id, to, &name, Some(without_npm(&range)), kind);
        }
        for (name, range) in yaml_pairs(entry.get("peerDependencies")) {
            graph.edge(id, None, &name, Some(without_npm(&range)), Kind::Peer);
        }
    }
    Ok(graph)
}

/// `@scope/name@npm:1.2.3` to `@scope/name`.
fn name_of(resolution: &str) -> &str {
    match resolution.get(1..).and_then(|rest| rest.find('@')) {
        Some(at) => &resolution[..at + 1],
        None => resolution,
    }
}

fn without_npm(range: &str) -> String {
    range.strip_prefix("npm:").unwrap_or(range).to_string()
}
