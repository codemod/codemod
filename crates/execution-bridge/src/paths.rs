//! Path containment for the JSSG worker.
//!
//! Every path that crosses the worker boundary is checked here, on both
//! directions, against the canonical target root:
//!
//! - source paths TypeScript sends (`index.path`, `transform.path`) must be
//!   safe relative paths whose nearest existing ancestor resolves inside the
//!   root, so a symlinked file or directory cannot point outside it;
//! - sandbox-produced paths (`rename_to`, `jssgTransform` targets, staged
//!   `write()` targets) may be absolute or relative and are normalized to a
//!   root-relative `/`-separated form after the same check.
//!
//! TypeScript repeats the checks before staging and again before writing
//! (`packages/orchestration/src/paths.ts`). This is deliberate: neither side
//! trusts the other's validation.

use std::path::{Component, Path, PathBuf};

use crate::validate_relative_path;

/// Canonicalize a root directory that must exist.
pub fn canonical_root(path: &Path, name: &str) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve {name} '{}': {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{name} '{}' is not a directory", path.display()));
    }
    Ok(canonical)
}

/// Resolve a wire-relative source path beneath `root`. The result is the
/// real absolute path (symlinks in existing ancestors resolved).
pub fn resolve_source(root: &Path, relative: &str, name: &str) -> Result<PathBuf, String> {
    validate_relative_path(relative, name)?;
    let candidate = root.join(Path::new(relative));
    contain(root, &candidate, name)
}

/// Normalize a sandbox-produced path to its root-relative wire form.
pub fn normalize_output(root: &Path, requested: &Path, name: &str) -> Result<String, String> {
    if requested.as_os_str().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let real = contain(root, &candidate, name)?;
    let relative = real
        .strip_prefix(root)
        .map_err(|_| format!("{name} '{}' escapes the target root", requested.display()))?;
    to_wire(relative, name)
}

/// Check that `candidate` lies inside `root` once symlinks in its nearest
/// existing ancestor are resolved, and return that resolved path.
fn contain(root: &Path, candidate: &Path, name: &str) -> Result<PathBuf, String> {
    if candidate
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(format!(
            "{name} '{}' contains parent traversal",
            candidate.display()
        ));
    }
    let existing = candidate
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| format!("no existing ancestor for {name} '{}'", candidate.display()))?;
    let canonical = existing
        .canonicalize()
        .map_err(|error| format!("failed to resolve {name} '{}': {error}", existing.display()))?;
    if !canonical.starts_with(root) {
        return Err(format!(
            "{name} '{}' escapes the target root",
            candidate.display()
        ));
    }
    let tail = candidate
        .strip_prefix(existing)
        .map_err(|_| format!("failed to resolve {name} '{}'", candidate.display()))?;
    let real = canonical.join(tail);
    if !real.starts_with(root) {
        return Err(format!(
            "{name} '{}' escapes the target root",
            candidate.display()
        ));
    }
    Ok(real)
}

/// Root-relative path as `/`-separated components. Components that contain
/// `\` are refused because the TypeScript side splits on both separators.
fn to_wire(relative: &Path, name: &str) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let text = part
                    .to_str()
                    .ok_or_else(|| format!("{name} '{}' is not valid UTF-8", relative.display()))?;
                if text.contains('\\') {
                    return Err(format!(
                        "{name} '{}' contains a backslash",
                        relative.display()
                    ));
                }
                parts.push(text.to_string());
            }
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "{name} '{}' is not a plain relative path",
                    relative.display()
                ))
            }
        }
    }
    if parts.is_empty() {
        return Err(format!("{name} resolves to the target root itself"));
    }
    Ok(parts.join("/"))
}
