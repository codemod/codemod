use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use butterflow_models::step::PackageManager;
use dependency_files::Ecosystem;

use crate::workflow_facts::{EcosystemFactSource, WorkflowFacts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManagerRoot {
    pub ecosystem: Ecosystem,
    pub manager: PackageManager,
    pub root: PathBuf,
    pub evidence_path: String,
}

pub const fn package_manager_ecosystem(manager: PackageManager) -> Ecosystem {
    match manager {
        PackageManager::Npm | PackageManager::Yarn | PackageManager::Pnpm | PackageManager::Bun => {
            Ecosystem::Npm
        }
        PackageManager::Cargo => Ecosystem::Cargo,
        PackageManager::Go => Ecosystem::Go,
        PackageManager::RequirementsTxt
        | PackageManager::Uv
        | PackageManager::Poetry
        | PackageManager::Pipenv => Ecosystem::PyPI,
        PackageManager::Bundler => Ecosystem::RubyGems,
        PackageManager::Maven | PackageManager::Gradle => Ecosystem::Java,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageManagerInferenceRequest {
    /// Optional ecosystem constraint supplied by the caller or step config.
    pub ecosystem: Option<Ecosystem>,
    /// Optional package-manager constraint supplied by the caller or step config.
    pub manager: Option<PackageManager>,
    /// Optional author-provided package root used to disambiguate monorepos.
    /// Wildcard roots such as "*" are authoring syntax and should be expanded
    /// before calling this single-root inference API.
    pub root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageManagerDetectionError {
    #[error("no package-manager root was detected")]
    MissingRoot,

    #[error("no package-manager root was detected for ecosystem {ecosystem:?}")]
    MissingEcosystemRoot { ecosystem: Ecosystem },

    #[error("no package-manager root was detected for manager {manager}")]
    MissingManagerRoot { manager: PackageManager },

    #[error("manager {manager} is not valid for ecosystem {ecosystem:?}")]
    ManagerEcosystemMismatch {
        manager: PackageManager,
        ecosystem: Ecosystem,
    },

    #[error("multiple package-manager roots match: {candidates:?}")]
    AmbiguousRoot { candidates: Vec<PackageManagerRoot> },
}

pub fn infer_package_manager_root(
    facts: &WorkflowFacts,
    request: &PackageManagerInferenceRequest,
) -> Result<PackageManagerRoot, PackageManagerDetectionError> {
    if let (Some(ecosystem), Some(manager)) = (request.ecosystem, request.manager) {
        if package_manager_ecosystem(manager) != ecosystem {
            return Err(PackageManagerDetectionError::ManagerEcosystemMismatch {
                manager,
                ecosystem,
            });
        }
    }

    let mut candidates = detect_package_manager_roots(facts);

    if let Some(ecosystem) = request.ecosystem {
        candidates.retain(|candidate| candidate.ecosystem == ecosystem);
    }
    if let Some(manager) = request.manager {
        candidates.retain(|candidate| candidate.manager == manager);
    }
    if let Some(root) = &request.root {
        candidates.retain(|candidate| candidate.root == *root);
    }

    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => {
            if let Some(manager) = request.manager {
                Err(PackageManagerDetectionError::MissingManagerRoot { manager })
            } else if let Some(ecosystem) = request.ecosystem {
                Err(PackageManagerDetectionError::MissingEcosystemRoot { ecosystem })
            } else {
                Err(PackageManagerDetectionError::MissingRoot)
            }
        }
        _ => Err(PackageManagerDetectionError::AmbiguousRoot { candidates }),
    }
}

pub fn detect_package_manager_roots(facts: &WorkflowFacts) -> Vec<PackageManagerRoot> {
    let mut roots: BTreeMap<(PathBuf, PackageManager), PackageManagerRoot> = BTreeMap::new();
    let javascript_lock_roots = facts
        .ecosystems
        .iter()
        .filter(|fact| {
            fact.source == EcosystemFactSource::LockFile
                && matches!(
                    dependency_files::file_name(&fact.path),
                    "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "bun.lock"
                )
        })
        .map(|fact| package_root(&fact.path))
        .collect::<BTreeSet<_>>();
    let uv_roots = facts
        .ecosystems
        .iter()
        .filter(|fact| {
            fact.source == EcosystemFactSource::LockFile
                && dependency_files::file_name(&fact.path) == "uv.lock"
        })
        .map(|fact| package_root(&fact.path))
        .collect::<BTreeSet<_>>();

    for fact in &facts.ecosystems {
        let root = package_root(&fact.path);
        if fact.source == EcosystemFactSource::ContextFile
            && dependency_files::file_name(&fact.path) == "package.json"
            && javascript_lock_roots.contains(&root)
        {
            continue;
        }
        if fact.source == EcosystemFactSource::ContextFile
            && dependency_files::file_name(&fact.path) == "pyproject.toml"
            && uv_roots.contains(&root)
        {
            continue;
        }
        let Some(manager) = manager_from_fact(&fact.path, fact.source) else {
            continue;
        };
        roots
            .entry((root.clone(), manager))
            .or_insert_with(|| PackageManagerRoot {
                ecosystem: package_manager_ecosystem(manager),
                manager,
                root,
                evidence_path: fact.path.clone(),
            });
    }

    roots.into_values().collect()
}

fn manager_from_fact(path: &str, source: EcosystemFactSource) -> Option<PackageManager> {
    let filename = dependency_files::file_name(path);
    match (filename, source) {
        ("package-lock.json", EcosystemFactSource::LockFile) => Some(PackageManager::Npm),
        ("yarn.lock", EcosystemFactSource::LockFile) => Some(PackageManager::Yarn),
        ("pnpm-lock.yaml", EcosystemFactSource::LockFile) => Some(PackageManager::Pnpm),
        ("bun.lock", EcosystemFactSource::LockFile) => Some(PackageManager::Bun),
        ("package.json", EcosystemFactSource::ContextFile) => Some(PackageManager::Npm),
        ("Cargo.lock", EcosystemFactSource::LockFile)
        | ("Cargo.toml", EcosystemFactSource::ContextFile) => Some(PackageManager::Cargo),
        ("go.mod", EcosystemFactSource::LockFile) | ("go.sum", EcosystemFactSource::LockFile) => {
            Some(PackageManager::Go)
        }
        ("requirements.txt", EcosystemFactSource::LockFile) => {
            Some(PackageManager::RequirementsTxt)
        }
        ("uv.lock", EcosystemFactSource::LockFile) => Some(PackageManager::Uv),
        ("poetry.lock", EcosystemFactSource::LockFile)
        | ("pyproject.toml", EcosystemFactSource::ContextFile) => Some(PackageManager::Poetry),
        ("Pipfile.lock", EcosystemFactSource::LockFile) => Some(PackageManager::Pipenv),
        ("Gemfile.lock", EcosystemFactSource::LockFile) => Some(PackageManager::Bundler),
        ("pom.xml", EcosystemFactSource::ContextFile) => Some(PackageManager::Maven),
        ("build.gradle", EcosystemFactSource::ContextFile)
        | ("build.gradle.kts", EcosystemFactSource::ContextFile)
        | ("settings.gradle", EcosystemFactSource::ContextFile)
        | ("settings.gradle.kts", EcosystemFactSource::ContextFile)
        | ("gradle.lockfile", EcosystemFactSource::ContextFile) => Some(PackageManager::Gradle),
        _ => None,
    }
}

fn package_root(path: &str) -> PathBuf {
    let parent = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
    if parent.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        parent.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_facts::{EcosystemFact, WorkflowFacts};

    #[test]
    fn detects_npm_lockfile_root() {
        let facts = facts_with_paths(&[(
            Ecosystem::Npm,
            EcosystemFactSource::LockFile,
            "apps/web/pnpm-lock.yaml",
        )]);

        let root = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                ecosystem: Some(Ecosystem::Npm),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap();

        assert_eq!(root.manager, PackageManager::Pnpm);
        assert_eq!(root.root, PathBuf::from("apps/web"));
        assert_eq!(root.evidence_path, "apps/web/pnpm-lock.yaml");
    }

    #[test]
    fn detects_java_managers_from_project_files() {
        let facts = facts_with_paths(&[
            (Ecosystem::Java, EcosystemFactSource::ContextFile, "pom.xml"),
            (
                Ecosystem::Java,
                EcosystemFactSource::ContextFile,
                "services/api/build.gradle.kts",
            ),
        ]);

        let maven = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                manager: Some(PackageManager::Maven),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap();
        let gradle = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                manager: Some(PackageManager::Gradle),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap();

        assert_eq!(maven.root, PathBuf::from("."));
        assert_eq!(gradle.root, PathBuf::from("services/api"));
    }

    #[test]
    fn detects_uv_lockfile_root() {
        let facts = facts_with_paths(&[(
            Ecosystem::PyPI,
            EcosystemFactSource::LockFile,
            "services/api/uv.lock",
        )]);

        let root = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                manager: Some(PackageManager::Uv),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap();

        assert_eq!(root.manager, PackageManager::Uv);
        assert_eq!(root.root, PathBuf::from("services/api"));
        assert_eq!(root.evidence_path, "services/api/uv.lock");
    }

    #[test]
    fn uv_lockfile_takes_precedence_over_pyproject_context() {
        let facts = facts_with_paths(&[
            (
                Ecosystem::PyPI,
                EcosystemFactSource::LockFile,
                "services/api/uv.lock",
            ),
            (
                Ecosystem::PyPI,
                EcosystemFactSource::ContextFile,
                "services/api/pyproject.toml",
            ),
        ]);

        let root = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                ecosystem: Some(Ecosystem::PyPI),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap();

        assert_eq!(root.manager, PackageManager::Uv);
        assert_eq!(root.root, PathBuf::from("services/api"));
    }

    #[test]
    fn javascript_lockfile_takes_precedence_over_package_json_context() {
        let facts = facts_with_paths(&[
            (
                Ecosystem::Npm,
                EcosystemFactSource::ContextFile,
                "apps/web/package.json",
            ),
            (
                Ecosystem::Npm,
                EcosystemFactSource::LockFile,
                "apps/web/pnpm-lock.yaml",
            ),
        ]);

        let root = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                ecosystem: Some(Ecosystem::Npm),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap();

        assert_eq!(root.manager, PackageManager::Pnpm);
        assert_eq!(root.root, PathBuf::from("apps/web"));
    }

    #[test]
    fn reports_ambiguous_roots() {
        let facts = facts_with_paths(&[
            (
                Ecosystem::Npm,
                EcosystemFactSource::LockFile,
                "apps/web/package-lock.json",
            ),
            (
                Ecosystem::Npm,
                EcosystemFactSource::LockFile,
                "apps/admin/package-lock.json",
            ),
        ]);

        let error = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                ecosystem: Some(Ecosystem::Npm),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            PackageManagerDetectionError::AmbiguousRoot { .. }
        ));
    }

    #[test]
    fn filters_by_explicit_root() {
        let facts = facts_with_paths(&[
            (
                Ecosystem::Npm,
                EcosystemFactSource::LockFile,
                "apps/web/package-lock.json",
            ),
            (
                Ecosystem::Npm,
                EcosystemFactSource::LockFile,
                "apps/admin/package-lock.json",
            ),
        ]);

        let root = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                ecosystem: Some(Ecosystem::Npm),
                root: Some(PathBuf::from("apps/admin")),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap();

        assert_eq!(root.root, PathBuf::from("apps/admin"));
    }

    #[test]
    fn rejects_manager_ecosystem_mismatch() {
        let facts = WorkflowFacts::empty();

        let error = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                ecosystem: Some(Ecosystem::Cargo),
                manager: Some(PackageManager::Pnpm),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap_err();

        assert_eq!(
            error,
            PackageManagerDetectionError::ManagerEcosystemMismatch {
                manager: PackageManager::Pnpm,
                ecosystem: Ecosystem::Cargo,
            }
        );
    }

    #[test]
    fn reports_missing_manager_root() {
        let facts = facts_with_paths(&[(
            Ecosystem::Npm,
            EcosystemFactSource::ContextFile,
            "package.json",
        )]);

        let error = infer_package_manager_root(
            &facts,
            &PackageManagerInferenceRequest {
                manager: Some(PackageManager::Cargo),
                ..PackageManagerInferenceRequest::default()
            },
        )
        .unwrap_err();

        assert_eq!(
            error,
            PackageManagerDetectionError::MissingManagerRoot {
                manager: PackageManager::Cargo,
            }
        );
    }

    fn facts_with_paths(paths: &[(Ecosystem, EcosystemFactSource, &str)]) -> WorkflowFacts {
        WorkflowFacts {
            schema_version: 1,
            ecosystems: paths
                .iter()
                .map(|(ecosystem, source, path)| EcosystemFact {
                    ecosystem: *ecosystem,
                    source: *source,
                    path: (*path).to_string(),
                })
                .collect(),
            dependencies: Vec::new(),
        }
    }
}
