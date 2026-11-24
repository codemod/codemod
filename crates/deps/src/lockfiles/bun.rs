//! Bun's text lockfile, `bun.lock` (Bun 1.2+): JSON with trailing commas.
//! `workspaces` maps each workspace directory (`""` is the root) to its
//! package.json sections; `packages` maps an install path of package names
//! (`parent/child` for a copy nested under `parent`) to
//! `["name@version", registry, { dependencies, ... }, integrity]`. A
//! dependency resolves the way Node does, from the nested copies up to the
//! hoisted one. The binary `bun.lockb` is not read.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::npm_sections;
use crate::model::{Ecosystem, Graph, Kind};

pub fn parse(text: &str) -> Result<Graph, String> {
    let doc: Value =
        serde_json::from_str(&without_trailing_commas(text)).map_err(|e| e.to_string())?;
    let workspaces = doc
        .get("workspaces")
        .and_then(Value::as_object)
        .ok_or("no `workspaces` map")?;
    let empty = serde_json::Map::new();
    let packages = doc
        .get("packages")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut graph = Graph::from_lockfile("bun.lock");

    let mut by_dir: HashMap<&str, usize> = HashMap::new();
    for (dir, workspace) in workspaces {
        let dir = if dir.is_empty() { "." } else { dir.as_str() };
        let name = workspace
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| format!("<{dir}>"), String::from);
        let version = workspace
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or(super::UNVERSIONED);
        by_dir.insert(dir, graph.workspace(Ecosystem::Npm, &name, version, dir));
    }

    let mut by_key: HashMap<&str, usize> = HashMap::new();
    let mut links: HashSet<&str> = HashSet::new();
    for (key, entry) in packages {
        let Some(ident) = entry.get(0).and_then(Value::as_str) else {
            continue;
        };
        let Some((name, version)) = split_ident(ident) else {
            continue;
        };
        if let Some(dir) = version.strip_prefix("workspace:") {
            if let Some(&id) = by_dir.get(dir) {
                by_key.insert(key, id);
                links.insert(key);
            }
            continue;
        }
        by_key.insert(key, graph.node(Ecosystem::Npm, name, version));
    }

    let resolve = |from: Option<&str>, name: &str| -> Option<usize> {
        let mut base = from.map(package_path);
        loop {
            let candidate = match &base {
                Some(parts) if !parts.is_empty() => format!("{}/{name}", parts.join("/")),
                _ => name.to_string(),
            };
            if let Some(&id) = by_key.get(candidate.as_str()) {
                return Some(id);
            }
            match &mut base {
                Some(parts) if !parts.is_empty() => {
                    parts.pop();
                }
                _ => return None,
            }
        }
    };

    for (dir, workspace) in workspaces {
        let from = by_dir[if dir.is_empty() { "." } else { dir.as_str() }];
        // A workspace's own nested copies are under its package name.
        let scope = (!dir.is_empty())
            .then(|| workspace.get("name").and_then(Value::as_str))
            .flatten();
        for (name, range, kind) in npm_sections(workspace) {
            let to = if kind == Kind::Peer {
                None
            } else {
                resolve(scope, &name)
            };
            graph.edge(from, to, &name, Some(range), kind);
        }
    }
    for (key, entry) in packages {
        if links.contains(key.as_str()) {
            continue;
        }
        let Some(&from) = by_key.get(key.as_str()) else {
            continue;
        };
        let Some(info) = entry
            .as_array()
            .and_then(|fields| fields.iter().skip(1).find(|field| field.is_object()))
        else {
            continue;
        };
        for (name, range, kind) in npm_sections(info) {
            let to = if kind == Kind::Peer {
                None
            } else {
                resolve(Some(key), &name)
            };
            graph.edge(from, to, &name, Some(range), kind);
        }
    }
    Ok(graph)
}

/// `@scope/name@1.2.3` into name and version.
fn split_ident(ident: &str) -> Option<(&str, &str)> {
    let at = ident.get(1..)?.find('@')? + 1;
    Some((&ident[..at], &ident[at + 1..]))
}

/// The package names an install path is made of: `@a/b/c` is `@a/b`, `c`.
fn package_path(key: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = key;
    while !rest.is_empty() {
        let end = if rest.starts_with('@') {
            rest.match_indices('/').nth(1).map(|(at, _)| at)
        } else {
            rest.find('/')
        };
        match end {
            Some(end) => {
                parts.push(&rest[..end]);
                rest = &rest[end + 1..];
            }
            None => {
                parts.push(rest);
                rest = "";
            }
        }
    }
    parts
}

/// JSON with the commas before a `}` or `]` dropped.
fn without_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    for (at, c) in text.char_indices() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            ',' => {
                let next = text[at + 1..].trim_start().chars().next();
                if !matches!(next, Some('}' | ']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}
