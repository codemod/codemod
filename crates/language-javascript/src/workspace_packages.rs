//! Workspace packages by name, read from the root `package.json`
//! (`workspaces`) or `pnpm-workspace.yaml` (`packages`), so that
//! `import { Button } from "@acme/ui"` resolves to the package's source on a
//! fresh clone: no `node_modules`, and no `dist/` built yet.

use std::path::{Path, PathBuf};

/// Directories under a `**` pattern are searched this deep.
const MAX_GLOB_DEPTH: usize = 4;

/// Reads a file's text; None when it is missing.
pub(crate) type ReadFile<'r> = &'r dyn Fn(&Path) -> Option<String>;
/// Lists the directories directly inside one.
pub(crate) type ListDirs<'r> = &'r dyn Fn(&Path) -> Vec<PathBuf>;

#[derive(Debug, Default)]
pub(crate) struct WorkspacePackages {
    /// (package name, package directory), longest name first.
    packages: Vec<(String, PathBuf)>,
}

impl WorkspacePackages {
    pub(crate) fn discover(root: &Path, read: ReadFile<'_>, list_dirs: ListDirs<'_>) -> Self {
        let mut packages = Vec::new();
        for pattern in patterns(root, read) {
            for dir in expand(root, &pattern, list_dirs) {
                let Some(manifest) = read(&dir.join("package.json")) else {
                    continue;
                };
                let name = serde_json::from_str::<serde_json::Value>(&manifest)
                    .ok()
                    .and_then(|json| json.get("name")?.as_str().map(str::to_string));
                if let Some(name) = name {
                    if !packages.iter().any(|(known, _)| known == &name) {
                        packages.push((name, dir));
                    }
                }
            }
        }
        packages.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        Self { packages }
    }

    /// The package a bare specifier names, and the subpath after the name
    /// (`""` for the package itself).
    pub(crate) fn find<'s>(&self, specifier: &'s str) -> Option<(&Path, &'s str)> {
        self.packages.iter().find_map(|(name, dir)| {
            let rest = specifier.strip_prefix(name.as_str())?;
            if rest.is_empty() {
                Some((dir.as_path(), ""))
            } else {
                rest.strip_prefix('/').map(|sub| (dir.as_path(), sub))
            }
        })
    }
}

/// Workspace globs from the root `package.json` and `pnpm-workspace.yaml`.
fn patterns(root: &Path, read: ReadFile<'_>) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(json) = read(&root.join("package.json"))
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    {
        let list = match json.get("workspaces") {
            Some(serde_json::Value::Array(list)) => Some(list),
            // Yarn's { packages: [...], nohoist: [...] }.
            Some(object) => object.get("packages").and_then(|p| p.as_array()),
            None => None,
        };
        found.extend(
            list.into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(str::to_string)),
        );
    }
    if let Some(yaml) = read(&root.join("pnpm-workspace.yaml")) {
        let mut in_packages = false;
        for line in yaml.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if !line.starts_with([' ', '\t', '-']) {
                in_packages = trimmed.starts_with("packages:");
                continue;
            }
            if let (true, Some(item)) = (in_packages, trimmed.strip_prefix('-')) {
                let item = item.split(" #").next().unwrap_or("").trim();
                found.push(item.trim_matches(|c| c == '"' || c == '\'').to_string());
            }
        }
    }
    found.retain(|p| !p.is_empty() && !p.starts_with('!'));
    found
}

/// The directories a workspace glob (`packages/*`, `apps/**`, `tools/cli`)
/// matches.
fn expand(root: &Path, pattern: &str, list_dirs: ListDirs<'_>) -> Vec<PathBuf> {
    let mut current = vec![root.to_path_buf()];
    for segment in pattern.trim_start_matches("./").split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        let mut next = Vec::new();
        for dir in &current {
            match segment {
                "**" => {
                    let mut frontier = vec![dir.clone()];
                    next.push(dir.clone());
                    for _ in 0..MAX_GLOB_DEPTH {
                        let mut deeper = Vec::new();
                        for d in &frontier {
                            for child in list_dirs(d) {
                                if child.file_name().is_some_and(|n| n != "node_modules") {
                                    deeper.push(child);
                                }
                            }
                        }
                        next.extend(deeper.iter().cloned());
                        frontier = deeper;
                    }
                }
                _ if segment.contains('*') => {
                    for child in list_dirs(dir) {
                        let name = child.file_name().map(|n| n.to_string_lossy().to_string());
                        if name.is_some_and(|n| n != "node_modules" && glob_match(segment, &n)) {
                            next.push(child);
                        }
                    }
                }
                _ => next.push(dir.join(segment)),
            }
        }
        current = next;
    }
    current
}

