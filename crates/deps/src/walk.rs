//! Reading the graph: edges into and out of a node, edges by dependency
//! name, and shortest chains between nodes. Built once per graph, then
//! shared by every question a workflow asks.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::model::{Graph, Kind};

pub struct Walk {
    pub graph: Graph,
    out: Vec<Vec<usize>>,
    into: Vec<Vec<usize>>,
    named: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
}

impl Walk {
    pub fn new(graph: Graph) -> Self {
        let mut out = vec![Vec::new(); graph.nodes.len()];
        let mut into = vec![Vec::new(); graph.nodes.len()];
        let mut named: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, edge) in graph.edges.iter().enumerate() {
            out[edge.from].push(index);
            if let Some(to) = edge.to {
                into[to].push(index);
            }
            named.entry(edge.name.clone()).or_default().push(index);
        }
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (id, node) in graph.nodes.iter().enumerate() {
            by_name.entry(node.name.clone()).or_default().push(id);
        }
        Self {
            graph,
            out,
            into,
            named,
            by_name,
        }
    }

    /// Every node named `name`, one per installed version and workspace.
    pub fn nodes_named(&self, name: &str) -> &[usize] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    pub fn workspaces(&self) -> Vec<usize> {
        (0..self.graph.nodes.len())
            .filter(|&id| self.graph.nodes[id].workspace.is_some())
            .collect()
    }

    /// Edges a node declares.
    pub fn dependencies(&self, id: usize, kinds: &[Kind]) -> Vec<usize> {
        self.filter(&self.out[id], kinds)
    }

    /// Edges installed onto a node. Peer edges name a package without
    /// resolving it; [`Walk::declaring`] finds those.
    pub fn dependents(&self, id: usize, kinds: &[Kind]) -> Vec<usize> {
        self.filter(&self.into[id], kinds)
    }

    /// Edges that declare a dependency on `name`, resolved or not.
    pub fn declaring(&self, name: &str, kinds: &[Kind]) -> Vec<usize> {
        self.filter(self.named.get(name).map_or(&[], Vec::as_slice), kinds)
    }

    /// The shortest chain from each start to any target, following installed
    /// edges of `kinds`. A chain does not pass through another workspace,
    /// so each workspace answers for its own dependencies.
    pub fn chains(&self, from: &[usize], to: &HashSet<usize>, kinds: &[Kind]) -> Vec<Vec<usize>> {
        let mut found = Vec::new();
        for &start in from {
            let mut previous: HashMap<usize, usize> = HashMap::new();
            let mut seen = HashSet::from([start]);
            let mut queue = VecDeque::from([start]);
            let mut end = None;
            'search: while let Some(id) = queue.pop_front() {
                for &edge in &self.dependencies(id, kinds) {
                    let Some(child) = self.graph.edges[edge].to else {
                        continue;
                    };
                    if !seen.insert(child) {
                        continue;
                    }
                    previous.insert(child, id);
                    if to.contains(&child) {
                        end = Some(child);
                        break 'search;
                    }
                    if self.graph.nodes[child].workspace.is_none() {
                        queue.push_back(child);
                    }
                }
            }
            if let Some(end) = end {
                let mut chain = vec![end];
                let mut id = end;
                while let Some(&parent) = previous.get(&id) {
                    chain.push(parent);
                    id = parent;
                }
                chain.reverse();
                found.push(chain);
            }
        }
        found
    }

    fn filter(&self, edges: &[usize], kinds: &[Kind]) -> Vec<usize> {
        edges
            .iter()
            .copied()
            .filter(|&edge| kinds.is_empty() || kinds.contains(&self.graph.edges[edge].kind))
            .collect()
    }
}
