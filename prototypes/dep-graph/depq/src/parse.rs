//! Lockfile parsers. Each maps one toolchain's lockfile (plus the manifests it
//! needs for ranges and dependency kinds) into the shared `Graph`.

use crate::model::{Eco, Graph, Kind};
use serde_yaml::Value;
use std::collections::HashMap;
use std::path::Path;

/// Reads a repo-relative file; tests pass a map instead of a directory.
pub type ReadFile<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn load_repo(repo: &Path) -> Result<Graph, String> {
    let read = |rel: &str| std::fs::read_to_string(repo.join(rel)).ok();
    let mut graph = Graph::default();
    let mut found = false;
    if let Some(text) = read("yarn.lock") {
        graph.merge(yarn(&text, &read)?);
        found = true;
    }
    if let Some(text) = read("pnpm-lock.yaml") {
        graph.merge(pnpm(&text, &read)?);
        found = true;
    }
    if let Some(text) = read("Cargo.lock") {
        let manifests = cargo_manifests(repo);
        graph.merge(cargo(&text, &manifests)?);
        found = true;
    }
    if !found {
        return Err(format!("no yarn.lock, pnpm-lock.yaml or Cargo.lock in {}", repo.display()));
    }
    Ok(graph)
}

fn s(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn map_of(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_mapping)
        .map(|m| m.iter().filter_map(|(k, v)| Some((s(k)?, s(v).unwrap_or_default()))).collect())
        .unwrap_or_default()
}

/// Declared ranges and kinds from a package.json.
fn package_json_deps(text: &str) -> (Option<String>, Vec<(String, String, Kind)>) {
    let json: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let mut out = vec![];
    for (field, kind) in [
        ("dependencies", Kind::Prod),
        ("devDependencies", Kind::Dev),
        ("peerDependencies", Kind::Peer),
        ("optionalDependencies", Kind::Optional),
    ] {
        if let Some(obj) = json.get(field).and_then(|v| v.as_object()) {
            for (name, range) in obj {
                out.push((name.clone(), range.as_str().unwrap_or("").to_string(), kind));
            }
        }
    }
    (json.get("name").and_then(|v| v.as_str()).map(String::from), out)
}

// ---------- Yarn Berry (yarn.lock is YAML) ----------

pub fn yarn(text: &str, read: ReadFile) -> Result<Graph, String> {
    let doc: Value = serde_yaml::from_str(text).map_err(|e| format!("yarn.lock: {e}"))?;
    let entries = doc.as_mapping().ok_or("yarn.lock: not a map")?;
    let mut g = Graph { sources: vec!["yarn.lock".into()], ..Default::default() };
    let mut by_desc: HashMap<String, usize> = HashMap::new();
    let mut parsed = vec![];

    for (key, entry) in entries {
        let key = s(key).unwrap_or_default();
        if key == "__metadata" {
            continue;
        }
        let Some(resolution) = entry.get("resolution").and_then(s) else { continue };
        let name = name_before_protocol(&resolution);
        let version = entry.get("version").and_then(s).unwrap_or_default();
        let id = g.node(Eco::Npm, &name, &version);
        if let Some(loc) = resolution.split("@workspace:").nth(1) {
            g.nodes[id].workspace = Some(loc.to_string());
        }
        for d in key.split(", ") {
            by_desc.insert(d.trim_matches('"').to_string(), id);
        }
        parsed.push((id, entry.clone()));
    }

    let mut by_name: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for (i, n) in g.nodes.iter().enumerate() {
        by_name.entry(n.name.clone()).or_default().push((i, n.version.clone()));
    }
    let resolve = |name: &str, range: &str| -> Option<usize> {
        let desc = if range.contains(':') { format!("{name}@{range}") } else { format!("{name}@npm:{range}") };
        by_desc.get(&desc).copied().or_else(|| {
            // Root `resolutions` rewrite a descriptor to another (often a patch:), so the
            // original range has no lock key. Fall back to the installed version that fits it.
            let cands = by_name.get(name)?;
            cands
                .iter()
                .find(|(_, v)| crate::ranges::satisfies(Eco::Npm, range, v) == Some(true))
                .or(if cands.len() == 1 { cands.first() } else { None })
                .map(|(i, _)| *i)
        })
    };

    for (id, entry) in parsed {
        let optional: Vec<String> = entry
            .get("dependenciesMeta")
            .and_then(Value::as_mapping)
            .map(|m| m.iter().filter(|(_, v)| v.get("optional").and_then(Value::as_bool) == Some(true)).filter_map(|(k, _)| s(k)).collect())
            .unwrap_or_default();

        if let Some(loc) = g.nodes[id].workspace.clone() {
            // The lockfile merges a workspace's dependencies and devDependencies;
            // its package.json says which is which.
            let pj = if loc == "." { "package.json".to_string() } else { format!("{loc}/package.json") };
            if let Some(text) = read(&pj) {
                let (_, deps) = package_json_deps(&text);
                for (name, range, kind) in deps {
                    let to = if kind == Kind::Peer { None } else { resolve(&name, &range) };
                    g.edge(id, to, &name, Some(range), kind);
                }
                continue;
            }
        }
        for (name, range) in map_of(entry.get("dependencies")) {
            let kind = if optional.contains(&name) { Kind::Optional } else { Kind::Prod };
            let to = resolve(&name, &range);
            g.edge(id, to, &name, Some(strip_npm(&range)), kind);
        }
        for (name, range) in map_of(entry.get("peerDependencies")) {
            g.edge(id, None, &name, Some(strip_npm(&range)), Kind::Peer);
        }
    }
    Ok(g)
}

