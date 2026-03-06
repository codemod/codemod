use std::path::{Component, Path, PathBuf};

pub(crate) fn has_parent_path_components(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

pub(crate) fn normalize_relative_path(configured_path: &str) -> Option<PathBuf> {
    if configured_path.is_empty() {
        return None;
    }

    let parsed = Path::new(configured_path);
    if parsed.is_absolute() {
        return None;
    }

    let mut normalized = PathBuf::new();
    for component in parsed.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

pub(crate) fn resolve_relative_path_within_root(
    root: &Path,
    relative_path: &str,
) -> Option<PathBuf> {
    let normalized = normalize_relative_path(relative_path.trim())?;
    Some(root.join(normalized))
}
