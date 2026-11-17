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
                constraint(&requirement),
            );
        }
        for group in table(metadata.and_then(|m| m.get("requires-dev")))
            .into_iter()
            .map(|(_, v)| v)
        {
            for requirement in array(Some(&group)) {
                ranges.insert(
                    (normalize(str_of(requirement.get("name"))), Kind::Dev),
                    constraint(&requirement),
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
                    .then(|| ranges.get(&(name.clone(), kind)).cloned().flatten())
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
            let (range, optional) = poetry_constraint(&value);
            let name = normalize(&name);
            let to = resolve_in_range(&graph, &by_name, &name, range.as_deref());
            let kind = if optional { Kind::Optional } else { Kind::Prod };
            graph.edge(from, to, &name, range, kind);
        }
    }

    // The project and its own ranges are only in pyproject.toml.
    let manifest = read("pyproject.toml").ok_or("no pyproject.toml beside poetry.lock")?;
    let manifest = document(&manifest).map_err(|e| format!("pyproject.toml: {e}"))?;
    {
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
            let to = resolve_in_range(&graph, &by_name, &name, range.as_deref());
            graph.edge(root, to, &name, range, kind);
        }
    }
    Ok(graph)
}

pub fn pipfile(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc: Json = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut graph = Graph::from_lockfile("Pipfile.lock");
    let mut by_name = HashMap::<String, Vec<usize>>::new();
    // `default`, `develop` and any custom category pipenv was asked for.
    let categories = doc.as_object().into_iter().flatten();
    let mut root = None;
    for (name, entry) in categories
        .filter(|(category, _)| *category != "_meta")
        .filter_map(|(_, packages)| packages.as_object())
        .flatten()
    {
        let name = normalize(name);
        let field = |key: &str| entry.get(key).and_then(Json::as_str);
        let editable = entry.get("editable").and_then(Json::as_bool) == Some(true);
        let id = match field("path") {
            Some(dir) if editable => {
                let id = graph.workspace(Ecosystem::Pypi, &name, UNVERSIONED, dir);
                if dir.trim_end_matches('/') == "." {
                    root = Some(id);
                }
                id
            }
            // A git source has no release; its ref stands in for one.
            _ => {
                let pinned = field("version")
                    .map(|v| v.trim_start_matches("=="))
                    .or_else(|| field("ref"))
                    .unwrap_or(UNVERSIONED);
                graph.node(Ecosystem::Pypi, &name, pinned)
            }
        };
        let ids = by_name.entry(name).or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }

    if let Some(manifest) = read("Pipfile").and_then(|text| document(&text).ok()) {
        // The project installed as itself (`-e .`) is the root, if listed.
        let root =
            root.unwrap_or_else(|| graph.workspace(Ecosystem::Pypi, "<.>", UNVERSIONED, "."));
        let mut categories: Vec<_> = table(Some(&manifest)).into_iter().collect();
        categories.sort_by_key(|(category, _)| category != "packages");
        for (category, packages) in categories {
            let kind = match category.as_str() {
                "packages" => Kind::Prod,
                "source" | "requires" | "scripts" | "pipenv" => continue,
                _ => Kind::Dev,
            };
            for (name, value) in table(Some(&packages)) {
                let name = normalize(&name);
                let to = resolve(&graph, &by_name, &name, None);
                if to != Some(root) {
                    graph.edge(root, to, &name, constraint(&value), kind);
                }
            }
        }
    }
    Ok(graph)
}

/// A PEP 508 requirement (`requests[socks] >= 2.31 ; python_version < "3.13"`)
/// as its normalized name and version specifier; a direct reference
/// (`name @ https://...`) has none.
pub(crate) fn requirement(text: &str) -> (String, Option<String>) {
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
    let range = match range {
        "" => Some(ANY.to_string()),
        _ if range.starts_with('@') => None,
        range => Some(range.to_string()),
    };
    (normalize(&text[..end]), range)
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
fn pyproject_dependencies(manifest: &Value) -> Vec<(String, Option<String>, Kind)> {
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
            let (range, optional) = poetry_constraint(&value);
            let kind = if optional { Kind::Optional } else { kind };
            out.push((normalize(&name), range, kind));
        }
    }
    out
}

/// A Poetry dependency's range and whether it is optional. The value is one
/// constraint, or a list of them split by environment markers; no range
/// when any of them is a git, URL or path source.
fn poetry_constraint(value: &Value) -> (Option<String>, bool) {
    let constraints = match value {
        Value::Array(list) => list.as_slice(),
        value => std::slice::from_ref(value),
    };
    let optional = constraints
        .iter()
        .all(|c| c.get("optional").and_then(Value::as_bool) == Some(true));
    let range = constraints
        .iter()
        .map(constraint)
        .collect::<Option<Vec<_>>>()
        .map(|ranges| ranges.join(" || "));
    (range, optional)
}

/// The installed package a dependency on `name` with `range` resolves to:
/// the first one the range allows, otherwise the first.
fn resolve_in_range(
    graph: &Graph,
    by_name: &HashMap<String, Vec<usize>>,
    name: &str,
    range: Option<&str>,
) -> Option<usize> {
    let candidates = by_name.get(name)?;
    candidates
        .iter()
        .find(|&&id| {
            range.and_then(|range| {
                crate::semver::satisfies(Ecosystem::Pypi, range, &graph.nodes[id].version)
            }) == Some(true)
        })
        .or(candidates.first())
        .copied()
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

/// A dependency's range from a string or a table's `version`: any version
/// when it names none, and no range for a git, URL or path source.
fn constraint(value: &Value) -> Option<String> {
    if let Some(range) = value
        .as_str()
        .or_else(|| value.get("version")?.as_str())
        .or_else(|| value.get("specifier")?.as_str())
    {
        return Some(range.to_string());
    }
    let local = ["git", "url", "path", "file", "editable", "directory"]
        .iter()
        .any(|key| value.get(key).is_some());
    (!local).then(|| ANY.to_string())
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
