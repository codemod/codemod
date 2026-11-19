//! The graph every lockfile maps into: nodes are `name@version` per
//! ecosystem, edges keep the declared range, the kind and the workspace.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Npm,
    Cargo,
}

impl Ecosystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Cargo => "cargo",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "npm" => Some(Self::Npm),
            "cargo" => Some(Self::Cargo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Prod,
    Dev,
    Peer,
    Optional,
    Build,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Prod,
        Kind::Dev,
        Kind::Peer,
        Kind::Optional,
        Kind::Build,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prod => "prod",
            Self::Dev => "dev",
            Self::Peer => "peer",
            Self::Optional => "optional",
            Self::Build => "build",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
    /// Directory, relative to the analysed root, when the node is one of the
    /// repository's own packages.
    pub workspace: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: usize,
    /// The installed target; none when the lockfile does not resolve it,
    /// as for peers other than ones pnpm records as installed.
    pub to: Option<usize>,
    pub name: String,
    /// Declared range. Cargo.lock keeps none for third-party crates.
    pub range: Option<String>,
    pub kind: Kind,
}

/// A lockfile that was found but could not be read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Skipped {
    pub lockfile: String,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Lockfiles that fed the graph, relative to the analysed root.
    pub lockfiles: Vec<String>,
    /// Lockfiles found but not read, with why: when none were asked for by
    /// name, one that cannot be read is left out instead of failing.
    pub skipped: Vec<Skipped>,
    /// Third-party packages by `name@version`.
    index: HashMap<(Ecosystem, String, String), usize>,
    /// The first workspace with each `name@version`, for edges that resolve
    /// to the repository's own packages. Kept apart from `index`, so a
    /// registry package never merges into a workspace that shares its
    /// name and version.
    workspace_versions: HashMap<(Ecosystem, String, String), usize>,
    /// Workspaces are also keyed by directory: two lockfiles in one
    /// repository can each have a root package with the same name.
    workspaces: HashMap<(Ecosystem, String, String, String), usize>,
    seen_edges: HashSet<(usize, Option<usize>, String, Kind)>,
}

impl Graph {
    /// An empty graph read from one lockfile.
    pub fn from_lockfile(lockfile: &str) -> Self {
        Self {
            lockfiles: vec![lockfile.to_string()],
            ..Self::default()
        }
    }

    /// The node for `name@version`, created on first use.
    pub fn node(&mut self, ecosystem: Ecosystem, name: &str, version: &str) -> usize {
        let key = (ecosystem, name.to_string(), version.to_string());
        if let Some(&id) = self.index.get(&key) {
            return id;
        }
        self.nodes.push(Node {
            ecosystem,
            name: name.to_string(),
            version: version.to_string(),
            workspace: None,
        });
        self.index.insert(key, self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    /// The repository's own package in `dir`, created on first use.
    pub fn workspace(
        &mut self,
        ecosystem: Ecosystem,
        name: &str,
        version: &str,
        dir: &str,
    ) -> usize {
        let key = (
            ecosystem,
            name.to_string(),
            version.to_string(),
            dir.to_string(),
        );
        if let Some(&id) = self.workspaces.get(&key) {
            return id;
        }
        self.nodes.push(Node {
            ecosystem,
            name: name.to_string(),
            version: version.to_string(),
            workspace: Some(dir.to_string()),
        });
        let id = self.nodes.len() - 1;
        self.workspaces.insert(key, id);
        self.workspace_versions
            .entry((ecosystem, name.to_string(), version.to_string()))
            .or_insert(id);
        id
    }

    /// The node an edge to `name@version` resolves to: one of the
    /// repository's own packages first, as lockfiles list them, then a
    /// third-party package.
    pub fn find(&self, ecosystem: Ecosystem, name: &str, version: &str) -> Option<usize> {
        let key = (ecosystem, name.to_string(), version.to_string());
        self.workspace_versions
            .get(&key)
            .or_else(|| self.index.get(&key))
            .copied()
    }

    pub fn edge(
        &mut self,
        from: usize,
        to: Option<usize>,
        name: &str,
        range: Option<String>,
        kind: Kind,
    ) {
        if self.seen_edges.insert((from, to, name.to_string(), kind)) {
            self.edges.push(Edge {
                from,
                to,
                name: name.to_string(),
                range,
                kind,
            });
        }
    }

    pub fn label(&self, id: usize) -> String {
        let node = &self.nodes[id];
        format!("{}@{}", node.name, node.version)
    }

    /// Add another lockfile's graph, with its workspace paths under `dir`.
    pub fn merge(&mut self, other: Graph, dir: &str) {
        let ids: Vec<usize> = other
            .nodes
            .iter()
            .map(|node| match &node.workspace {
                Some(workspace) => self.workspace(
                    node.ecosystem,
                    &node.name,
                    &node.version,
                    &join(dir, workspace),
                ),
                None => self.node(node.ecosystem, &node.name, &node.version),
            })
            .collect();
        for edge in other.edges {
            self.edge(
                ids[edge.from],
                edge.to.map(|to| ids[to]),
                &edge.name,
                edge.range,
                edge.kind,
            );
        }
        self.lockfiles
            .extend(other.lockfiles.iter().map(|lockfile| join(dir, lockfile)));
    }
}

/// `dir/path`, normalised, with "." for the root.
pub(crate) fn join(dir: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in dir.split('/').chain(path.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}
