//! Python lockfiles: `uv.lock`, `poetry.lock` and `Pipfile.lock`. Package
//! names are normalized as PyPI does (PEP 503), so `Django`, `django` and
//! `typing_extensions` / `typing-extensions` are one package each.
//!
//! - uv.lock keeps every package with its dependencies, and the ranges of
//!   the repository's own packages in their `metadata`.
//! - poetry.lock keeps every package with its dependencies and ranges; the
//!   project itself is in `pyproject.toml`.
//! - Pipfile.lock keeps installed versions only, with no edges between
//!   packages; the project's own ranges are in `Pipfile`. Packages the
//!   Pipfile does not name are in the graph but reached by no chain.

use std::collections::HashMap;

use serde_json::Value as Json;
use toml::Value;

use super::{ReadFile, UNVERSIONED};
use crate::model::{Ecosystem, Graph, Kind};
use crate::semver::satisfies;

/// What a dependency without a range accepts.
const ANY: &str = "*";

pub fn uv(text: &str) -> Result<Graph, String> {
    let doc = document(text)?;
    let packages = array(doc.get("package"));
    let mut graph = Graph::from_lockfile("uv.lock");
    let mut by_name = HashMap::<String, Vec<usize>>::new();
    let mut ids = Vec::new();
    for package in &packages {
        let name = normalize(str_of(package.get("name")));
        let version = package.get("version").and_then(Value::as_str);
        let local = package.get("source").and_then(|source| {
            ["editable", "virtual", "directory"]
                .iter()
                .find_map(|key| source.get(key)?.as_str())
        });
        let id = match local {
            Some(dir) => {
                graph.workspace(Ecosystem::Pypi, &name, version.unwrap_or(UNVERSIONED), dir)
            }
            None => graph.node(Ecosystem::Pypi, &name, version.unwrap_or_default()),
        };
        by_name.entry(name).or_default().push(id);
        ids.push(id);
    }

    for (package, from) in packages.iter().zip(ids) {
        let metadata = package.get("metadata");
        let mut sections = vec![(package.get("dependencies").cloned(), Kind::Prod)];
        for (field, kind) in [
            ("optional-dependencies", Kind::Optional),
            ("dev-dependencies", Kind::Dev),
        ] {
            for group in table(package.get(field)).into_iter().map(|(_, v)| v) {
                sections.push((Some(group), kind));
            }
        }
        let mut ranges = HashMap::new();
        for requirement in array(metadata.and_then(|m| m.get("requires-dist"))) {
            let kind = match requirement.get("marker").and_then(Value::as_str) {
                Some(marker) if marker.contains("extra ==") => Kind::Optional,
                _ => Kind::Prod,
            };
            ranges.insert(
                (normalize(str_of(requirement.get("name"))), kind),
                specifier(&requirement),
            );
        }
        for group in table(metadata.and_then(|m| m.get("requires-dev")))
            .into_iter()
            .map(|(_, v)| v)
        {
            for requirement in array(Some(&group)) {
                ranges.insert(
                    (normalize(str_of(requirement.get("name"))), Kind::Dev),
                    specifier(&requirement),
                );
            }
        }

        let is_workspace = graph.nodes[from].workspace.is_some();
        for (section, kind) in sections {
            for dependency in array(section.as_ref()) {
                let name = normalize(str_of(dependency.get("name")));
                let version = dependency.get("version").and_then(Value::as_str);
                let to = resolve(&graph, &by_name, &name, version);
                // A third-party package's requirements are not in uv.lock.
                let range = is_workspace
                    .then(|| ranges.get(&(name.clone(), kind)).cloned())
                    .flatten();
                graph.edge(from, to, &name, range, kind);
            }
        }
    }
    Ok(graph)
}

