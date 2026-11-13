//! Lockfile parsers. Each maps one toolchain's lockfile, plus the manifests
//! it needs for ranges and kinds, into a [`Graph`].
//!
//! The file names and the pnpm and package-lock.json handling started from
//! Grepkon's catalog (`codemod-app: crates/grep-engine/src/catalog/lockfile.rs`).
//! Grepkon keeps one resolved version per package name and one parent per
//! package; here every `name@version` is a node and every declared
//! dependency an edge, so depth and "first parent" can be derived instead
//! of stored.

use std::collections::HashMap;
use std::path::Path;

use crate::model::{Graph, Kind};

mod cargo;
mod npm;
mod pnpm;
mod yarn;

pub use cargo::{parse as cargo, Manifests as CargoManifests};
pub use npm::parse as package_lock;
pub use pnpm::parse as pnpm;
pub use yarn::parse as yarn;

/// Reads a file relative to the lockfile's directory.
pub type ReadFile<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Lockfiles this crate reads, by file name.
pub const LOCKFILES: [&str; 4] = [
    "yarn.lock",
    "pnpm-lock.yaml",
    "package-lock.json",
    "Cargo.lock",
];

/// Every lockfile under `root`, merged into one graph. `.gitignore` is
/// respected, and `node_modules` and `target` are skipped. `only` keeps
/// the lockfiles whose relative path matches one of its globs.
pub fn load(root: &Path, only: &[String]) -> Result<Graph, String> {
    let mut found: Vec<String> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some("node_modules" | "target" | ".git")
            )
        })
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .filter(|entry| LOCKFILES.contains(&entry.file_name().to_str().unwrap_or_default()))
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?;
            Some(relative.to_string_lossy().replace('\\', "/"))
        })
        .filter(|path: &String| only.is_empty() || only.iter().any(|glob| glob_match(glob, path)))
        .collect();
    found.sort();
    if found.is_empty() {
        return Err(format!(
            "no lockfile ({}) under {}",
            LOCKFILES.join(", "),
            root.display()
        ));
    }

    let mut graph = Graph::default();
    for lockfile in found {
        let (dir, file) = lockfile
            .rsplit_once('/')
            .unwrap_or((".", lockfile.as_str()));
        let base = root.join(dir);
        let read = |path: &str| std::fs::read_to_string(base.join(path)).ok();
        let text = read(file).ok_or_else(|| format!("cannot read {lockfile}"))?;
        let parsed = match file {
            "yarn.lock" => yarn(&text, &read),
            "pnpm-lock.yaml" => pnpm(&text, &read),
            "package-lock.json" => package_lock(&text),
            _ => cargo(&text, &CargoManifests::find(&base)),
        }
        .map_err(|error| format!("{lockfile}: {error}"))?;
        graph.merge(parsed, dir);
    }
    Ok(graph)
}

/// A package.json's name and its declared dependencies with their kinds.
pub(crate) fn package_json(text: &str) -> (Option<String>, Vec<(String, String, Kind)>) {
    let json: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let name = json.get("name").and_then(|v| v.as_str()).map(String::from);
    (name, npm_sections(&json))
}

/// `dependencies`, `devDependencies`, `peerDependencies` and
/// `optionalDependencies` of a package.json-shaped object.
pub(crate) fn npm_sections(object: &serde_json::Value) -> Vec<(String, String, Kind)> {
    let mut out = Vec::new();
    for (field, kind) in [
        ("dependencies", Kind::Prod),
        ("devDependencies", Kind::Dev),
        ("peerDependencies", Kind::Peer),
        ("optionalDependencies", Kind::Optional),
    ] {
        for (name, range) in object
            .get(field)
            .and_then(|v| v.as_object())
            .into_iter()
            .flatten()
        {
            out.push((
                name.clone(),
                range.as_str().unwrap_or_default().to_string(),
                kind,
            ));
        }
    }
    out
}

/// String values of a YAML mapping, as (key, value) pairs.
pub(crate) fn yaml_pairs(value: Option<&serde_yaml::Value>) -> Vec<(String, String)> {
    value
        .and_then(serde_yaml::Value::as_mapping)
        .map(|mapping| {
            mapping
                .iter()
                .filter_map(|(key, value)| {
                    Some((yaml_string(key)?, yaml_string(value).unwrap_or_default()))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn yaml_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `name@1.2.3` (or `@scope/name@1.2.3`) into name and version; the version
/// stops at pnpm's peer suffix `(...)`.
pub(crate) fn split_name_version(key: &str) -> Option<(&str, &str)> {
    let key = key.strip_prefix('/').unwrap_or(key);
    let at = key.get(1..)?.find('@')? + 1;
    let version = key[at + 1..].split('(').next()?;
    (!version.is_empty()).then(|| (&key[..at], version))
}

/// Map of name to every installed version, for range fallbacks.
pub(crate) fn versions_by_name(graph: &Graph) -> HashMap<String, Vec<usize>> {
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();
    for (id, node) in graph.nodes.iter().enumerate() {
        map.entry(node.name.clone()).or_default().push(id);
    }
    map
}

/// `*` matches any run of characters.
fn glob_match(glob: &str, name: &str) -> bool {
    let parts: Vec<&str> = glob.split('*').collect();
    if parts.len() == 1 {
        return glob == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    for middle in &parts[1..parts.len() - 1] {
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}
