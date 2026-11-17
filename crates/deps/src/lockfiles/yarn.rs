//! `yarn.lock`. Yarn Berry (v2+) writes YAML; Yarn 1 writes its own
//! indented format, headed `# yarn lockfile v1`. Each entry lists the
//! descriptors it satisfies, its version and its dependencies. Berry also
//! lists the repository's workspaces (entries resolved `@workspace:`) and
//! peer ranges; Yarn 1 lists neither, so its workspaces are the directories
//! the root package.json's `workspaces` globs name ([`workspaces`]).

use std::collections::HashMap;
use std::path::Path;

use serde_yaml::Value;

use super::{
    package_json, package_json_version, versions_by_name, yaml_pairs, yaml_string, ReadFile,
};
use crate::model::{join, Ecosystem, Graph, Kind};
use crate::semver::satisfies;

const V1_HEADER: &str = "# yarn lockfile v1";

/// One lock entry, in either format.
struct Entry {
    /// `name@npm:range` for registry ranges, `name@protocol:...` otherwise.
    descriptors: Vec<String>,
    name: String,
    version: String,
    workspace: Option<String>,
    /// Name, range as written, and kind (prod, optional or peer).
    dependencies: Vec<(String, String, Kind)>,
}

pub fn is_v1(text: &str) -> bool {
    text.contains(V1_HEADER)
}

