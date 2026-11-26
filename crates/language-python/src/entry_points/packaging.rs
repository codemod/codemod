//! Packaging metadata: console scripts and plugin entry points declared in
//! `pyproject.toml` (`[project.scripts]`, `[project.gui-scripts]`,
//! `[project.entry-points.<group>]`, `[tool.poetry.scripts]`) and
//! `setup.cfg` (`[options.entry_points]`). A value `pkg.mod:func` (or
//! `pkg.mod:Class.method`) names the function the installed command calls.

use super::frameworks::packaging as table;
use super::{Build, Val};
use std::path::{Path, PathBuf};

/// Directories the manifest search does not enter.
const SKIPPED: &[&str] = &[
    "node_modules",
    "__pycache__",
    "site-packages",
    "venv",
    "env",
    "build",
    "dist",
];

/// How deep below the workspace root manifests are looked for.
const MAX_DEPTH: usize = 3;

pub(super) fn run(build: &mut Build) {
    let Some(root) = build.root.clone() else {
        return;
    };
    for manifest in manifests(&root) {
        let Ok(text) = build
            .owner
            .manifests
            .get_or_read(&manifest, || std::fs::read_to_string(&manifest))
        else {
            continue;
        };
        let entries = if manifest.ends_with("pyproject.toml") {
            pyproject(&text)
        } else {
            setup_cfg(&text)
        };
        let dir = manifest.parent().unwrap_or(&root).to_path_buf();
        for (kind, label, value) in entries {
            if let Some((path, def)) = target(build, &dir, &value) {
                build.emit(&path, def, kind, label);
            }
        }
    }
}

/// `pyproject.toml` and `setup.cfg` files at the root and a few levels down.
fn manifests(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for name in ["pyproject.toml", "setup.cfg"] {
            let file = dir.join(name);
            if file.is_file() {
                found.push(file);
            }
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_ok_and(|t| t.is_dir())
                && !name.starts_with('.')
                && !SKIPPED.contains(&name.as_str())
            {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// (kind, label, value) for each script and plugin entry point.
fn pyproject(text: &str) -> Vec<(&'static str, String, String)> {
    let Ok(document) = toml::from_str::<toml::Table>(text) else {
        return Vec::new();
    };
    let table_at = |keys: &[&str]| -> Option<toml::Table> {
        let mut current = &document;
        for key in keys {
            current = current.get(*key)?.as_table()?;
        }
        Some(current.clone())
    };
    let mut found = Vec::new();
    for (keys, kind) in table::PYPROJECT_SCRIPTS {
        for (name, value) in table_at(keys).unwrap_or_default() {
            // Poetry also takes `{ reference = "...", type = "console" }`
            // and `{ callable = "..." }`.
            let value = match &value {
                toml::Value::String(value) => Some(value.clone()),
                toml::Value::Table(table) => table
                    .get("callable")
                    .or_else(|| table.get("reference"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                _ => None,
            };
            if let Some(value) = value {
                found.push((*kind, name.clone(), value));
            }
        }
    }
    for (group, entries) in table_at(table::PYPROJECT_GROUPS).unwrap_or_default() {
        let Some(entries) = entries.as_table() else {
            continue;
        };
        for (name, value) in entries {
            if let Some(value) = value.as_str() {
                found.push((
                    table::PLUGIN_KIND,
                    format!("{group}:{name}"),
                    value.to_string(),
                ));
            }
        }
    }
    found
}

/// `setup.cfg`'s `[options.entry_points]`: `group =` then indented
/// `name = value` lines.
fn setup_cfg(text: &str) -> Vec<(&'static str, String, String)> {
    let mut found = Vec::new();
    let mut in_section = false;
    let mut group: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        if trimmed.starts_with('[') {
            in_section = trimmed == "[options.entry_points]";
            group = None;
            continue;
        }
        if !in_section {
            continue;
        }
        let indented = line.starts_with([' ', '\t']);
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if !indented {
            group = Some(key.to_string());
            if value.is_empty() {
                continue;
            }
        }
        let Some(group) = &group else {
            continue;
        };
        let (name, value) = if indented {
            (key, value)
        } else {
            match value.split_once('=') {
                Some((name, value)) => (name.trim(), value.trim()),
                None => continue,
            }
        };
        if table::SETUP_CFG_SCRIPT_GROUPS.contains(&group.as_str()) {
            found.push(("script", name.to_string(), value.to_string()));
        } else {
            found.push((
                table::PLUGIN_KIND,
                format!("{group}:{name}"),
                value.to_string(),
            ));
        }
    }
    found
}

/// The function `pkg.mod:attr.path` names, looked up from the manifest's
/// directory (and its `src/`).
fn target(build: &Build, dir: &Path, value: &str) -> Option<(PathBuf, usize)> {
    // `pkg.mod:func [extra]`
    let value = value.split('[').next()?.trim();
    let (module, attrs) = value.split_once(':')?;
    let module = module.trim();
    let found = [
        dir.join("__entry_point__.py"),
        dir.join("src").join("__entry_point__.py"),
    ]
    .iter()
    .find_map(|from| build.module_path(from, module))?;
    let mut current = Val::Module(found);
    for attr in attrs.trim().split('.') {
        current = build.attr(current, attr, 0);
    }
    build.function(&current)
}