fn name_before_protocol(resolution: &str) -> String {
    // "@scope/name@npm:1.2.3" -> "@scope/name"; the first '@' after index 0 starts the descriptor.
    match resolution[1..].find('@') {
        Some(i) => resolution[..i + 1].to_string(),
        None => resolution.to_string(),
    }
}

fn strip_npm(range: &str) -> String {
    range.strip_prefix("npm:").unwrap_or(range).to_string()
}

// ---------- pnpm v9 ----------

pub fn pnpm(text: &str, read: ReadFile) -> Result<Graph, String> {
    // pnpm may write a first YAML document for its own package manager; take the one with the packages.
    let doc = serde_yaml::Deserializer::from_str(text)
        .filter_map(|d| Value::deserialize_any(d).ok())
        .filter(|v| v.get("importers").is_some())
        .max_by_key(|v| v.get("snapshots").and_then(Value::as_mapping).map_or(0, |m| m.len()))
        .ok_or("pnpm-lock.yaml: no importers")?;
    let mut g = Graph { sources: vec!["pnpm-lock.yaml".into()], ..Default::default() };

    let key_name_version = |key: &str| -> Option<(String, String)> {
        let base = key.split('(').next()?;
        let i = base[1..].find('@')? + 1;
        Some((base[..i].to_string(), base[i + 1..].to_string()))
    };

    // Workspaces first, so `link:` targets resolve.
    let importers = doc.get("importers").and_then(Value::as_mapping).cloned().unwrap_or_default();
    let mut ws_by_path: HashMap<String, usize> = HashMap::new();
    for (path, _) in &importers {
        let path = s(path).unwrap_or_default();
        let pj = if path == "." { "package.json".into() } else { format!("{path}/package.json") };
        let name = read(&pj).and_then(|t| package_json_deps(&t).0).unwrap_or_else(|| format!("<{path}>"));
        let id = g.node(Eco::Npm, &name, "workspace");
        g.nodes[id].workspace = Some(path.clone());
        ws_by_path.insert(path, id);
    }

    let snapshots = doc.get("snapshots").and_then(Value::as_mapping).cloned().unwrap_or_default();
    let packages = doc.get("packages").and_then(Value::as_mapping).cloned().unwrap_or_default();
    for (key, _) in snapshots.iter().chain(packages.iter()) {
        if let Some((n, v)) = key_name_version(&s(key).unwrap_or_default()) {
            g.node(Eco::Npm, &n, &v);
        }
    }

    let target = |g: &mut Graph, from_dir: Option<&str>, name: &str, version: &str| -> Option<usize> {
        if let Some(link) = version.strip_prefix("link:") {
            let base = from_dir.unwrap_or(".");
            let joined = normalize(&format!("{base}/{link}"));
            return ws_by_path.get(&joined).copied();
        }
        let version = version.split('(').next().unwrap_or(version);
        // An alias resolves to "realname@version".
        if version[1..].contains('@') {
            let (n, v) = key_name_version(version)?;
            return g.find(Eco::Npm, &n, &v);
        }
        g.find(Eco::Npm, name, version)
    };

    for (path, sections) in &importers {
        let path = s(path).unwrap_or_default();
        let from = ws_by_path[&path];
        for (field, kind) in [("dependencies", Kind::Prod), ("devDependencies", Kind::Dev), ("optionalDependencies", Kind::Optional)] {
            for (name, spec) in sections.get(field).and_then(Value::as_mapping).into_iter().flatten() {
                let name = s(name).unwrap_or_default();
                let req = spec.get("specifier").and_then(s);
                let version = spec.get("version").and_then(s).unwrap_or_default();
                let to = target(&mut g, Some(&path), &name, &version);
                g.edge(from, to, &name, req, kind);
            }
        }
        let pj = if path == "." { "package.json".into() } else { format!("{path}/package.json") };
        if let Some(text) = read(&pj) {
            for (name, range, kind) in package_json_deps(&text).1 {
                if kind == Kind::Peer {
                    g.edge(from, None, &name, Some(range), Kind::Peer);
                }
            }
        }
    }

    // Snapshot edges carry resolved versions; the declared range lives in `packages`.
    for (key, snap) in &snapshots {
        let key = s(key).unwrap_or_default();
        let Some((n, v)) = key_name_version(&key) else { continue };
        let from = g.find(Eco::Npm, &n, &v).unwrap();
        let declared: HashMap<String, String> = packages
            .get(Value::String(format!("{n}@{v}")))
            .map(|p| map_of(p.get("dependencies")).into_iter().chain(map_of(p.get("optionalDependencies"))).collect())
            .unwrap_or_default();
        for (field, kind) in [("dependencies", Kind::Prod), ("optionalDependencies", Kind::Optional)] {
            for (name, version) in map_of(snap.get(field)) {
                let to = target(&mut g, None, &name, &version);
                g.edge(from, to, &name, declared.get(&name).cloned(), kind);
            }
        }
    }
    for (key, pkg) in &packages {
        let Some((n, v)) = key_name_version(&s(key).unwrap_or_default()) else { continue };
        let from = g.find(Eco::Npm, &n, &v).unwrap();
        for (name, range) in map_of(pkg.get("peerDependencies")) {
            g.edge(from, None, &name, Some(range), Kind::Peer);
        }
    }
    Ok(g)
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = vec![];
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

trait DeserializeAny {
    fn deserialize_any(d: serde_yaml::Deserializer) -> Result<Value, serde_yaml::Error>;
}
impl DeserializeAny for Value {
    fn deserialize_any(d: serde_yaml::Deserializer) -> Result<Value, serde_yaml::Error> {
        serde::Deserialize::deserialize(d)
    }
}

// ---------- Cargo ----------

/// Workspace member manifests by crate name, plus the root manifest's workspace table.
pub struct CargoManifests {
    pub members: HashMap<String, (String, toml::Value)>,
    pub root: Option<toml::Value>,
}

pub fn cargo_manifests(repo: &Path) -> CargoManifests {
    let mut members = HashMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let fname = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !matches!(fname.as_str(), "target" | "node_modules" | ".git") {
                    stack.push(p);
                }
            } else if fname == "Cargo.toml" {
                if let Some(v) = std::fs::read_to_string(&p).ok().and_then(|t| t.parse::<toml::Value>().ok()) {
                    if let Some(name) = v.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) {
                        let rel = p.parent().unwrap().strip_prefix(repo).unwrap().to_string_lossy().to_string();
                        members.insert(name.to_string(), (if rel.is_empty() { ".".into() } else { rel }, v.clone()));
                    }
                }
            }
        }
    }
    let root = std::fs::read_to_string(repo.join("Cargo.toml")).ok().and_then(|t| t.parse().ok());
    CargoManifests { members, root }
}