pub fn poetry(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc = document(text)?;
    let packages = array(doc.get("package"));
    let mut graph = Graph::from_lockfile("poetry.lock");
    let mut by_name = HashMap::<String, Vec<usize>>::new();
    let mut ids = Vec::new();
    for package in &packages {
        let name = normalize(str_of(package.get("name")));
        let version = str_of(package.get("version"));
        let source = package.get("source");
        let id = match source.and_then(|s| s.get("type")).and_then(Value::as_str) {
            Some("directory") => {
                let dir = str_of(source.and_then(|s| s.get("url")));
                graph.workspace(Ecosystem::Pypi, &name, version, dir)
            }
            _ => graph.node(Ecosystem::Pypi, &name, version),
        };
        by_name.entry(name).or_default().push(id);
        ids.push(id);
    }

    for (package, from) in packages.iter().zip(ids) {
        for (name, value) in table(package.get("dependencies")) {
            let (range, optional) = poetry_constraints(value);
            let name = normalize(&name);
            let to = resolve_range(&graph, &by_name, &name, &range);
            let kind = if optional { Kind::Optional } else { Kind::Prod };
            graph.edge(from, to, &name, Some(range), kind);
        }
    }

    if let Some(manifest) = read("pyproject.toml").and_then(|text| document(&text).ok()) {
        let project = manifest.get("project");
        let poetry = manifest.get("tool").and_then(|t| t.get("poetry"));
        let field = |key: &str| {
            project
                .and_then(|p| p.get(key))
                .or_else(|| poetry?.get(key))
                .and_then(Value::as_str)
        };
        let name = field("name").map_or_else(|| "<.>".to_string(), normalize);
        let root = graph.workspace(
            Ecosystem::Pypi,
            &name,
            field("version").unwrap_or(UNVERSIONED),
            ".",
        );
        for (name, range, kind) in pyproject_dependencies(&manifest) {
            let to = resolve_range(&graph, &by_name, &name, &range);
            graph.edge(root, to, &name, Some(range), kind);
        }
    }
    Ok(graph)
}