/// `*` matches any run of characters, within one path segment.
fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Paths (relative to the package directory, without an extension) to try
/// for `subpath` of a package, most likely first: what its `package.json`
/// points at, then the same path under `src/` when it points at build
/// output (`dist/`, `lib/`, …) that may not exist yet, then `src/index` and
/// `index`.
pub(crate) fn entry_candidates(dir: &Path, subpath: &str, read: ReadFile<'_>) -> Vec<String> {
    let manifest = read(&dir.join("package.json"))
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let mut targets: Vec<String> = Vec::new();
    if let Some(json) = &manifest {
        let key = if subpath.is_empty() {
            ".".to_string()
        } else {
            format!("./{subpath}")
        };
        let exported = match json.get("exports") {
            Some(serde_json::Value::String(target)) if subpath.is_empty() => {
                Some(serde_json::Value::String(target.clone()))
            }
            Some(serde_json::Value::Object(map)) if map.keys().all(|k| k.starts_with('.')) => {
                map.get(&key).cloned()
            }
            // { "import": …, "require": … } is the "." entry.
            Some(serde_json::Value::Object(map)) if subpath.is_empty() => {
                Some(serde_json::Value::Object(map.clone()))
            }
            _ => None,
        };
        if let Some(exported) = exported {
            collect_targets(&exported, &mut targets);
        }
        if subpath.is_empty() {
            for field in ["source", "module", "main", "types", "typings"] {
                if let Some(target) = json.get(field).and_then(|v| v.as_str()) {
                    targets.push(target.to_string());
                }
            }
        }
    }
    if !subpath.is_empty() {
        targets.push(subpath.to_string());
    }

    let mut candidates: Vec<String> = Vec::new();
    let mut push = |candidate: String| {
        if !candidate.is_empty() && !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    };
    for target in &targets {
        let plain = strip_extension(target.trim_start_matches("./"));
        push(plain.clone());
        push(source_twin(&plain));
        if !subpath.is_empty() {
            push(format!("src/{plain}"));
        }
    }
    if subpath.is_empty() {
        push("src/index".to_string());
        push("index".to_string());
    }
    candidates
}

/// Every path string in an `exports` entry, conditions in order.
fn collect_targets(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(target) => out.push(target.clone()),
        serde_json::Value::Object(map) => {
            for condition in map.values() {
                collect_targets(condition, out);
            }
        }
        serde_json::Value::Array(list) => {
            for item in list {
                collect_targets(item, out);
            }
        }
        _ => {}
    }
}

fn strip_extension(path: &str) -> String {
    for extension in [
        ".d.ts", ".d.mts", ".d.cts", ".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".cts", ".tsx",
    ] {
        if let Some(stripped) = path.strip_suffix(extension) {
            return stripped.to_string();
        }
    }
    path.to_string()
}

/// `dist/index` -> `src/index`: where build output usually comes from.
fn source_twin(path: &str) -> String {
    let mut segments = path.splitn(2, '/');
    let first = segments.next().unwrap_or("");
    let rest = segments.next().unwrap_or("");
    let build_dir = matches!(
        first,
        "dist" | "lib" | "build" | "out" | "esm" | "cjs" | "es" | "module" | "types"
    );
    match (build_dir, rest.is_empty()) {
        (true, false) => {
            // dist/esm/index -> src/index
            let rest = rest
                .strip_prefix("esm/")
                .or_else(|| rest.strip_prefix("cjs/"))
                .unwrap_or(rest);
            format!("src/{rest}")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        files
            .iter()
            .map(|(path, text)| (PathBuf::from(path), text.to_string()))
            .collect()
    }

    fn dirs_of(files: &HashMap<PathBuf, String>, dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = files
            .keys()
            .filter_map(|path| {
                let rest = path.strip_prefix(dir).ok()?;
                let first = rest.components().next()?;
                (rest.components().count() > 1).then(|| dir.join(first))
            })
            .collect();
        found.sort();
        found.dedup();
        found
    }

    #[test]
    fn npm_and_pnpm_workspaces() {
        let files = tree(&[
            (
                "/r/package.json",
                r#"{ "workspaces": ["packages/*", "apps/web"] }"#,
            ),
            (
                "/r/pnpm-workspace.yaml",
                "packages:\n  - 'tools/**'\n  - \"!**/test\"\n",
            ),
            ("/r/packages/ui/package.json", r#"{ "name": "@acme/ui" }"#),
            (
                "/r/packages/ui-kit/package.json",
                r#"{ "name": "@acme/ui-kit" }"#,
            ),
            ("/r/apps/web/package.json", r#"{ "name": "web" }"#),
            ("/r/tools/x/cli/package.json", r#"{ "name": "cli" }"#),
        ]);
        let read = |p: &Path| files.get(p).cloned();
        let list = |p: &Path| dirs_of(&files, p);
        let packages = WorkspacePackages::discover(Path::new("/r"), &read, &list);
        assert_eq!(
            packages.find("@acme/ui"),
            Some((Path::new("/r/packages/ui"), ""))
        );
        assert_eq!(
            packages.find("@acme/ui-kit/button"),
            Some((Path::new("/r/packages/ui-kit"), "button"))
        );
        assert_eq!(packages.find("web"), Some((Path::new("/r/apps/web"), "")));
        assert_eq!(
            packages.find("cli"),
            Some((Path::new("/r/tools/x/cli"), ""))
        );
        assert_eq!(packages.find("@acme/uix"), None);
        assert_eq!(packages.find("react"), None);
    }

    #[test]
    fn entry_falls_back_from_build_output_to_source() {
        let files = tree(&[(
            "/p/package.json",
            r#"{ "main": "dist/index.js", "exports": { ".": { "import": "./dist/esm/index.mjs" }, "./button": "./dist/button.js" } }"#,
        )]);
        let read = |p: &Path| files.get(p).cloned();
        assert_eq!(
            entry_candidates(Path::new("/p"), "", &read),
            ["dist/esm/index", "src/index", "dist/index", "index"]
        );
        assert_eq!(
            entry_candidates(Path::new("/p"), "button", &read),
            ["dist/button", "src/button", "src/dist/button", "button"]
        );
        assert!(glob_match("ui-*", "ui-kit"));
        assert!(!glob_match("ui-*", "core"));
    }
}
