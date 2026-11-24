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

use crate::model::{Graph, Kind, Skipped};

mod bun;
mod cargo;
mod npm;
mod pnpm;
mod python;
mod yarn;
mod yarn_classic;

pub use bun::parse as bun;
pub use cargo::{parse as cargo, Manifests as CargoManifests};
pub use npm::parse as package_lock;
pub use pnpm::parse as pnpm;
pub use python::{pipfile, poetry, uv};
pub use yarn::parse as yarn;
pub use yarn_classic::parse as yarn_classic;

/// Reads a file relative to the lockfile's directory.
pub type ReadFile<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Lockfiles this crate reads, by file name.
pub const LOCKFILES: [&str; 9] = [
    "yarn.lock",
    "pnpm-lock.yaml",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "bun.lock",
    "Cargo.lock",
    "uv.lock",
    "poetry.lock",
    "Pipfile.lock",
];

/// The lockfiles at `root`, merged into one graph. `only` instead keeps
/// every lockfile under `root` whose relative path matches one of its globs
/// (`**/yarn.lock` for nested ones); `.gitignore` is respected, and
/// `node_modules` and `target` are skipped. A workspace keeps its lockfile
/// at its root, so nested ones are usually fixtures or examples.
///
/// A lockfile `only` names that cannot be read is an error, and so are
/// globs that match no lockfile. Without `only`, one that cannot be read is
/// left out and listed in [`Graph::skipped`], and a root without lockfiles
/// is an empty graph.
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
        .collect();
    found.sort();
    found.retain(|path| match only {
        [] => !path.contains('/'),
        globs => globs.iter().any(|glob| glob_match(glob, path)),
    });
    if found.is_empty() && !only.is_empty() {
        return Err(format!(
            "no lockfile matches {} under {}",
            only.join(", "),
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
        let parsed = read(file)
            .ok_or_else(|| "cannot read the file".to_string())
            .and_then(|text| match file {
                "yarn.lock" if yarn_classic::is_classic(&text) => {
                    yarn_classic(&text, &read, &yarn_workspaces(&base, &read))
                }
                "yarn.lock" => yarn(&text, &read),
                "pnpm-lock.yaml" => pnpm(&text, &read),
                "package-lock.json" | "npm-shrinkwrap.json" => package_lock(&text, &read),
                "bun.lock" => bun(&text),
                "uv.lock" => uv(&text),
                "poetry.lock" => poetry(&text, &read),
                "Pipfile.lock" => pipfile(&text, &read),
                _ => cargo(&text, &CargoManifests::find(&base)),
            });
        match parsed {
            Ok(mut parsed) => {
                // package-lock.json's parser also reads npm-shrinkwrap.json.
                parsed.lockfiles = vec![file.to_string()];
                graph.merge(parsed, dir)
            }
            Err(error) if only.is_empty() => graph.skipped.push(Skipped { lockfile, error }),
            Err(error) => return Err(format!("{lockfile}: {error}")),
        }
    }
    Ok(graph)
}

/// The directories of a Yarn 1 project's workspaces other than the root,
/// relative to `base`: those with a package.json that the root
/// package.json's `workspaces` globs (or `workspaces.packages`) match and
/// no `!` glob excludes.
fn yarn_workspaces(base: &Path, read: ReadFile) -> Vec<String> {
    let Some(manifest) =
        read("package.json").and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    else {
        return Vec::new();
    };
    let Some(workspaces) = manifest.get("workspaces") else {
        return Vec::new();
    };
    let globs: Vec<&str> = workspaces
        .get("packages")
        .unwrap_or(workspaces)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let normalize = |glob: &str| {
        glob.trim_start_matches("./")
            .trim_end_matches('/')
            .to_string()
    };
    let include: Vec<String> = globs
        .iter()
        .filter(|glob| !glob.starts_with('!'))
        .map(|glob| normalize(glob))
        .collect();
    let exclude: Vec<String> = globs
        .iter()
        .filter_map(|glob| glob.strip_prefix('!'))
        .map(normalize)
        .collect();
    if include.is_empty() {
        return Vec::new();
    }
    let mut dirs: Vec<String> = ignore::WalkBuilder::new(base)
        .hidden(false)
        .filter_entry(|entry| !matches!(entry.file_name().to_str(), Some("node_modules" | ".git")))
        .build()
        .flatten()
        .filter(|entry| entry.file_name() == "package.json")
        .filter_map(|entry| {
            let dir = entry.path().parent()?.strip_prefix(base).ok()?;
            Some(dir.to_string_lossy().replace('\\', "/"))
        })
        .filter(|dir| !dir.is_empty())
        .filter(|dir| {
            include.iter().any(|glob| path_glob_match(glob, dir))
                && !exclude.iter().any(|glob| path_glob_match(glob, dir))
        })
        .collect();
    dirs.sort();
    dirs
}

/// The version of a workspace package that declares none, as Yarn records it.
pub(crate) const UNVERSIONED: &str = "0.0.0-use.local";

/// A package.json's `version`.
pub(crate) fn package_json_version(text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    json.get("version")?.as_str().map(String::from)
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

/// A glob over `/`-separated paths: `*` stays within one directory name,
/// and `**` matches any number of them, none included.
pub(crate) fn path_glob_match(glob: &str, path: &str) -> bool {
    fn matches(glob: &[&str], path: &[&str]) -> bool {
        match glob.split_first() {
            None => path.is_empty(),
            Some((&"**", rest)) => (0..=path.len()).any(|skip| matches(rest, &path[skip..])),
            Some((first, rest)) => path
                .split_first()
                .is_some_and(|(name, path)| star_match(first, name) && matches(rest, path)),
        }
    }
    let glob: Vec<&str> = glob.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    matches(&glob, &path)
}

/// `*` matches any run of characters, and a leading `**/` also matches no
/// directory at all, so `**/yarn.lock` matches the root `yarn.lock`.
pub(crate) fn glob_match(glob: &str, name: &str) -> bool {
    if let Some(rest) = glob.strip_prefix("**/") {
        if glob_match(rest, name) {
            return true;
        }
    }
    star_match(glob, name)
}

fn star_match(glob: &str, name: &str) -> bool {
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