pub fn pipfile(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc: Json = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut graph = Graph::from_lockfile("Pipfile.lock");
    let mut by_name = HashMap::<String, Vec<usize>>::new();
    for section in ["default", "develop"] {
        for (name, entry) in doc
            .get(section)
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
        {
            let name = normalize(name);
            let id = match entry.get("path").and_then(Json::as_str) {
                Some(dir) => graph.workspace(Ecosystem::Pypi, &name, UNVERSIONED, dir),
                None => {
                    let pinned = entry
                        .get("version")
                        .and_then(Json::as_str)
                        .unwrap_or_default();
                    graph.node(Ecosystem::Pypi, &name, pinned.trim_start_matches("=="))
                }
            };
            let ids = by_name.entry(name).or_default();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }

    if let Some(manifest) = read("Pipfile").and_then(|text| document(&text).ok()) {
        let root = graph.workspace(Ecosystem::Pypi, "<.>", UNVERSIONED, ".");
        for (field, kind) in [("packages", Kind::Prod), ("dev-packages", Kind::Dev)] {
            for (name, value) in table(manifest.get(field)) {
                let range = value
                    .as_str()
                    .or_else(|| value.get("version")?.as_str())
                    .unwrap_or(ANY)
                    .to_string();
                let name = normalize(&name);
                let to = resolve(&graph, &by_name, &name, None);
                graph.edge(root, to, &name, Some(range), kind);
            }
        }
    }
    Ok(graph)
}

/// A PEP 508 requirement (`requests[socks] >= 2.31 ; python_version < "3.13"`)
/// as its normalized name and version specifier.
pub(crate) fn requirement(text: &str) -> (String, String) {
    let text = text.split(';').next().unwrap_or_default().trim();
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(text.len());
    let rest = text[end..].trim_start();
    let rest = match rest.strip_prefix('[') {
        Some(extras) => extras.split_once(']').map_or("", |(_, rest)| rest),
        None => rest,
    };
    let range = rest
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let range = if range.is_empty() || range.starts_with('@') {
        ANY
    } else {
        range
    };
    (normalize(&text[..end]), range.to_string())
}

/// PEP 503: lowercase, with each run of `-`, `_` and `.` as one `-`.
pub(crate) fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// A pyproject.toml's dependencies, from PEP 621 `[project]`, PEP 735
/// `[dependency-groups]` and Poetry's own tables. Every group is `dev`.
fn pyproject_dependencies(manifest: &Value) -> Vec<(String, String, Kind)> {
    let mut out = Vec::new();
    let project = manifest.get("project");
    let mut listed = |list: Option<&Value>, kind: Kind| {
        for item in array(list) {
            if let Some(text) = item.as_str() {
                let (name, range) = requirement(text);
                out.push((name, range, kind));
            }
        }
    };
    listed(project.and_then(|p| p.get("dependencies")), Kind::Prod);
    for extra in table(project.and_then(|p| p.get("optional-dependencies")))
        .into_iter()
        .map(|(_, v)| v)
    {
        listed(Some(&extra), Kind::Optional);
    }
    for group in table(manifest.get("dependency-groups"))
        .into_iter()
        .map(|(_, v)| v)
    {
        listed(Some(&group), Kind::Dev);
    }

    let poetry = manifest.get("tool").and_then(|t| t.get("poetry"));
    let groups: Vec<Value> = table(poetry.and_then(|p| p.get("group")))
        .into_iter()
        .map(|(_, v)| v)
        .filter_map(|group| group.get("dependencies").cloned())
        .collect();
    let mut tables = vec![
        (poetry.and_then(|p| p.get("dependencies")), Kind::Prod),
        (poetry.and_then(|p| p.get("dev-dependencies")), Kind::Dev),
    ];
    tables.extend(groups.iter().map(|deps| (Some(deps), Kind::Dev)));
    for (deps, kind) in tables {
        for (name, value) in table(deps) {
            if name == "python" {
                continue;
            }
            let (range, optional) = poetry_constraints(value);
            let kind = if optional { Kind::Optional } else { kind };
            out.push((normalize(&name), range, kind));
        }
    }
    out
}

/// A Poetry dependency's range and whether it is optional. The value is one
/// constraint, or several split by environment markers: their ranges are
/// joined by `||`, and it is optional only when every one of them is.
fn poetry_constraints(value: Value) -> (String, bool) {
    let constraints = match value {
        Value::Array(list) => list,
        value => vec![value],
    };
    let optional = constraints
        .iter()
        .all(|c| c.get("optional").and_then(Value::as_bool) == Some(true));
    let range = constraints
        .iter()
        .map(|c| {
            c.as_str()
                .or_else(|| c.get("version")?.as_str())
                .unwrap_or(ANY)
        })
        .collect::<Vec<_>>()
        .join(" || ");
    (range, optional)
}

/// The installed package a dependency on `name` resolves to: the one with
/// `version` when the lockfile names it, otherwise the first.
fn resolve(
    graph: &Graph,
    by_name: &HashMap<String, Vec<usize>>,
    name: &str,
    version: Option<&str>,
) -> Option<usize> {
    let candidates = by_name.get(name)?;
    version
        .and_then(|version| {
            candidates
                .iter()
                .find(|&&id| graph.nodes[id].version == version)
        })
        .or(candidates.first())
        .copied()
}

/// The installed package a dependency on `name` with `range` resolves to:
/// the first one whose version satisfies the range, otherwise the first.
fn resolve_range(
    graph: &Graph,
    by_name: &HashMap<String, Vec<usize>>,
    name: &str,
    range: &str,
) -> Option<usize> {
    let candidates = by_name.get(name)?;
    candidates
        .iter()
        .find(|&&id| satisfies(Ecosystem::Pypi, range, &graph.nodes[id].version) == Some(true))
        .or(candidates.first())
        .copied()
}

fn specifier(requirement: &Value) -> String {
    requirement
        .get("specifier")
        .and_then(Value::as_str)
        .unwrap_or(ANY)
        .to_string()
}

fn document(text: &str) -> Result<Value, String> {
    toml::from_str::<toml::Table>(text)
        .map(Value::Table)
        .map_err(|e| e.to_string())
}

fn array(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn table(value: Option<&Value>) -> toml::Table {
    value.and_then(Value::as_table).cloned().unwrap_or_default()
}

fn str_of(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or_default()
}