/// `workspaces` are the workspace directories for a Yarn 1 lockfile, which
/// does not list them; Berry lists its own and ignores them.
pub fn parse(text: &str, read: ReadFile, workspaces: &[String]) -> Result<Graph, String> {
    let entries = if is_v1(text) {
        v1_entries(text, read, workspaces)?
    } else {
        berry_entries(text)?
    };
    let mut graph = Graph::from_lockfile("yarn.lock");
    let mut by_descriptor: HashMap<String, usize> = HashMap::new();
    let mut ids = Vec::new();
    for entry in &entries {
        let id = match &entry.workspace {
            Some(location) => {
                graph.workspace(Ecosystem::Npm, &entry.name, &entry.version, location)
            }
            None => graph.node(Ecosystem::Npm, &entry.name, &entry.version),
        };
        for descriptor in &entry.descriptors {
            by_descriptor.insert(descriptor.clone(), id);
        }
        ids.push(id);
    }

    let by_name = versions_by_name(&graph);
    let resolve = |graph: &Graph, name: &str, range: &str| -> Option<usize> {
        by_descriptor
            .get(&descriptor(name, range))
            .copied()
            .or_else(|| {
                // `resolutions` rewrite a descriptor (often to `patch:`), and
                // Yarn 1 keeps no entries for workspaces, so the declared range
                // has no lock key: take the installed version that satisfies
                // it, or the only one.
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

    for (entry, id) in entries.into_iter().zip(ids) {
        if let Some(location) = &entry.workspace {
            // The lockfile merges a workspace's dependencies and
            // devDependencies; its package.json says which is which.
            if let Some(text) = read(&join(location, "package.json")) {
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
        for (name, range, kind) in entry.dependencies {
            let to = if kind == Kind::Peer {
                None
            } else {
                resolve(&graph, &name, &range)
            };
            graph.edge(id, to, &name, Some(without_npm(&range)), kind);
        }
    }
    Ok(graph)
}

fn berry_entries(text: &str) -> Result<Vec<Entry>, String> {
    let doc: Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    let mapping = doc.as_mapping().ok_or("not a YAML mapping")?;
    let mut entries = Vec::new();
    for (key, entry) in mapping {
        let key = yaml_string(key).unwrap_or_default();
        let Some(resolution) = entry.get("resolution").and_then(yaml_string) else {
            continue;
        };
        let optional: Vec<String> = entry
            .get("dependenciesMeta")
            .and_then(Value::as_mapping)
            .into_iter()
            .flatten()
            .filter(|(_, meta)| meta.get("optional").and_then(Value::as_bool) == Some(true))
            .filter_map(|(name, _)| yaml_string(name))
            .collect();
        let mut dependencies: Vec<(String, String, Kind)> = yaml_pairs(entry.get("dependencies"))
            .into_iter()
            .map(|(name, range)| {
                let kind = if optional.contains(&name) {
                    Kind::Optional
                } else {
                    Kind::Prod
                };
                (name, range, kind)
            })
            .collect();
        dependencies.extend(
            yaml_pairs(entry.get("peerDependencies"))
                .into_iter()
                .map(|(name, range)| (name, range, Kind::Peer)),
        );
        entries.push(Entry {
            descriptors: key
                .split(", ")
                .map(|descriptor| descriptor.trim_matches('"').to_string())
                .collect(),
            name: name_of(&resolution).to_string(),
            version: entry
                .get("version")
                .and_then(yaml_string)
                .unwrap_or_default(),
            workspace: resolution.split("@workspace:").nth(1).map(String::from),
            dependencies,
        });
    }
    Ok(entries)
}

/// Yarn 1: `descriptor, descriptor:` at the margin, then indented
/// `version "1.2.3"` and `dependencies:` / `optionalDependencies:` blocks of
/// `name "range"`.
fn v1_entries(text: &str, read: ReadFile, workspaces: &[String]) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = Vec::new();
    for dir in std::iter::once(".").chain(workspaces.iter().map(String::as_str)) {
        let manifest = read(&join(dir, "package.json"));
        entries.push(Entry {
            descriptors: Vec::new(),
            name: manifest
                .as_deref()
                .and_then(|text| package_json(text).0)
                .unwrap_or_else(|| format!("<{dir}>")),
            version: manifest
                .as_deref()
                .and_then(package_json_version)
                .unwrap_or_else(|| super::UNVERSIONED.to_string()),
            workspace: Some(dir.to_string()),
            dependencies: Vec::new(),
        });
    }
    let first_package = entries.len();
    let mut section = None;
    for (number, line) in text.lines().enumerate() {
        let content = line.trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let error = || format!("line {}: cannot read `{content}`", number + 1);
        match line.len() - line.trim_start().len() {
            0 => {
                let key = content.strip_suffix(':').ok_or_else(error)?;
                let descriptors: Vec<(String, String)> = tokens(key)
                    .iter()
                    .filter_map(|descriptor| {
                        let at = descriptor.get(1..)?.find('@')? + 1;
                        Some((
                            descriptor[..at].to_string(),
                            descriptor[at + 1..].to_string(),
                        ))
                    })
                    .collect();
                let (name, range) = descriptors.first().ok_or_else(error)?;
                // An alias `foo@npm:bar@^1` installs `bar`.
                let name = range
                    .strip_prefix("npm:")
                    .map_or(name.as_str(), name_of)
                    .to_string();
                entries.push(Entry {
                    descriptors: descriptors
                        .iter()
                        .map(|(name, range)| descriptor(name, range))
                        .collect(),
                    name,
                    version: String::new(),
                    workspace: None,
                    dependencies: Vec::new(),
                });
                section = None;
            }
            2 if entries.len() > first_package => {
                let entry = entries.last_mut().ok_or_else(error)?;
                section = match content.strip_suffix(':') {
                    Some("dependencies") => Some(Kind::Prod),
                    Some("optionalDependencies") => Some(Kind::Optional),
                    Some(_) => None,
                    None => {
                        if let [field, value] = tokens(content).as_slice() {
                            if field == "version" {
                                entry.version = value.clone();
                            }
                        }
                        None
                    }
                };
            }
            _ if entries.len() > first_package => {
                if let (Some(kind), [name, range]) = (section, tokens(content).as_slice()) {
                    let entry = entries.last_mut().ok_or_else(error)?;
                    entry.dependencies.push((name.clone(), range.clone(), kind));
                }
            }
            _ => return Err(error()),
        }
    }
    Ok(entries)
}

/// Space- or comma-separated words, each optionally in double quotes.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let (token, after) = match rest.strip_prefix('"') {
            Some(quoted) => {
                let end = quoted.find('"').unwrap_or(quoted.len());
                (&quoted[..end], quoted.get(end + 1..).unwrap_or_default())
            }
            None => {
                let end = rest.find([' ', ',']).unwrap_or(rest.len());
                (&rest[..end], &rest[end..])
            }
        };
        out.push(token.to_string());
        rest = after.trim_start_matches([' ', ',']);
    }
    out
}

/// The lock key a dependency on `name` with `range` is listed under.
fn descriptor(name: &str, range: &str) -> String {
    if range.contains(':') {
        format!("{name}@{range}")
    } else {
        format!("{name}@npm:{range}")
    }
}

/// The directories, relative to `dir`, of the workspaces its package.json
/// names in `workspaces` (a list of globs, or `{ "packages": [...] }`; a
/// glob starting with `!` excludes). A `*` stays within one directory and
/// `**` spans any number.
pub fn workspaces(dir: &Path) -> Vec<String> {
    let Some(text) = std::fs::read_to_string(dir.join("package.json")).ok() else {
        return Vec::new();
    };
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let globs: Vec<&str> = json
        .get("workspaces")
        .map(|field| field.get("packages").unwrap_or(field))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(|glob| glob.trim_start_matches("./").trim_end_matches('/'))
        .collect();
    if globs.is_empty() {
        return Vec::new();
    }
    let matches = |glob: &str, relative: &str| {
        let glob: Vec<&str> = glob.split('/').collect();
        let path: Vec<&str> = relative.split('/').collect();
        segments_match(&glob, &path)
    };
    let mut dirs: Vec<String> = ignore::WalkBuilder::new(dir)
        .filter_entry(|entry| entry.file_name() != "node_modules")
        .build()
        .flatten()
        .filter(|entry| entry.file_name() == "package.json")
        .filter_map(|entry| {
            let relative = entry.path().parent()?.strip_prefix(dir).ok()?;
            Some(relative.to_string_lossy().replace('\\', "/"))
        })
        .filter(|relative| !relative.is_empty())
        .filter(|relative| {
            globs.iter().any(|glob| matches(glob, relative))
                && !globs.iter().any(|glob| {
                    glob.strip_prefix('!')
                        .is_some_and(|glob| matches(glob, relative))
                })
        })
        .collect();
    dirs.sort();
    dirs
}

fn segments_match(glob: &[&str], path: &[&str]) -> bool {
    match glob.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => path.split_first().is_some_and(|(segment, path)| {
            super::star_match(first, segment) && segments_match(rest, path)
        }),
    }
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
