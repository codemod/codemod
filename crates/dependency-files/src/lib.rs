//! Shared dependency ecosystem and dependency file detection helpers.
//!
//! This crate owns filename-based ecosystem and context-file detection so the
//! workflow engine and code indexer can share the same low-risk primitives.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Lock files currently recognized by the code indexer.
pub const LOCK_FILE_PATTERNS: &[(&str, Ecosystem)] = &[
    ("package-lock.json", Ecosystem::Npm),
    ("yarn.lock", Ecosystem::Npm),
    ("pnpm-lock.yaml", Ecosystem::Npm),
    ("bun.lock", Ecosystem::Npm),
    // bun.lockb is a binary format and cannot be parsed as JSON; only bun.lock
    // (text) is currently supported by the code indexer.
    ("poetry.lock", Ecosystem::PyPI),
    ("Pipfile.lock", Ecosystem::PyPI),
    ("requirements.txt", Ecosystem::PyPI),
    ("Cargo.lock", Ecosystem::Cargo),
    ("go.mod", Ecosystem::Go),
    ("go.sum", Ecosystem::Go),
    ("Gemfile.lock", Ecosystem::RubyGems),
];

/// Context files currently fetched alongside lock files by the code indexer,
/// plus project files needed for package-manager/root detection.
pub const CONTEXT_FILE_PATTERNS: &[(&str, Ecosystem)] = &[
    ("package.json", Ecosystem::Npm),
    ("Cargo.toml", Ecosystem::Cargo),
    ("pyproject.toml", Ecosystem::PyPI),
    ("pom.xml", Ecosystem::Java),
    ("build.gradle", Ecosystem::Java),
    ("build.gradle.kts", Ecosystem::Java),
    ("settings.gradle", Ecosystem::Java),
    ("settings.gradle.kts", Ecosystem::Java),
    ("gradle.lockfile", Ecosystem::Java),
];

/// Project files that identify a Java package-manager root.
pub const JAVA_PROJECT_FILE_NAMES: &[&str] = &[
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "gradle.lockfile",
];

/// Ecosystem identifier shared by dependency file consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ecosystem {
    #[serde(rename = "npm")]
    Npm,
    #[serde(rename = "pypi")]
    PyPI,
    #[serde(rename = "cargo")]
    Cargo,
    #[serde(rename = "go")]
    Go,
    #[serde(rename = "rubygems")]
    RubyGems,
    #[serde(rename = "java")]
    Java,
}

impl Ecosystem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::PyPI => "pypi",
            Self::Cargo => "cargo",
            Self::Go => "go",
            Self::RubyGems => "rubygems",
            Self::Java => "java",
        }
    }
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

pub fn is_context_file(path: &str) -> bool {
    detect_context_file(path).is_some()
}

pub fn detect_context_file(path: &str) -> Option<Ecosystem> {
    let filename = file_name(path);
    CONTEXT_FILE_PATTERNS
        .iter()
        .find(|(pattern, _)| filename == *pattern)
        .map(|(_, ecosystem)| *ecosystem)
}

pub fn is_java_project_file(path: &str) -> bool {
    let filename = file_name(path);
    JAVA_PROJECT_FILE_NAMES.contains(&filename)
}

pub fn detect_lock_file(path: &str) -> Option<Ecosystem> {
    let filename = file_name(path);
    LOCK_FILE_PATTERNS
        .iter()
        .find(|(pattern, _)| filename == *pattern)
        .map(|(_, ecosystem)| *ecosystem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_current_lock_file_patterns() {
        let cases = [
            ("package-lock.json", Ecosystem::Npm),
            ("apps/web/yarn.lock", Ecosystem::Npm),
            (r"apps\web\yarn.lock", Ecosystem::Npm),
            ("apps/web/pnpm-lock.yaml", Ecosystem::Npm),
            ("apps/web/bun.lock", Ecosystem::Npm),
            ("poetry.lock", Ecosystem::PyPI),
            ("services/api/Pipfile.lock", Ecosystem::PyPI),
            ("requirements.txt", Ecosystem::PyPI),
            ("Cargo.lock", Ecosystem::Cargo),
            ("crates/core/Cargo.lock", Ecosystem::Cargo),
            ("go.mod", Ecosystem::Go),
            ("go.sum", Ecosystem::Go),
            ("Gemfile.lock", Ecosystem::RubyGems),
        ];

        for (path, expected) in cases {
            assert_eq!(detect_lock_file(path), Some(expected), "{path}");
        }
    }

    #[test]
    fn ignores_unsupported_lock_file_patterns() {
        assert_eq!(detect_lock_file("bun.lockb"), None);
        assert_eq!(detect_lock_file("package.json"), None);
        assert_eq!(detect_lock_file("src/main.rs"), None);
        assert_eq!(detect_lock_file("Package-lock.json"), None);
    }

    #[test]
    fn detects_current_context_file_patterns() {
        let cases = [
            ("package.json", Ecosystem::Npm),
            ("packages/web/package.json", Ecosystem::Npm),
            (r"packages\web\package.json", Ecosystem::Npm),
            ("Cargo.toml", Ecosystem::Cargo),
            ("crates/core/Cargo.toml", Ecosystem::Cargo),
            ("pyproject.toml", Ecosystem::PyPI),
            ("services/api/pyproject.toml", Ecosystem::PyPI),
            ("pom.xml", Ecosystem::Java),
            ("services/api/build.gradle", Ecosystem::Java),
            ("services/api/build.gradle.kts", Ecosystem::Java),
            ("services/api/settings.gradle", Ecosystem::Java),
            ("services/api/settings.gradle.kts", Ecosystem::Java),
            ("services/api/gradle.lockfile", Ecosystem::Java),
        ];

        for (path, expected) in cases {
            assert!(is_context_file(path), "{path}");
            assert_eq!(detect_context_file(path), Some(expected), "{path}");
        }
    }

    #[test]
    fn ignores_non_context_files() {
        assert!(!is_context_file("Cargo.lock"));
        assert!(!is_context_file("package-lock.json"));
        assert!(!is_context_file("src/main.rs"));
        assert!(!is_context_file("PyProject.toml"));
        assert_eq!(detect_context_file("package-lock.json"), None);
    }

    #[test]
    fn detects_java_project_file_patterns() {
        assert!(is_java_project_file("pom.xml"));
        assert!(is_java_project_file("services/api/pom.xml"));
        assert!(is_java_project_file("build.gradle"));
        assert!(is_java_project_file("services/api/build.gradle.kts"));
        assert!(is_java_project_file("settings.gradle"));
        assert!(is_java_project_file("services/api/settings.gradle.kts"));
        assert!(is_java_project_file("gradle.lockfile"));
        assert!(!is_java_project_file("package.json"));
        assert!(!is_java_project_file("Cargo.toml"));
    }

    #[test]
    fn renders_ecosystem_names_unchanged() {
        assert_eq!(Ecosystem::Npm.as_str(), "npm");
        assert_eq!(Ecosystem::PyPI.as_str(), "pypi");
        assert_eq!(Ecosystem::Cargo.as_str(), "cargo");
        assert_eq!(Ecosystem::Go.as_str(), "go");
        assert_eq!(Ecosystem::RubyGems.as_str(), "rubygems");
        assert_eq!(Ecosystem::Java.as_str(), "java");
        assert_eq!(Ecosystem::RubyGems.to_string(), "rubygems");
    }
}