pub fn cargo(text: &str, manifests: &CargoManifests) -> Result<Graph, String> {
    let doc: toml::Value = text.parse().map_err(|e| format!("Cargo.lock: {e}"))?;
    let pkgs = doc.get("package").and_then(|p| p.as_array()).cloned().unwrap_or_default();
    let mut g = Graph { sources: vec!["Cargo.lock".into()], ..Default::default() };
    let mut versions: HashMap<String, Vec<String>> = HashMap::new();
    for p in &pkgs {
        let (n, v) = (p["name"].as_str().unwrap_or(""), p["version"].as_str().unwrap_or(""));
        let id = g.node(Eco::Cargo, n, v);
        if p.get("source").is_none() {
            g.nodes[id].workspace = Some(manifests.members.get(n).map(|m| m.0.clone()).unwrap_or_else(|| "?".into()));
        }
        versions.entry(n.into()).or_default().push(v.into());
    }
    for p in &pkgs {
        let from = g.find(Eco::Cargo, p["name"].as_str().unwrap(), p["version"].as_str().unwrap()).unwrap();
        let declared = g.nodes[from].workspace.as_ref().and_then(|_| manifests.members.get(&g.nodes[from].name)).map(|m| cargo_declared(&m.1, manifests.root.as_ref()));
        for d in p.get("dependencies").and_then(|d| d.as_array()).into_iter().flatten() {
            // "name", "name version" or "name version (source)"
            let mut parts = d.as_str().unwrap_or("").split(' ');
            let name = parts.next().unwrap_or("").to_string();
            let to = match parts.next() {
                Some(v) => g.find(Eco::Cargo, &name, v),
                None => versions.get(&name).and_then(|vs| vs.first()).and_then(|v| g.find(Eco::Cargo, &name, v)),
            };
            let (req, kind) = declared
                .as_ref()
                .and_then(|d| d.get(&name).cloned())
                .map(|(r, k)| (Some(r), k))
                .unwrap_or((None, Kind::Prod));
            g.edge(from, to, &name, req, kind);
        }
    }
    Ok(g)
}

