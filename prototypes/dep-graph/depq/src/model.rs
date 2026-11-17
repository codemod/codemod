//! The shared shape every lockfile parser maps into, the way a tree-sitter
//! grammar maps source text into a syntax tree.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Eco {
    Npm,
    Cargo,
}

impl Eco {
    pub fn as_str(self) -> &'static str {
        match self {
            Eco::Npm => "npm",
            Eco::Cargo => "cargo",
        }
    }
    pub fn parse(s: &str) -> Option<Eco> {
        match s {
            "npm" => Some(Eco::Npm),
            "cargo" => Some(Eco::Cargo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Prod,
    Dev,
    Peer,
    Optional,
    Build,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Prod => "prod",
            Kind::Dev => "dev",
            Kind::Peer => "peer",
            Kind::Optional => "optional",
            Kind::Build => "build",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "prod" => Kind::Prod,
            "dev" => Kind::Dev,
            "peer" => Kind::Peer,
            "optional" => Kind::Optional,
            "build" => Kind::Build,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub eco: Eco,
    pub name: String,
    pub version: String,
    /// Repo-relative directory when this package is one of the repo's own workspaces.
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: usize,
    /// Resolved target. Peer edges and edges a lockfile doesn't resolve have none.
    pub to: Option<usize>,
    pub name: String,
    /// Declared range, when the lockfile or manifest keeps one (Cargo.lock doesn't).
    pub req: Option<String>,
    pub kind: Kind,
}

#[derive(Debug, Default, Serialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Which lockfiles fed the graph.
    pub sources: Vec<String>,
    #[serde(skip)]
    pub(crate) index: HashMap<(Eco, String, String), usize>,
}

impl Graph {
    pub fn node(&mut self, eco: Eco, name: &str, version: &str) -> usize {
        let key = (eco, name.to_string(), version.to_string());
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        self.nodes.push(Node { eco, name: name.into(), version: version.into(), workspace: None });
        self.index.insert(key, self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    pub fn find(&self, eco: Eco, name: &str, version: &str) -> Option<usize> {
        self.index.get(&(eco, name.to_string(), version.to_string())).copied()
    }

    pub fn edge(&mut self, from: usize, to: Option<usize>, name: &str, req: Option<String>, kind: Kind) {
        let dup = self.edges.iter().rev().take(64).any(|e| e.from == from && e.name == name && e.kind == kind && e.to == to);
        if !dup {
            self.edges.push(Edge { from, to, name: name.into(), req, kind });
        }
    }

    pub fn label(&self, i: usize) -> String {
        let n = &self.nodes[i];
        format!("{}@{}", n.name, n.version)
    }

    /// Several lockfiles (a pnpm workspace next to a Cargo workspace) become one graph.
    pub fn merge(&mut self, other: Graph) {
        let offset: Vec<usize> = other
            .nodes
            .iter()
            .map(|n| {
                let i = self.node(n.eco, &n.name, &n.version);
                if n.workspace.is_some() {
                    self.nodes[i].workspace = n.workspace.clone();
                }
                i
            })
            .collect();
        for e in other.edges {
            self.edges.push(Edge { from: offset[e.from], to: e.to.map(|t| offset[t]), ..e });
        }
        self.sources.extend(other.sources);
    }
}
