use std::collections::HashMap;
use std::path::{Path, PathBuf};

use butterflow_models::step::{BumpDependencySpec, PackageManager, UseBumpDependency};
use butterflow_models::Error;
use butterflow_runners::{OutputCallback, Runner};

use crate::package_manager_detection::{
    infer_package_manager_root, PackageManagerDetectionError, PackageManagerInferenceRequest,
    PackageManagerRoot,
};
use crate::workflow_facts::{DependencyFact, WorkflowFacts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BumpDependencyMode {
    ConditionalRemediation,
    Ensure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BumpDependencyPlan {
    pub manager_root: PackageManagerRoot,
    pub actions: Vec<BumpDependencyAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BumpDependencyAction {
    pub dependency: String,
    pub current_version: String,
    pub target: String,
    pub manifest_path: String,
    pub dependency_type: Option<String>,
    pub mode: BumpDependencyMode,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BumpDependencyCommand {
    pub manager: PackageManager,
    pub working_dir: PathBuf,
    pub command: String,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BumpDependencyExecution {
    pub commands: Vec<BumpDependencyCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BumpDependencyPlanError {
    #[error("failed to infer package manager root: {0}")]
    PackageManagerDetection(#[from] PackageManagerDetectionError),

    #[error("dependency {name} was not found for manager {manager} at root {root}")]
    DependencyNotFound {
        name: String,
        manager: PackageManager,
        root: PathBuf,
    },

    #[error(
        "multiple dependency facts matched {name} for manager {manager} at root {root}: {paths:?}"
    )]
    AmbiguousDependency {
        name: String,
        manager: PackageManager,
        root: PathBuf,
        paths: Vec<String>,
    },

    #[error(
        "dependency {name} version {current_version} does not match if_version {required_version}"
    )]
    IfVersionMismatch {
        name: String,
        current_version: String,
        required_version: String,
    },

    #[error("unsupported version requirement {requirement} for dependency {name}")]
    UnsupportedVersionRequirement { name: String, requirement: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BumpDependencyExecutionError {
    #[error("package manager {manager} is not supported for command execution yet")]
    UnsupportedPackageManagerCommand { manager: PackageManager },

    #[error("package-manager command failed: {command}")]
    CommandFailed {
        command: String,
        exit_code: i32,
        output: String,
    },

    #[error("package-manager command failed: {0}")]
    Runtime(String),
}

pub fn plan_bump_dependency_step(
    facts: &WorkflowFacts,
    step: &UseBumpDependency,
    dry_run: bool,
) -> Result<BumpDependencyPlan, BumpDependencyPlanError> {
    let root = step.root.as_ref().map(|root| {
        let trimmed = root.trim();
        if trimmed == "." {
            PathBuf::from(".")
        } else {
            PathBuf::from(trimmed)
        }
    });
    let manager_root = infer_package_manager_root(
        facts,
        &PackageManagerInferenceRequest {
            manager: step.manager,
            root,
            ..PackageManagerInferenceRequest::default()
        },
    )?;

    let mut actions = Vec::new();
    for dependency in &step.dependencies {
        if let Some(action) = plan_dependency_action(facts, &manager_root, dependency, dry_run)? {
            actions.push(action);
        }
    }

    Ok(BumpDependencyPlan {
        manager_root,
        actions,
    })
}

pub async fn execute_bump_dependency_plan(
    runner: &dyn Runner,
    plan: &BumpDependencyPlan,
    target_path: &Path,
    env: &HashMap<String, String>,
    output_callback: Option<OutputCallback>,
) -> Result<BumpDependencyExecution, BumpDependencyExecutionError> {
    let mut commands = Vec::new();

    for action in &plan.actions {
        let command = command_for_action(&plan.manager_root, action, target_path)?;
        if !command.dry_run {
            runner
                .run_command(&command.command, env, output_callback.clone())
                .await
                .map_err(|error| match error {
                    Error::ShellCommandFailed { exit_code, output } => {
                        BumpDependencyExecutionError::CommandFailed {
                            command: command.command.clone(),
                            exit_code,
                            output,
                        }
                    }
                    error => BumpDependencyExecutionError::Runtime(error.to_string()),
                })?;
        }
        commands.push(command);
    }

    Ok(BumpDependencyExecution { commands })
}

pub fn command_for_action(
    manager_root: &PackageManagerRoot,
    action: &BumpDependencyAction,
    target_path: &Path,
) -> Result<BumpDependencyCommand, BumpDependencyExecutionError> {
    let working_dir = if manager_root.root == Path::new(".") {
        target_path.to_path_buf()
    } else {
        target_path.join(&manager_root.root)
    };
    let package = package_with_target(manager_root.manager, &action.dependency, &action.target);
    let mut args = match manager_root.manager {
        PackageManager::Npm => vec!["install".to_string(), package],
        PackageManager::Yarn | PackageManager::Pnpm | PackageManager::Bun => {
            vec!["add".to_string(), package]
        }
        PackageManager::Cargo => vec!["add".to_string(), package],
        PackageManager::Go => vec!["get".to_string(), package],
        PackageManager::Pip => vec!["install".to_string(), package],
        PackageManager::Poetry => vec!["add".to_string(), package],
        PackageManager::Pipenv => vec!["install".to_string(), package],
        PackageManager::Bundler => vec![
            "add".to_string(),
            action.dependency.clone(),
            "--version".to_string(),
            action.target.clone(),
        ],
        PackageManager::Maven | PackageManager::Gradle => {
            return Err(
                BumpDependencyExecutionError::UnsupportedPackageManagerCommand {
                    manager: manager_root.manager,
                },
            );
        }
    };
    args.extend(dependency_type_args(
        manager_root.manager,
        action.dependency_type.as_deref(),
    ));

    let invocation = match manager_root.manager {
        PackageManager::Pip => shell_command("python", ["-m", "pip"].into_iter(), args),
        PackageManager::Bundler => shell_command("bundle", std::iter::empty::<&str>(), args),
        manager => shell_command(manager.as_str(), std::iter::empty::<&str>(), args),
    };
    let command = format!(
        "cd {} && {}",
        shell_quote(&working_dir.to_string_lossy()),
        invocation
    );

    Ok(BumpDependencyCommand {
        manager: manager_root.manager,
        working_dir,
        command,
        dry_run: action.dry_run,
    })
}

fn plan_dependency_action(
    facts: &WorkflowFacts,
    manager_root: &PackageManagerRoot,
    dependency: &BumpDependencySpec,
    dry_run: bool,
) -> Result<Option<BumpDependencyAction>, BumpDependencyPlanError> {
    let fact = find_dependency_fact(facts, manager_root, dependency)?;

    if let Some(if_version) = dependency.if_version.as_deref().map(str::trim) {
        if !version_requirement_matches(&fact.version, if_version).map_err(|_| {
            BumpDependencyPlanError::UnsupportedVersionRequirement {
                name: dependency.name.clone(),
                requirement: if_version.to_string(),
            }
        })? {
            return Err(BumpDependencyPlanError::IfVersionMismatch {
                name: dependency.name.clone(),
                current_version: fact.version.clone(),
                required_version: if_version.to_string(),
            });
        }

        return Ok(Some(action_from_fact(
            dependency,
            fact,
            dependency
                .target
                .as_deref()
                .map(str::trim)
                .expect("bump-dependency validation should require target when if_version is used"),
            BumpDependencyMode::ConditionalRemediation,
            dry_run,
        )));
    }

    let ensure =
        dependency.ensure.as_deref().map(str::trim).expect(
            "bump-dependency validation should require exactly one of if_version or ensure",
        );
    if version_requirement_matches(&fact.version, ensure).map_err(|_| {
        BumpDependencyPlanError::UnsupportedVersionRequirement {
            name: dependency.name.clone(),
            requirement: ensure.to_string(),
        }
    })? {
        return Ok(None);
    }

    Ok(Some(action_from_fact(
        dependency,
        fact,
        dependency
            .target
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .unwrap_or(ensure),
        BumpDependencyMode::Ensure,
        dry_run,
    )))
}

fn find_dependency_fact<'a>(
    facts: &'a WorkflowFacts,
    manager_root: &PackageManagerRoot,
    dependency: &BumpDependencySpec,
) -> Result<&'a DependencyFact, BumpDependencyPlanError> {
    let mut matches = facts
        .dependencies
        .iter()
        .filter(|fact| {
            fact.ecosystem == manager_root.ecosystem
                && fact.name == dependency.name
                && dependency_path_is_in_root(&fact.path, &manager_root.root)
        })
        .collect::<Vec<_>>();

    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(BumpDependencyPlanError::DependencyNotFound {
            name: dependency.name.clone(),
            manager: manager_root.manager,
            root: manager_root.root.clone(),
        }),
        _ => Err(BumpDependencyPlanError::AmbiguousDependency {
            name: dependency.name.clone(),
            manager: manager_root.manager,
            root: manager_root.root.clone(),
            paths: matches.iter().map(|fact| fact.path.clone()).collect(),
        }),
    }
}

fn dependency_path_is_in_root(path: &str, root: &Path) -> bool {
    if root == Path::new(".") {
        return !path.contains('/');
    }

    let Ok(relative) = Path::new(path).strip_prefix(root) else {
        return false;
    };
    relative.components().count() == 1
}

fn action_from_fact(
    dependency: &BumpDependencySpec,
    fact: &DependencyFact,
    target: &str,
    mode: BumpDependencyMode,
    dry_run: bool,
) -> BumpDependencyAction {
    BumpDependencyAction {
        dependency: dependency.name.clone(),
        current_version: fact.version.clone(),
        target: target.to_string(),
        manifest_path: fact.path.clone(),
        dependency_type: fact.dependency_type.clone(),
        mode,
        dry_run,
    }
}

fn package_with_target(manager: PackageManager, name: &str, target: &str) -> String {
    match manager {
        PackageManager::Pip | PackageManager::Pipenv => {
            if target.starts_with(['<', '>', '=', '!', '~']) {
                format!("{name}{target}")
            } else {
                format!("{name}=={target}")
            }
        }
        _ => format!("{name}@{target}"),
    }
}

fn dependency_type_args(manager: PackageManager, dependency_type: Option<&str>) -> Vec<String> {
    match (manager, dependency_type) {
        (
            PackageManager::Npm | PackageManager::Pnpm,
            Some("devDependencies" | "dev-dependencies"),
        ) => vec!["--save-dev".to_string()],
        (PackageManager::Yarn | PackageManager::Bun, Some("devDependencies")) => {
            vec!["--dev".to_string()]
        }
        (PackageManager::Cargo, Some("dev-dependencies")) => vec!["--dev".to_string()],
        (PackageManager::Cargo, Some("build-dependencies")) => vec!["--build".to_string()],
        (PackageManager::Poetry, Some("devDependencies" | "dev-dependencies")) => {
            vec!["--group".to_string(), "dev".to_string()]
        }
        (PackageManager::Pipenv, Some("devDependencies" | "dev-dependencies")) => {
            vec!["--dev".to_string()]
        }
        _ => Vec::new(),
    }
}

fn shell_command<I, S>(program: &str, prefix_args: I, args: Vec<String>) -> String
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    std::iter::once(program.to_string())
        .chain(prefix_args.map(|arg| arg.as_ref().to_string()))
        .chain(args)
        .map(|part| shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn version_requirement_matches(
    current_version: &str,
    requirement: &str,
) -> Result<bool, VersionRequirementError> {
    let requirement = requirement.trim();
    if requirement == "*" || requirement.eq_ignore_ascii_case("any") {
        return Ok(true);
    }

    if normalize_version(current_version) == normalize_version(requirement) {
        return Ok(true);
    }

    let Some(current) = SemverLike::parse(current_version) else {
        return Ok(false);
    };

    if let Some(required) = requirement.strip_prefix('^') {
        let Some(required) = SemverLike::parse(required) else {
            return Err(VersionRequirementError);
        };
        return Ok(current.major == required.major && current >= required);
    }

    if let Some(required) = requirement.strip_prefix('~') {
        let Some(required) = SemverLike::parse(required) else {
            return Err(VersionRequirementError);
        };
        return Ok(current.major == required.major
            && current.minor == required.minor
            && current >= required);
    }

    let comparators = requirement
        .split_whitespace()
        .map(Comparator::parse)
        .collect::<Option<Vec<_>>>()
        .ok_or(VersionRequirementError)?;
    if comparators.is_empty() {
        return Err(VersionRequirementError);
    }

    Ok(comparators
        .iter()
        .all(|comparator| comparator.matches(current)))
}

fn normalize_version(version: &str) -> &str {
    version
        .trim()
        .trim_start_matches(['^', '~', '=', 'v', 'V'])
        .trim()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemverLike {
    major: u64,
    minor: u64,
    patch: u64,
}

impl SemverLike {
    fn parse(value: &str) -> Option<Self> {
        let value = normalize_version(value);
        let value = value.split(['-', '+']).next().unwrap_or(value);
        let mut parts = value.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comparator {
    Equal(SemverLike),
    GreaterThan(SemverLike),
    GreaterThanOrEqual(SemverLike),
    LessThan(SemverLike),
    LessThanOrEqual(SemverLike),
}

impl Comparator {
    fn parse(value: &str) -> Option<Self> {
        let (operator, version) = if let Some(version) = value.strip_prefix(">=") {
            (">=", version)
        } else if let Some(version) = value.strip_prefix("<=") {
            ("<=", version)
        } else if let Some(version) = value.strip_prefix('>') {
            (">", version)
        } else if let Some(version) = value.strip_prefix('<') {
            ("<", version)
        } else if let Some(version) = value.strip_prefix('=') {
            ("=", version)
        } else {
            ("=", value)
        };

        let version = SemverLike::parse(version)?;
        Some(match operator {
            ">=" => Self::GreaterThanOrEqual(version),
            "<=" => Self::LessThanOrEqual(version),
            ">" => Self::GreaterThan(version),
            "<" => Self::LessThan(version),
            "=" => Self::Equal(version),
            _ => return None,
        })
    }

    fn matches(self, current: SemverLike) -> bool {
        match self {
            Self::Equal(version) => current == version,
            Self::GreaterThan(version) => current > version,
            Self::GreaterThanOrEqual(version) => current >= version,
            Self::LessThan(version) => current < version,
            Self::LessThanOrEqual(version) => current <= version,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VersionRequirementError;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_facts::{DependencyFact, EcosystemFact, EcosystemFactSource};
    use async_trait::async_trait;
    use butterflow_models::Result;
    use dependency_files::Ecosystem;
    use std::sync::{Arc, Mutex};

    fn npm_facts(root: &str, name: &str, version: &str) -> WorkflowFacts {
        let package_path = if root == "." {
            "package.json".to_string()
        } else {
            format!("{root}/package.json")
        };
        let lock_path = if root == "." {
            "package-lock.json".to_string()
        } else {
            format!("{root}/pnpm-lock.yaml")
        };

        WorkflowFacts {
            schema_version: 1,
            ecosystems: vec![
                EcosystemFact {
                    ecosystem: Ecosystem::Npm,
                    source: EcosystemFactSource::ContextFile,
                    path: package_path.clone(),
                },
                EcosystemFact {
                    ecosystem: Ecosystem::Npm,
                    source: EcosystemFactSource::LockFile,
                    path: lock_path,
                },
            ],
            dependencies: vec![DependencyFact {
                ecosystem: Ecosystem::Npm,
                name: name.to_string(),
                version: version.to_string(),
                path: package_path,
                dependency_type: Some("dependencies".to_string()),
            }],
        }
    }

    fn dependency_with_if_version(
        name: &str,
        if_version: &str,
        target: &str,
    ) -> BumpDependencySpec {
        BumpDependencySpec {
            name: name.to_string(),
            target: Some(target.to_string()),
            if_version: Some(if_version.to_string()),
            ensure: None,
        }
    }

    fn dependency_with_ensure(
        name: &str,
        ensure: &str,
        target: Option<&str>,
    ) -> BumpDependencySpec {
        BumpDependencySpec {
            name: name.to_string(),
            target: target.map(str::to_string),
            if_version: None,
            ensure: Some(ensure.to_string()),
        }
    }

    #[derive(Default)]
    struct RecordingRunner {
        commands: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Runner for RecordingRunner {
        async fn run_command(
            &self,
            command: &str,
            _env: &HashMap<String, String>,
            _output_callback: Option<OutputCallback>,
        ) -> Result<String> {
            self.commands.lock().unwrap().push(command.to_string());
            Ok(String::new())
        }
    }

    #[test]
    fn plans_if_version_remediation() {
        let facts = npm_facts("apps/web", "react", "^17.0.2");
        let plan = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Pnpm),
                root: Some("apps/web".to_string()),
                dependencies: vec![dependency_with_if_version("react", "^17.0.0", "^18.0.0")],
            },
            true,
        )
        .unwrap();

        assert_eq!(plan.manager_root.manager, PackageManager::Pnpm);
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].dependency, "react");
        assert_eq!(
            plan.actions[0].mode,
            BumpDependencyMode::ConditionalRemediation
        );
        assert_eq!(plan.actions[0].target, "^18.0.0");
        assert!(plan.actions[0].dry_run);
    }

    #[test]
    fn wildcard_if_version_matches_existing_dependency() {
        let facts = npm_facts(".", "react", "workspace:*");
        let plan = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: Some(".".to_string()),
                dependencies: vec![dependency_with_if_version("react", "any", "^18.0.0")],
            },
            false,
        )
        .unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(!plan.actions[0].dry_run);
    }

    #[test]
    fn if_version_mismatch_fails_fast() {
        let facts = npm_facts(".", "react", "^18.2.0");
        let error = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: Some(".".to_string()),
                dependencies: vec![
                    dependency_with_if_version("react", "^17.0.0", "^18.0.0"),
                    dependency_with_if_version("vite", "*", "^6.0.0"),
                ],
            },
            false,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            BumpDependencyPlanError::IfVersionMismatch { .. }
        ));
    }

    #[test]
    fn ensure_skips_when_current_version_satisfies_requirement() {
        let facts = npm_facts(".", "react", "^18.2.0");
        let plan = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: Some(".".to_string()),
                dependencies: vec![dependency_with_ensure("react", "^18.0.0", None)],
            },
            false,
        )
        .unwrap();

        assert!(plan.actions.is_empty());
    }

    #[test]
    fn ensure_without_target_uses_ensure_requirement_as_target() {
        let facts = npm_facts(".", "react", "^17.0.2");
        let plan = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: Some(".".to_string()),
                dependencies: vec![dependency_with_ensure("react", "^18.0.0", None)],
            },
            false,
        )
        .unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].mode, BumpDependencyMode::Ensure);
        assert_eq!(plan.actions[0].target, "^18.0.0");
    }

    #[test]
    fn ensure_target_can_override_requirement() {
        let facts = npm_facts(".", "react", "^17.0.2");
        let plan = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: Some(".".to_string()),
                dependencies: vec![dependency_with_ensure("react", ">=18.0.0", Some("^18.2.0"))],
            },
            false,
        )
        .unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].target, "^18.2.0");
    }

    #[test]
    fn missing_dependency_fails() {
        let facts = npm_facts(".", "react", "^17.0.2");
        let error = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: Some(".".to_string()),
                dependencies: vec![dependency_with_if_version("vite", "*", "^6.0.0")],
            },
            false,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            BumpDependencyPlanError::DependencyNotFound { .. }
        ));
    }

    #[test]
    fn ambiguous_root_fails_before_matching_dependencies() {
        let facts = WorkflowFacts {
            schema_version: 1,
            ecosystems: vec![
                EcosystemFact {
                    ecosystem: Ecosystem::Npm,
                    source: EcosystemFactSource::LockFile,
                    path: "apps/web/package-lock.json".to_string(),
                },
                EcosystemFact {
                    ecosystem: Ecosystem::Npm,
                    source: EcosystemFactSource::LockFile,
                    path: "apps/admin/package-lock.json".to_string(),
                },
            ],
            dependencies: Vec::new(),
        };

        let error = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: None,
                dependencies: vec![dependency_with_if_version("react", "*", "^18.0.0")],
            },
            false,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            BumpDependencyPlanError::PackageManagerDetection(
                PackageManagerDetectionError::AmbiguousRoot { .. }
            )
        ));
    }

    #[test]
    fn unsupported_requirement_fails() {
        let facts = npm_facts(".", "react", "^18.2.0");
        let error = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Npm),
                root: Some(".".to_string()),
                dependencies: vec![dependency_with_ensure(
                    "react",
                    ">=18 || ^19",
                    Some("^19.0.0"),
                )],
            },
            false,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            BumpDependencyPlanError::UnsupportedVersionRequirement { .. }
        ));
    }

    #[test]
    fn generates_package_manager_command_at_detected_root() {
        let command = command_for_action(
            &PackageManagerRoot {
                ecosystem: Ecosystem::Npm,
                manager: PackageManager::Pnpm,
                root: PathBuf::from("apps/web"),
                evidence_path: "apps/web/pnpm-lock.yaml".to_string(),
            },
            &BumpDependencyAction {
                dependency: "react".to_string(),
                current_version: "^17.0.0".to_string(),
                target: "^18.0.0".to_string(),
                manifest_path: "apps/web/package.json".to_string(),
                dependency_type: Some("devDependencies".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            },
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(command.manager, PackageManager::Pnpm);
        assert_eq!(command.working_dir, PathBuf::from("/repo/apps/web"));
        assert_eq!(
            command.command,
            "cd '/repo/apps/web' && 'pnpm' 'add' 'react@^18.0.0' '--save-dev'"
        );
    }

    #[test]
    fn generates_python_package_spec() {
        let command = command_for_action(
            &PackageManagerRoot {
                ecosystem: Ecosystem::PyPI,
                manager: PackageManager::Pip,
                root: PathBuf::from("."),
                evidence_path: "requirements.txt".to_string(),
            },
            &BumpDependencyAction {
                dependency: "requests".to_string(),
                current_version: "requests>=2.31.0".to_string(),
                target: ">=2.32.0".to_string(),
                manifest_path: "requirements.txt".to_string(),
                dependency_type: Some("requirements".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            },
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(
            command.command,
            "cd '/repo' && 'python' '-m' 'pip' 'install' 'requests>=2.32.0'"
        );
    }

    #[test]
    fn reports_unsupported_package_manager_command() {
        let error = command_for_action(
            &PackageManagerRoot {
                ecosystem: Ecosystem::Java,
                manager: PackageManager::Maven,
                root: PathBuf::from("."),
                evidence_path: "pom.xml".to_string(),
            },
            &BumpDependencyAction {
                dependency: "org.example:library".to_string(),
                current_version: "1.0.0".to_string(),
                target: "2.0.0".to_string(),
                manifest_path: "pom.xml".to_string(),
                dependency_type: None,
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            },
            Path::new("/repo"),
        )
        .unwrap_err();

        assert_eq!(
            error,
            BumpDependencyExecutionError::UnsupportedPackageManagerCommand {
                manager: PackageManager::Maven,
            }
        );
    }

    #[tokio::test]
    async fn dry_run_execution_returns_commands_without_running_them() {
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::Npm,
                manager: PackageManager::Npm,
                root: PathBuf::from("."),
                evidence_path: "package-lock.json".to_string(),
            },
            actions: vec![BumpDependencyAction {
                dependency: "react".to_string(),
                current_version: "^17.0.0".to_string(),
                target: "^18.0.0".to_string(),
                manifest_path: "package.json".to_string(),
                dependency_type: Some("dependencies".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: true,
            }],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, Path::new("/repo"), &HashMap::new(), None)
                .await
                .unwrap();

        assert_eq!(execution.commands.len(), 1);
        assert_eq!(runner.commands.lock().unwrap().len(), 0);
    }
}
