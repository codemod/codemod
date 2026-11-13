//! `Cargo.lock`. Packages without a `source` are the workspace's own crates
//! (Grepkon's rule). Cargo.lock keeps resolved versions but no
//! requirements, so the workspace crates' manifests supply ranges and kinds;
//! third-party requirements come from the registry when a query needs them.

use std::collections::HashMap;
use std::path::Path;

use super::ReadFile;
use crate::model::{Ecosystem, Graph, Kind};

/// Workspace member manifests by crate name, with the root manifest.
#[derive(Default)]
pub struct Manifests {
    /// Crate name to (directory, manifest).
    pub members: HashMap<String, (String, toml::Value)>,
    pub root: Option<toml::Value>,
}

impl Manifests {
    /// Every `Cargo.toml` under `dir`, skipping `target` and ignored files.
    pub fn find(dir: &Path) -> Self {
        let mut manifests = Self::default();
        let walk = ignore::WalkBuilder::new(dir)
            .filter_entry(|entry| {
                !matches!(entry.file_name().to_str(), Some("target" | "node_modules"))
            })
            .build();
        for entry in walk.flatten() {
            if entry.file_name() != "Cargo.toml" {
                continue;
            }
            let relative = entry.path().parent().and_then(|p| p.strip_prefix(dir).ok());
            let relative = relative
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let read: ReadFile = &|_| std::fs::read_to_string(entry.path()).ok();
            manifests.add(if relative.is_empty() { "." } else { &relative }, read);
        }
        manifests
    }

    /// Add the manifest in `dir`; `read` returns its text.
    pub fn add(&mut self, dir: &str, read: ReadFile) {
        let Some(manifest) = read("Cargo.toml").and_then(|text| document(&text).ok()) else {
            return;
        };
        if dir == "." {
            self.root = Some(manifest.clone());
        }
        if let Some(name) = manifest
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
        {
            self.members
                .insert(name.to_string(), (dir.to_string(), manifest));
        }
    }
}

pub fn parse(text: &str, manifests: &Manifests) -> Result<Graph, String> {
    let doc = document(text)?;
    let packages = doc
        .get("package")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut graph = Graph::from_lockfile("Cargo.lock");
    let field = |package: &toml::Value, key: &str| {
        package
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let mut versions: HashMap<String, Vec<String>> = HashMap::new();
    let mut ids = Vec::new();
    for package in &packages {
        let (name, version) = (field(package, "name"), field(package, "version"));
        let id = if package.get("source").is_none() {
            let dir = manifests
                .members
                .get(&name)
                .map_or("?", |(dir, _)| dir.as_str());
            graph.workspace(Ecosystem::Cargo, &name, &version, dir)
        } else {
            graph.node(Ecosystem::Cargo, &name, &version)
        };
        versions.entry(name).or_default().push(version);
        ids.push(id);
    }
    for (package, from) in packages.iter().zip(ids) {
        let declared = graph.nodes[from]
            .workspace
            .as_ref()
            .and_then(|_| manifests.members.get(&graph.nodes[from].name))
            .map(|(_, manifest)| declared(manifest, manifests.root.as_ref()))
            .unwrap_or_default();
        for dependency in package
            .get("dependencies")
            .and_then(|d| d.as_array())
            .into_iter()
            .flatten()
        {
            // "name", "name version" or "name version (source)"
            let mut parts = dependency.as_str().unwrap_or_default().split(' ');
            let name = parts.next().unwrap_or_default().to_string();
            let version = parts
                .next()
                .map(String::from)
                .or_else(|| versions.get(&name)?.first().cloned());
            let to = version.and_then(|version| graph.find(Ecosystem::Cargo, &name, &version));
            let (range, kind) = match declared.get(&name) {
                Some((range, kind)) => (Some(range.clone()), *kind),
                None => (None, Kind::Prod),
            };
            graph.edge(from, to, &name, range, kind);
        }
    }
    Ok(graph)
}

fn document(text: &str) -> Result<toml::Value, String> {
    toml::from_str::<toml::Table>(text)
        .map(toml::Value::Table)
        .map_err(|e| e.to_string())
}

/// Requirement and kind of each dependency a workspace crate declares,
/// following `workspace = true` and `package = "..."` renames.
fn declared(manifest: &toml::Value, root: Option<&toml::Value>) -> HashMap<String, (String, Kind)> {
    let inherited = root
        .and_then(|r| r.get("workspace"))
        .and_then(|w| w.get("dependencies"));
    let mut tables = Vec::new();
    for (field, kind) in [
        ("dependencies", Kind::Prod),
        ("dev-dependencies", Kind::Dev),
        ("build-dependencies", Kind::Build),
    ] {
        tables.push((manifest.get(field), kind));
        for target in manifest
            .get("target")
            .and_then(|t| t.as_table())
            .into_iter()
            .flat_map(|t| t.values())
        {
            tables.push((target.get(field), kind));
        }
    }
    let mut out = HashMap::new();
    for (table, kind) in tables {
        for (key, spec) in table.and_then(|t| t.as_table()).into_iter().flatten() {
            let mut spec = spec.clone();
            if spec.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
                if let Some(shared) = inherited.and_then(|w| w.get(key)) {
                    spec = shared.clone();
                }
            }
            let name = spec
                .get("package")
                .and_then(|p| p.as_str())
                .unwrap_or(key)
                .to_string();
            let range = spec
                .as_str()
                .or_else(|| spec.get("version").and_then(|v| v.as_str()))
                .unwrap_or("*")
                .to_string();
            let kind = if spec.get("optional").and_then(|o| o.as_bool()) == Some(true) {
                Kind::Optional
            } else {
                kind
            };
            // Listed in both [dependencies] and [dev-dependencies]: the prod entry wins.
            out.entry(name).or_insert((range, kind));
        }
    }
    out
}
