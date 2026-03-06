use crate::utils::path_safety::normalize_relative_path;
use log::warn;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const AGENTS_SKILL_ROOT_RELATIVE_PATH: &str = "agents/skill";
pub(crate) const SKILL_FILE_NAME: &str = "SKILL.md";

pub(crate) fn derive_skill_name_from_package_name(package_name: &str) -> String {
    package_name
        .rsplit('/')
        .next()
        .unwrap_or(package_name)
        .trim_start_matches('@')
        .to_string()
}

pub(crate) fn expected_authored_skill_dir(package_dir: &Path, package_name: &str) -> PathBuf {
    package_dir
        .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
        .join(derive_skill_name_from_package_name(package_name))
}

pub(crate) fn expected_authored_skill_file(package_dir: &Path, package_name: &str) -> PathBuf {
    expected_authored_skill_dir(package_dir, package_name).join(SKILL_FILE_NAME)
}

pub(crate) fn expected_authored_skill_relative_file(package_name: &str) -> String {
    format!(
        "./{}/{}/{}",
        AGENTS_SKILL_ROOT_RELATIVE_PATH,
        derive_skill_name_from_package_name(package_name),
        SKILL_FILE_NAME
    )
}

pub(crate) fn resolve_configured_skill_file_path(
    package_dir: &Path,
    configured_path: &str,
) -> Option<PathBuf> {
    let trimmed = configured_path.trim();
    let configured = normalize_relative_path(trimmed)?;
    let with_root = package_dir.join(&configured);
    let is_directory_hint = trimmed.ends_with('/') || trimmed.ends_with('\\');
    let explicit_skill_file = configured.file_name().is_some_and(|name| {
        name.to_string_lossy().eq_ignore_ascii_case(SKILL_FILE_NAME)
            || name.to_string_lossy().to_ascii_lowercase().ends_with(".md")
    });

    if is_directory_hint || !explicit_skill_file {
        Some(with_root.join(SKILL_FILE_NAME))
    } else {
        Some(with_root)
    }
}

pub(crate) fn find_authored_skill_dir(
    package_dir: &Path,
    package_name: Option<&str>,
) -> Option<PathBuf> {
    if let Some(package_name) = package_name {
        let expected_dir = expected_authored_skill_dir(package_dir, package_name);
        if expected_dir.join(SKILL_FILE_NAME).is_file() {
            return Some(expected_dir);
        }
    }

    let mut candidates = find_all_authored_skill_dirs(package_dir);
    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

pub(crate) fn find_all_authored_skill_dirs(package_dir: &Path) -> Vec<PathBuf> {
    let skill_root = package_dir.join(AGENTS_SKILL_ROOT_RELATIVE_PATH);
    let Ok(entries) = fs::read_dir(&skill_root) else {
        return Vec::new();
    };

    let mut skill_dirs = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!(
                    "Failed to read entry in authored skill directory {}: {}",
                    skill_root.display(),
                    error
                );
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        if path.join(SKILL_FILE_NAME).is_file() {
            skill_dirs.push(path);
        }
    }

    skill_dirs.sort();
    skill_dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn derive_skill_name_from_package_name_handles_scoped_names() {
        assert_eq!(
            derive_skill_name_from_package_name("@codemod/jest-to-vitest"),
            "jest-to-vitest"
        );
        assert_eq!(
            derive_skill_name_from_package_name("jest-to-vitest"),
            "jest-to-vitest"
        );
    }

    #[test]
    fn find_authored_skill_dir_prefers_expected_name() {
        let temp_dir = tempdir().unwrap();
        let expected = expected_authored_skill_dir(temp_dir.path(), "@codemod/example");
        fs::create_dir_all(&expected).unwrap();
        fs::write(expected.join(SKILL_FILE_NAME), "# Skill\n").unwrap();

        let found = find_authored_skill_dir(temp_dir.path(), Some("@codemod/example"));
        assert_eq!(found.as_deref(), Some(expected.as_path()));
    }

    #[test]
    fn find_authored_skill_dir_returns_none_when_multiple_ambiguous() {
        let temp_dir = tempdir().unwrap();
        let first = temp_dir
            .path()
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join("first");
        let second = temp_dir
            .path()
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join(SKILL_FILE_NAME), "# First\n").unwrap();
        fs::write(second.join(SKILL_FILE_NAME), "# Second\n").unwrap();

        let found = find_authored_skill_dir(temp_dir.path(), None);
        assert!(found.is_none());
    }

    #[test]
    fn expected_authored_skill_relative_file_uses_conventional_layout() {
        assert_eq!(
            expected_authored_skill_relative_file("@codemod/example"),
            "./agents/skill/example/SKILL.md"
        );
    }

    #[test]
    fn resolve_configured_skill_file_path_accepts_explicit_file_path() {
        let temp_dir = tempdir().unwrap();
        let path = resolve_configured_skill_file_path(temp_dir.path(), "./custom/SKILL.md")
            .expect("expected configured path");
        assert_eq!(path, temp_dir.path().join("custom/SKILL.md"));
    }

    #[test]
    fn resolve_configured_skill_file_path_supports_directory_path() {
        let temp_dir = tempdir().unwrap();
        let path = resolve_configured_skill_file_path(temp_dir.path(), "./custom")
            .expect("expected configured path");
        assert_eq!(path, temp_dir.path().join("custom").join(SKILL_FILE_NAME));
    }

    #[test]
    fn resolve_configured_skill_file_path_rejects_parent_traversal() {
        let temp_dir = tempdir().unwrap();
        assert!(
            resolve_configured_skill_file_path(temp_dir.path(), "../custom/SKILL.md").is_none()
        );
    }
}