/// Declared requirement and kind for each dependency of a workspace crate.
fn cargo_declared(manifest: &toml::Value, root: Option<&toml::Value>) -> HashMap<String, (String, Kind)> {
    let ws_deps = root.and_then(|r| r.get("workspace")).and_then(|w| w.get("dependencies"));
    let mut out = HashMap::new();
    let mut tables = vec![];
    for (field, kind) in [("dependencies", Kind::Prod), ("dev-dependencies", Kind::Dev), ("build-dependencies", Kind::Build)] {
        tables.push((manifest.get(field), kind));
        for t in manifest.get("target").and_then(|t| t.as_table()).into_iter().flat_map(|t| t.values()) {
            tables.push((t.get(field), kind));
        }
    }
    for (table, kind) in tables {
        for (key, spec) in table.and_then(|t| t.as_table()).into_iter().flatten() {
            let mut spec = spec.clone();
            if spec.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
                if let Some(ws) = ws_deps.and_then(|w| w.get(key)) {
                    spec = ws.clone();
                }
            }
            let name = spec.get("package").and_then(|p| p.as_str()).unwrap_or(key).to_string();
            let req = spec.as_str().map(String::from).or_else(|| spec.get("version").and_then(|v| v.as_str()).map(String::from)).unwrap_or_else(|| "*".into());
            let kind = if spec.get("optional").and_then(|o| o.as_bool()) == Some(true) { Kind::Optional } else { kind };
            // A crate listed in both [dependencies] and [dev-dependencies] keeps its prod entry.
            out.entry(name).or_insert((req, kind));
        }
    }
    out
}
