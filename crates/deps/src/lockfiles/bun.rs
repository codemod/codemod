//! `bun.lock`, JSON with trailing commas. `workspaces` holds each
//! workspace's declared dependencies by directory (`""` is the root);
//! `packages` maps an install path to `[name@version, registry, metadata,
//! integrity]`, the metadata holding the package's own ranges. A path
//! `a/b` is `b` installed under `a`, and a workspace's own copies sit under
//! its package key, so a dependency resolves as Node does, from the
//! deepest path up. The binary `bun.lockb` is not read ([`LOCKB`]).

use std::collections::HashMap;

use serde_json::Value;

use super::{npm_sections, package_json_version, ReadFile};
use crate::model::{join, Ecosystem, Graph, Kind};

pub const LOCKB: &str = "bun.lockb is binary and not read; write bun.lock with `bun install --save-text-lockfile --frozen-lockfile --lockfile-only` and delete bun.lockb";

pub fn parse(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc: Value =
        serde_json::from_str(&without_trailing_commas(text)).map_err(|e| e.to_string())?;
    let object = |field: &str| doc.get(field).and_then(Value::as_object);
    let workspaces = object("workspaces").ok_or("no `workspaces` map")?;
    let packages = object("packages").cloned().unwrap_or_default();
    let mut graph = Graph::from_lockfile("bun.lock");

    let mut by_dir: HashMap<&str, usize> = HashMap::new();
    for (dir, workspace) in workspaces {
        let dir = if dir.is_empty() { "." } else { dir.as_str() };
        let field = |key: &str| workspace.get(key).and_then(Value::as_str).map(String::from);
        let name = field("name").unwrap_or_else(|| format!("<{dir}>"));
        // bun.lock keeps no version for the root; package.json does.
        let version = field("version")
            .or_else(|| read(&join(dir, "package.json")).and_then(|t| package_json_version(&t)))
            .unwrap_or_else(|| super::UNVERSIONED.to_string());
        by_dir.insert(dir, graph.workspace(Ecosystem::Npm, &name, &version, dir));
    }

    // Install path to node; a workspace's path is its package key.
    let mut by_path: HashMap<&str, usize> = HashMap::new();
    let mut workspace_paths: HashMap<usize, &str> = HashMap::new();
    for (path, package) in &packages {
        let Some(ident) = package.get(0).and_then(Value::as_str) else {
            continue;
        };
        let Some(at) = ident
            .get(1..)
            .and_then(|rest| rest.find('@'))
            .map(|at| at + 1)
        else {
            continue;
        };
        let (name, version) = (&ident[..at], &ident[at + 1..]);
        let id = match version.strip_prefix("workspace:") {
            Some(dir) => match by_dir.get(dir) {
                Some(&id) => {
                    workspace_paths.insert(id, path);
                    id
                }
                None => continue,
            },
            None => graph.node(Ecosystem::Npm, name, version),
        };
        by_path.insert(path, id);
    }

    let resolve = |from: &str, name: &str| -> Option<usize> {
        let mut base = segments(from);
        loop {
            let mut candidate = base.clone();
            candidate.push(name);
            if let Some(&id) = by_path.get(candidate.join("/").as_str()) {
                return Some(id);
            }
            base.pop()?;
        }
    };
    let add_edges = |graph: &mut Graph, from: usize, path: &str, declared: &Value| {
        for (name, range, kind) in npm_sections(declared) {
            let to = if kind == Kind::Peer {
                None
            } else {
                resolve(path, &name)
            };
            graph.edge(from, to, &name, Some(range), kind);
        }
    };

    for (dir, workspace) in workspaces {
        let dir = if dir.is_empty() { "." } else { dir.as_str() };
        let id = by_dir[dir];
        let path = match workspace_paths.get(&id) {
            Some(path) => path,
            None if dir == "." => "",
            None => graph.nodes[id].name.as_str(),
        }
        .to_string();
        add_edges(&mut graph, id, &path, workspace);
    }
    for (path, package) in &packages {
        let Some(&from) = by_path.get(path.as_str()) else {
            continue;
        };
        if graph.nodes[from].workspace.is_none() {
            if let Some(metadata) = package.get(2) {
                add_edges(&mut graph, from, path, metadata);
            }
        }
    }
    Ok(graph)
}

/// `a/@scope/b/c` into `a`, `@scope/b`, `c`.
fn segments(path: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = path;
    while !rest.is_empty() {
        let mut end = rest.find('/').unwrap_or(rest.len());
        if rest.starts_with('@') {
            end = rest[end..]
                .get(1..)
                .and_then(|after| after.find('/'))
                .map_or(rest.len(), |slash| end + 1 + slash);
        }
        out.push(&rest[..end]);
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    out
}

/// JSON text without the commas bun writes before `}` and `]`.
fn without_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    for (at, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
        } else if c == '"' {
            in_string = true;
        } else if c == ',' && text[at + 1..].trim_start().starts_with(['}', ']']) {
            continue;
        }
        out.push(c);
    }
    out
}
