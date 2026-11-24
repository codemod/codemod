//! Yarn 1 (classic) `yarn.lock`: an indented format of its own. Each entry
//! lists the descriptors (`name@range`) it satisfies, its version and its
//! dependencies with their ranges. Unlike Yarn 2+, the lockfile does not
//! list the repository's workspaces: they come from the root package.json's
//! `workspaces` globs, which [`super::load`] expands, and each workspace's
//! package.json gives its dependencies and their kinds.

use std::collections::HashMap;

use super::{package_json, package_json_version, versions_by_name, ReadFile};
use crate::model::{join, Ecosystem, Graph, Kind};
use crate::semver::satisfies;

/// Whether `text` is a Yarn 1 lockfile.
pub(crate) fn is_classic(text: &str) -> bool {
    text.contains("# yarn lockfile v1")
}

#[derive(Default)]
struct Entry {
    descriptors: Vec<String>,
    version: String,
    dependencies: Vec<(String, String, Kind)>,
}

/// `workspaces` are the workspace directories other than the root, relative
/// to the lockfile's directory.
pub fn parse(text: &str, read: ReadFile, workspaces: &[String]) -> Result<Graph, String> {
    let entries = entries(text)?;
    let mut graph = Graph::from_lockfile("yarn.lock");

    let mut by_descriptor: HashMap<String, usize> = HashMap::new();
    let mut ids = Vec::with_capacity(entries.len());
    for entry in &entries {
        let Some((name, range)) = entry.descriptors.first().and_then(|d| split_descriptor(d))
        else {
            ids.push(None);
            continue;
        };
        // `alias@npm:real@^1` installs `real`.
        let name = range
            .strip_prefix("npm:")
            .and_then(split_descriptor)
            .map_or(name, |(real, _)| real);
        let id = graph.node(Ecosystem::Npm, name, &entry.version);
        for descriptor in &entry.descriptors {
            by_descriptor.insert(descriptor.clone(), id);
        }
        ids.push(Some(id));
    }

    let mut members: Vec<(usize, Option<String>)> = Vec::new();
    let mut by_workspace_name: HashMap<String, usize> = HashMap::new();
    for dir in std::iter::once(".").chain(workspaces.iter().map(String::as_str)) {
        let manifest = read(&join(dir, "package.json"));
        let (name, _) = manifest.as_deref().map(package_json).unwrap_or_default();
        let version = manifest
            .as_deref()
            .and_then(package_json_version)
            .unwrap_or_else(|| super::UNVERSIONED.to_string());
        let label = name.clone().unwrap_or_else(|| format!("<{dir}>"));
        let id = graph.workspace(Ecosystem::Npm, &label, &version, dir);
        if let Some(name) = name {
            by_workspace_name.entry(name).or_insert(id);
        }
        members.push((id, manifest));
    }

    let by_name = versions_by_name(&graph);
    let resolve = |graph: &Graph, name: &str, range: &str| -> Option<usize> {
        by_descriptor
            .get(&format!("{name}@{range}"))
            .copied()
            // A workspace dependency is linked, not locked.
            .or_else(|| by_workspace_name.get(name).copied())
            .or_else(|| {
                // `resolutions` rewrite a descriptor, so the declared range
                // has no lock entry: take the installed version that
                // satisfies it, or the only one.
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

    for (from, manifest) in members {
        let Some(text) = manifest else { continue };
        for (name, range, kind) in package_json(&text).1 {
            let to = if kind == Kind::Peer {
                None
            } else {
                resolve(&graph, &name, &range)
            };
            graph.edge(from, to, &name, Some(range), kind);
        }
    }
    for (entry, id) in entries.iter().zip(ids) {
        let Some(from) = id else { continue };
        for (name, range, kind) in &entry.dependencies {
            let to = resolve(&graph, name, range);
            graph.edge(from, to, name, Some(range.clone()), *kind);
        }
    }
    Ok(graph)
}

fn entries(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut section: Option<Kind> = None;
    for (number, line) in text.lines().enumerate() {
        let content = line.trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            let keys = content
                .strip_suffix(':')
                .ok_or_else(|| format!("line {}: expected an entry", number + 1))?;
            entries.push(Entry {
                descriptors: keys
                    .split(", ")
                    .map(|key| unquote(key.trim()).to_string())
                    .collect(),
                ..Entry::default()
            });
            section = None;
            continue;
        }
        let Some(entry) = entries.last_mut() else {
            return Err(format!("line {}: a field outside an entry", number + 1));
        };
        if indent <= 2 {
            section = match content.strip_suffix(':') {
                Some("dependencies") => Some(Kind::Prod),
                Some("optionalDependencies") => Some(Kind::Optional),
                _ => None,
            };
            if let ("version", version) = key_value(content) {
                entry.version = version.to_string();
            }
        } else if let Some(kind) = section {
            let (name, range) = key_value(content);
            entry
                .dependencies
                .push((name.to_string(), range.to_string(), kind));
        }
    }
    Ok(entries)
}

/// `key value`, either of which may be quoted.
fn key_value(line: &str) -> (&str, &str) {
    let (key, rest) = match line.strip_prefix('"') {
        Some(quoted) => match quoted.find('"') {
            Some(end) => (&quoted[..end], &quoted[end + 1..]),
            None => (quoted, ""),
        },
        None => line.split_once(' ').unwrap_or((line, "")),
    };
    (key, unquote(rest.trim()))
}

fn unquote(text: &str) -> &str {
    text.strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(text)
}

/// `@scope/name@^1` into name and range.
fn split_descriptor(descriptor: &str) -> Option<(&str, &str)> {
    let at = descriptor.get(1..)?.find('@')? + 1;
    Some((&descriptor[..at], &descriptor[at + 1..]))
}
