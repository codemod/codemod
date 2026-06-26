use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use butterflow_models::step::{BumpDependencySpec, PackageManager, UseBumpDependency};
use butterflow_models::Error;
use butterflow_runners::{OutputCallback, Runner};

use super::utils::ast::ast_grep_root;
use super::utils::gradle::gradle_dependency_configuration_for_literal;
use super::utils::ranges::{quoted_string_content_range, replace_range};
use super::utils::xml::{
    xml_direct_child_element, xml_direct_child_text, xml_element_is_inside, xml_element_name,
    xml_element_trimmed_text_range,
};
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
pub struct BumpDependencyFileEdit {
    pub manager: PackageManager,
    pub path: PathBuf,
    pub dependency: String,
    pub target: String,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BumpDependencyExecution {
    pub commands: Vec<BumpDependencyCommand>,
    pub file_edits: Vec<BumpDependencyFileEdit>,
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

    #[error("failed to edit dependency manifest {path}: {reason}")]
    FileEditFailed { path: PathBuf, reason: String },
}

pub fn plan_bump_dependency_step(
    facts: &WorkflowFacts,
    step: &UseBumpDependency,
    dry_run: bool,
) -> Result<BumpDependencyPlan, BumpDependencyPlanError> {
    let root = step.root.as_ref().map(|root| normalize_step_root(root));
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

fn normalize_step_root(root: &str) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in Path::new(root.trim()).components() {
        match component {
            Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

pub async fn execute_bump_dependency_plan(
    runner: &dyn Runner,
    plan: &BumpDependencyPlan,
    target_path: &Path,
    env: &HashMap<String, String>,
    output_callback: Option<OutputCallback>,
) -> Result<BumpDependencyExecution, BumpDependencyExecutionError> {
    let mut commands = Vec::new();
    let mut file_edits = Vec::new();

    if uses_manifest_edit(plan.manager_root.manager) {
        for action in &plan.actions {
            let manager = plan.manager_root.manager;
            let action = action.clone();
            let target_path = target_path.to_path_buf();
            let file_edit = tokio::task::spawn_blocking(move || {
                edit_manifest_dependency(manager, &action, &target_path)
            })
            .await
            .map_err(|error| {
                BumpDependencyExecutionError::Runtime(format!("manifest edit task failed: {error}"))
            })??;
            file_edits.push(file_edit);
        }
    } else {
        for command in commands_for_actions(&plan.manager_root, &plan.actions, target_path)? {
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
    }

    Ok(BumpDependencyExecution {
        commands,
        file_edits,
    })
}

pub fn commands_for_actions(
    manager_root: &PackageManagerRoot,
    actions: &[BumpDependencyAction],
    target_path: &Path,
) -> Result<Vec<BumpDependencyCommand>, BumpDependencyExecutionError> {
    if actions.is_empty() {
        return Ok(Vec::new());
    }

    if manager_root.manager == PackageManager::Bundler {
        return actions
            .iter()
            .map(|action| command_for_action(manager_root, action, target_path))
            .collect();
    }

    let mut commands = Vec::new();
    let mut groups = Vec::<Vec<BumpDependencyAction>>::new();
    for action in actions {
        if let Some(group) = groups.iter_mut().find(|group| {
            dependency_type_args_are_compatible(manager_root.manager, &group[0], action)
        }) {
            group.push(action.clone());
        } else {
            groups.push(vec![action.clone()]);
        }
    }

    for group in groups {
        commands.push(command_for_actions(manager_root, &group, target_path)?);
    }

    Ok(commands)
}

pub fn command_for_action(
    manager_root: &PackageManagerRoot,
    action: &BumpDependencyAction,
    target_path: &Path,
) -> Result<BumpDependencyCommand, BumpDependencyExecutionError> {
    command_for_actions(manager_root, std::slice::from_ref(action), target_path)
}

fn command_for_actions(
    manager_root: &PackageManagerRoot,
    actions: &[BumpDependencyAction],
    target_path: &Path,
) -> Result<BumpDependencyCommand, BumpDependencyExecutionError> {
    let working_dir = if manager_root.root == Path::new(".") {
        target_path.to_path_buf()
    } else {
        target_path.join(&manager_root.root)
    };
    let packages = actions
        .iter()
        .map(|action| package_with_target(manager_root.manager, &action.dependency, &action.target))
        .collect::<Vec<_>>();
    let mut args = match manager_root.manager {
        PackageManager::Npm => command_args_with_packages("install", packages),
        PackageManager::Yarn | PackageManager::Pnpm | PackageManager::Bun => {
            command_args_with_packages("add", packages)
        }
        PackageManager::Cargo => command_args_with_packages("add", packages),
        PackageManager::Go => command_args_with_packages("get", packages),
        PackageManager::RequirementsTxt => {
            return Err(
                BumpDependencyExecutionError::UnsupportedPackageManagerCommand {
                    manager: manager_root.manager,
                },
            );
        }
        PackageManager::Uv => command_args_with_packages("add", packages),
        PackageManager::Poetry => command_args_with_packages("add", packages),
        PackageManager::Pipenv => command_args_with_packages("install", packages),
        PackageManager::Bundler => {
            let [action] = actions else {
                return Err(BumpDependencyExecutionError::Runtime(
                    "bundler dependency bumps must be executed one dependency at a time"
                        .to_string(),
                ));
            };
            vec![
                "add".to_string(),
                action.dependency.clone(),
                "--version".to_string(),
                action.target.clone(),
            ]
        }
        PackageManager::Maven | PackageManager::Gradle => {
            return Err(
                BumpDependencyExecutionError::UnsupportedPackageManagerCommand {
                    manager: manager_root.manager,
                },
            );
        }
    };
    args.extend(dependency_type_args_for_actions(
        manager_root.manager,
        actions,
    ));
    args.extend(ignore_scripts_args(manager_root.manager));

    let invocation = match manager_root.manager {
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
        dry_run: actions.iter().all(|action| action.dry_run),
    })
}

fn command_args_with_packages(command: &str, packages: Vec<String>) -> Vec<String> {
    std::iter::once(command.to_string())
        .chain(packages)
        .collect()
}

fn ignore_scripts_args(manager: PackageManager) -> Vec<String> {
    match manager {
        PackageManager::Npm | PackageManager::Yarn | PackageManager::Pnpm | PackageManager::Bun => {
            vec!["--ignore-scripts".to_string()]
        }
        _ => Vec::new(),
    }
}

fn dependency_type_args_for_actions(
    manager: PackageManager,
    actions: &[BumpDependencyAction],
) -> Vec<String> {
    let Some(first_dependency_type) = actions
        .first()
        .and_then(|action| action.dependency_type.as_deref())
    else {
        return Vec::new();
    };
    if actions
        .iter()
        .all(|action| action.dependency_type.as_deref() == Some(first_dependency_type))
    {
        return dependency_type_args(manager, Some(first_dependency_type));
    }
    Vec::new()
}

fn dependency_type_args_are_compatible(
    manager: PackageManager,
    left: &BumpDependencyAction,
    right: &BumpDependencyAction,
) -> bool {
    dependency_type_args(manager, left.dependency_type.as_deref())
        == dependency_type_args(manager, right.dependency_type.as_deref())
}

fn uses_manifest_edit(manager: PackageManager) -> bool {
    matches!(
        manager,
        PackageManager::RequirementsTxt | PackageManager::Maven | PackageManager::Gradle
    )
}

fn edit_manifest_dependency(
    manager: PackageManager,
    action: &BumpDependencyAction,
    target_path: &Path,
) -> Result<BumpDependencyFileEdit, BumpDependencyExecutionError> {
    let manifest_path = target_path.join(&action.manifest_path);
    let content = fs::read_to_string(&manifest_path).map_err(|error| {
        BumpDependencyExecutionError::FileEditFailed {
            path: manifest_path.clone(),
            reason: error.to_string(),
        }
    })?;
    let updated = match manager {
        PackageManager::RequirementsTxt => edit_requirements_dependency_spec(&content, action),
        PackageManager::Maven => edit_maven_dependency_version(&content, action),
        PackageManager::Gradle => edit_gradle_dependency_version(&content, action),
        _ => unreachable!("manifest edits are only used for requirements.txt, Maven, and Gradle"),
    }
    .map_err(|reason| BumpDependencyExecutionError::FileEditFailed {
        path: manifest_path.clone(),
        reason,
    })?;

    if !action.dry_run {
        fs::write(&manifest_path, updated).map_err(|error| {
            BumpDependencyExecutionError::FileEditFailed {
                path: manifest_path.clone(),
                reason: error.to_string(),
            }
        })?;
    }

    Ok(BumpDependencyFileEdit {
        manager,
        path: manifest_path,
        dependency: action.dependency.clone(),
        target: action.target.clone(),
        dry_run: action.dry_run,
    })
}

fn edit_requirements_dependency_spec(
    content: &str,
    action: &BumpDependencyAction,
) -> Result<String, String> {
    if dependency_files::file_name(&action.manifest_path) != "requirements.txt" {
        return Err(format!(
            "unsupported requirements manifest for dependency edits: {}",
            action.manifest_path
        ));
    }

    let replacement = package_with_target(
        PackageManager::RequirementsTxt,
        &action.dependency,
        &action.target,
    );
    let mut matched_range = None::<Range<usize>>;
    let mut offset = 0;

    for line in content.split_inclusive('\n') {
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        let dependency_part = line_without_newline.split('#').next().unwrap_or("");
        let trimmed_start = dependency_part.len() - dependency_part.trim_start().len();
        let trimmed_end = dependency_part.trim_end().len();
        let spec = &dependency_part[trimmed_start..trimmed_end];

        if !spec.is_empty()
            && !spec.starts_with('-')
            && requirement_dependency_name(spec)
                .is_some_and(|name| requirement_dependency_names_match(name, &action.dependency))
        {
            if matched_range.is_some() {
                return Err(format!(
                    "multiple requirements declarations matched {}",
                    action.dependency
                ));
            }
            matched_range = Some(offset + trimmed_start..offset + trimmed_end);
        }

        offset += line.len();
    }

    let Some(range) = matched_range else {
        return Err(format!(
            "could not find requirements dependency {}",
            action.dependency
        ));
    };
    replace_range(content, range, &replacement)
}

fn edit_maven_dependency_version(
    content: &str,
    action: &BumpDependencyAction,
) -> Result<String, String> {
    let Some((group_id, artifact_id)) = action.dependency.split_once(':') else {
        return Err(format!(
            "Maven dependency names must use groupId:artifactId, got {}",
            action.dependency
        ));
    };
    let root = ast_grep_root(content, "xml")?;
    let mut replacement = None::<Range<usize>>;

    for dependency in root.root().dfs().filter(|node| {
        node.kind() == "element"
            && xml_element_name(node).as_deref() == Some("dependency")
            && !xml_element_is_inside(node, &["dependencyManagement", "build"])
    }) {
        if xml_direct_child_text(&dependency, "groupId").as_deref() != Some(group_id)
            || xml_direct_child_text(&dependency, "artifactId").as_deref() != Some(artifact_id)
        {
            continue;
        }
        if replacement.is_some() {
            return Err(format!(
                "multiple Maven dependency declarations matched {}",
                action.dependency
            ));
        }
        let Some(version_element) = xml_direct_child_element(&dependency, "version") else {
            return Err(format!(
                "Maven dependency {} does not declare a direct <version>",
                action.dependency
            ));
        };
        let Some((current_version, version_range)) =
            xml_element_trimmed_text_range(&version_element)
        else {
            return Err(format!(
                "Maven dependency {} does not declare a direct text <version>",
                action.dependency
            ));
        };
        if current_version.contains("${") {
            return Err(format!(
                "Maven dependency {} uses a property-managed version",
                action.dependency
            ));
        }
        replacement = Some(version_range);
    }

    let Some(range) = replacement else {
        return Err(format!(
            "could not find direct Maven dependency {}",
            action.dependency
        ));
    };
    replace_range(content, range, &action.target)
}

fn edit_gradle_dependency_version(
    content: &str,
    action: &BumpDependencyAction,
) -> Result<String, String> {
    let Some(configuration) = action.dependency_type.as_deref() else {
        return Err(format!(
            "Gradle dependency {} is missing its dependency configuration",
            action.dependency
        ));
    };
    let manifest_file_name = dependency_files::file_name(&action.manifest_path);
    let language = match manifest_file_name {
        "build.gradle.kts" => "kotlin",
        "build.gradle" => "groovy",
        _ => {
            return Err(format!(
                "unsupported Gradle manifest for parser-backed dependency edits: {manifest_file_name}"
            ));
        }
    };

    let root = ast_grep_root(content, language)?;
    let prefix = format!("{}:", action.dependency);
    let mut replacement = None::<Range<usize>>;

    for literal in root
        .root()
        .dfs()
        .filter(|node| matches!(node.kind().as_ref(), "string_literal" | "string"))
    {
        if gradle_dependency_configuration_for_literal(&literal) != Some(configuration) {
            continue;
        }
        let text = literal.text();
        let Some((spec, range)) = quoted_string_content_range(content, literal.range()) else {
            continue;
        };
        let Some(current_version) = spec.strip_prefix(&prefix) else {
            continue;
        };
        if current_version.is_empty()
            || current_version.contains('$')
            || current_version.contains(':')
        {
            continue;
        }
        if replacement.is_some() {
            return Err(format!(
                "multiple Gradle dependency declarations matched {}",
                action.dependency
            ));
        }
        if text.contains('$') {
            return Err(format!(
                "Gradle dependency {} uses an interpolated version",
                action.dependency
            ));
        }
        replacement = Some(range);
    }

    let Some(range) = replacement else {
        return Err(format!(
            "could not find direct Gradle dependency {}",
            action.dependency
        ));
    };
    replace_range(content, range, &format!("{prefix}{}", action.target))
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
        PackageManager::RequirementsTxt | PackageManager::Uv | PackageManager::Pipenv => {
            if target.starts_with(['<', '>', '=', '!', '~']) {
                format!("{name}{target}")
            } else {
                format!("{name}=={target}")
            }
        }
        _ => format!("{name}@{target}"),
    }
}

fn requirement_dependency_name(spec: &str) -> Option<&str> {
    let name = spec
        .split(['<', '>', '=', '!', '~', ';', '[', ' '])
        .next()
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn requirement_dependency_names_match(left: &str, right: &str) -> bool {
    left.replace('_', "-")
        .eq_ignore_ascii_case(&right.replace('_', "-"))
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
        (PackageManager::Uv, Some("devDependencies" | "dev-dependencies")) => {
            vec!["--dev".to_string()]
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
    fn normalizes_step_root_before_package_root_matching() {
        let facts = npm_facts("apps/web", "react", "^17.0.2");
        let plan = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::Pnpm),
                root: Some("./apps/web".to_string()),
                dependencies: vec![dependency_with_if_version("react", "^17.0.0", "^18.0.0")],
            },
            true,
        )
        .unwrap();

        assert_eq!(plan.manager_root.root, PathBuf::from("apps/web"));
        assert_eq!(plan.actions.len(), 1);
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
    fn ensure_skips_python_requirement_when_current_version_satisfies_requirement() {
        let facts = WorkflowFacts {
            schema_version: 1,
            ecosystems: vec![EcosystemFact {
                ecosystem: Ecosystem::PyPI,
                source: EcosystemFactSource::LockFile,
                path: "requirements.txt".to_string(),
            }],
            dependencies: vec![DependencyFact {
                ecosystem: Ecosystem::PyPI,
                name: "requests".to_string(),
                version: "==2.31.0".to_string(),
                path: "requirements.txt".to_string(),
                dependency_type: Some("requirements".to_string()),
            }],
        };
        let plan = plan_bump_dependency_step(
            &facts,
            &UseBumpDependency {
                manager: Some(PackageManager::RequirementsTxt),
                root: Some(".".to_string()),
                dependencies: vec![dependency_with_ensure("requests", ">=2.0.0", None)],
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
            "cd '/repo/apps/web' && 'pnpm' 'add' 'react@^18.0.0' '--save-dev' '--ignore-scripts'"
        );
    }

    #[test]
    fn batches_compatible_package_manager_actions() {
        let commands = commands_for_actions(
            &PackageManagerRoot {
                ecosystem: Ecosystem::Npm,
                manager: PackageManager::Npm,
                root: PathBuf::from("."),
                evidence_path: "package-lock.json".to_string(),
            },
            &[
                BumpDependencyAction {
                    dependency: "react".to_string(),
                    current_version: "^17.0.0".to_string(),
                    target: "^18.0.0".to_string(),
                    manifest_path: "package.json".to_string(),
                    dependency_type: Some("dependencies".to_string()),
                    mode: BumpDependencyMode::Ensure,
                    dry_run: false,
                },
                BumpDependencyAction {
                    dependency: "react-dom".to_string(),
                    current_version: "^17.0.0".to_string(),
                    target: "^18.0.0".to_string(),
                    manifest_path: "package.json".to_string(),
                    dependency_type: Some("dependencies".to_string()),
                    mode: BumpDependencyMode::Ensure,
                    dry_run: false,
                },
            ],
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0].command,
            "cd '/repo' && 'npm' 'install' 'react@^18.0.0' 'react-dom@^18.0.0' '--ignore-scripts'"
        );
    }

    #[test]
    fn splits_package_manager_actions_by_dependency_type_flags() {
        let commands = commands_for_actions(
            &PackageManagerRoot {
                ecosystem: Ecosystem::Npm,
                manager: PackageManager::Npm,
                root: PathBuf::from("."),
                evidence_path: "package-lock.json".to_string(),
            },
            &[
                BumpDependencyAction {
                    dependency: "react".to_string(),
                    current_version: "^17.0.0".to_string(),
                    target: "^18.0.0".to_string(),
                    manifest_path: "package.json".to_string(),
                    dependency_type: Some("dependencies".to_string()),
                    mode: BumpDependencyMode::Ensure,
                    dry_run: false,
                },
                BumpDependencyAction {
                    dependency: "vite".to_string(),
                    current_version: "^5.0.0".to_string(),
                    target: "^6.0.0".to_string(),
                    manifest_path: "package.json".to_string(),
                    dependency_type: Some("devDependencies".to_string()),
                    mode: BumpDependencyMode::Ensure,
                    dry_run: false,
                },
                BumpDependencyAction {
                    dependency: "react-dom".to_string(),
                    current_version: "^17.0.0".to_string(),
                    target: "^18.0.0".to_string(),
                    manifest_path: "package.json".to_string(),
                    dependency_type: Some("dependencies".to_string()),
                    mode: BumpDependencyMode::Ensure,
                    dry_run: false,
                },
            ],
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0].command,
            "cd '/repo' && 'npm' 'install' 'react@^18.0.0' 'react-dom@^18.0.0' '--ignore-scripts'"
        );
        assert_eq!(
            commands[1].command,
            "cd '/repo' && 'npm' 'install' 'vite@^6.0.0' '--save-dev' '--ignore-scripts'"
        );
    }

    #[test]
    fn generates_uv_package_spec() {
        let command = command_for_action(
            &PackageManagerRoot {
                ecosystem: Ecosystem::PyPI,
                manager: PackageManager::Uv,
                root: PathBuf::from("."),
                evidence_path: "uv.lock".to_string(),
            },
            &BumpDependencyAction {
                dependency: "requests".to_string(),
                current_version: "requests>=2.31.0".to_string(),
                target: ">=2.32.0".to_string(),
                manifest_path: "pyproject.toml".to_string(),
                dependency_type: Some("dependencies".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            },
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(
            command.command,
            "cd '/repo' && 'uv' 'add' 'requests>=2.32.0'"
        );
    }

    #[test]
    fn reports_unsupported_package_manager_command_for_manifest_edit_managers() {
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
    async fn executes_maven_manifest_edit() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("pom.xml"),
            r#"<project>
  <dependencies>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter-api</artifactId>
      <version>5.10.1</version>
    </dependency>
  </dependencies>
</project>
"#,
        )
        .unwrap();
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::Java,
                manager: PackageManager::Maven,
                root: PathBuf::from("."),
                evidence_path: "pom.xml".to_string(),
            },
            actions: vec![BumpDependencyAction {
                dependency: "org.junit.jupiter:junit-jupiter-api".to_string(),
                current_version: "5.10.1".to_string(),
                target: "5.10.2".to_string(),
                manifest_path: "pom.xml".to_string(),
                dependency_type: Some("dependencies".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            }],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, temp.path(), &HashMap::new(), None)
                .await
                .unwrap();

        assert!(execution.commands.is_empty());
        assert_eq!(execution.file_edits.len(), 1);
        assert!(fs::read_to_string(temp.path().join("pom.xml"))
            .unwrap()
            .contains("<version>5.10.2</version>"));
        assert_eq!(runner.commands.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn executes_gradle_manifest_edit() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("build.gradle.kts"),
            r#"dependencies {
    implementation("org.slf4j:slf4j-api:2.0.9")
}
"#,
        )
        .unwrap();
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::Java,
                manager: PackageManager::Gradle,
                root: PathBuf::from("."),
                evidence_path: "build.gradle.kts".to_string(),
            },
            actions: vec![BumpDependencyAction {
                dependency: "org.slf4j:slf4j-api".to_string(),
                current_version: "2.0.9".to_string(),
                target: "2.0.12".to_string(),
                manifest_path: "build.gradle.kts".to_string(),
                dependency_type: Some("implementation".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            }],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, temp.path(), &HashMap::new(), None)
                .await
                .unwrap();

        assert!(execution.commands.is_empty());
        assert_eq!(execution.file_edits.len(), 1);
        assert!(fs::read_to_string(temp.path().join("build.gradle.kts"))
            .unwrap()
            .contains(r#"implementation("org.slf4j:slf4j-api:2.0.12")"#));
        assert_eq!(runner.commands.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn executes_requirements_txt_manifest_edit() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("requirements.txt"),
            "flask==3.0.0\nrequests==2.31.0  # keep this comment\n",
        )
        .unwrap();
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::PyPI,
                manager: PackageManager::RequirementsTxt,
                root: PathBuf::from("."),
                evidence_path: "requirements.txt".to_string(),
            },
            actions: vec![BumpDependencyAction {
                dependency: "requests".to_string(),
                current_version: "requests==2.31.0".to_string(),
                target: "2.32.3".to_string(),
                manifest_path: "requirements.txt".to_string(),
                dependency_type: Some("requirements".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            }],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, temp.path(), &HashMap::new(), None)
                .await
                .unwrap();

        assert!(execution.commands.is_empty());
        assert_eq!(execution.file_edits.len(), 1);
        assert_eq!(
            fs::read_to_string(temp.path().join("requirements.txt")).unwrap(),
            "flask==3.0.0\nrequests==2.32.3  # keep this comment\n"
        );
        assert_eq!(runner.commands.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn dry_run_manifest_edit_does_not_write_file() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("build.gradle.kts"),
            r#"dependencies {
    implementation("org.slf4j:slf4j-api:2.0.9")
}
"#,
        )
        .unwrap();
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::Java,
                manager: PackageManager::Gradle,
                root: PathBuf::from("."),
                evidence_path: "build.gradle.kts".to_string(),
            },
            actions: vec![BumpDependencyAction {
                dependency: "org.slf4j:slf4j-api".to_string(),
                current_version: "2.0.9".to_string(),
                target: "2.0.12".to_string(),
                manifest_path: "build.gradle.kts".to_string(),
                dependency_type: Some("implementation".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: true,
            }],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, temp.path(), &HashMap::new(), None)
                .await
                .unwrap();

        assert_eq!(execution.file_edits.len(), 1);
        assert!(execution.file_edits[0].dry_run);
        assert!(fs::read_to_string(temp.path().join("build.gradle.kts"))
            .unwrap()
            .contains("org.slf4j:slf4j-api:2.0.9"));
    }

    #[tokio::test]
    async fn dry_run_requirements_txt_edit_does_not_write_file() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("requirements.txt"), "requests==2.31.0\n").unwrap();
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::PyPI,
                manager: PackageManager::RequirementsTxt,
                root: PathBuf::from("."),
                evidence_path: "requirements.txt".to_string(),
            },
            actions: vec![BumpDependencyAction {
                dependency: "requests".to_string(),
                current_version: "requests==2.31.0".to_string(),
                target: "2.32.3".to_string(),
                manifest_path: "requirements.txt".to_string(),
                dependency_type: Some("requirements".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: true,
            }],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, temp.path(), &HashMap::new(), None)
                .await
                .unwrap();

        assert!(execution.commands.is_empty());
        assert_eq!(execution.file_edits.len(), 1);
        assert!(execution.file_edits[0].dry_run);
        assert_eq!(
            fs::read_to_string(temp.path().join("requirements.txt")).unwrap(),
            "requests==2.31.0\n"
        );
        assert_eq!(runner.commands.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn executes_gradle_groovy_manifest_edit() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("build.gradle"),
            r#"dependencies {
    implementation "org.slf4j:slf4j-api:2.0.9"
}
"#,
        )
        .unwrap();
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::Java,
                manager: PackageManager::Gradle,
                root: PathBuf::from("."),
                evidence_path: "build.gradle".to_string(),
            },
            actions: vec![BumpDependencyAction {
                dependency: "org.slf4j:slf4j-api".to_string(),
                current_version: "2.0.9".to_string(),
                target: "2.0.12".to_string(),
                manifest_path: "build.gradle".to_string(),
                dependency_type: Some("implementation".to_string()),
                mode: BumpDependencyMode::Ensure,
                dry_run: false,
            }],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, temp.path(), &HashMap::new(), None)
                .await
                .unwrap();

        assert!(execution.commands.is_empty());
        assert_eq!(execution.file_edits.len(), 1);
        assert!(fs::read_to_string(temp.path().join("build.gradle"))
            .unwrap()
            .contains(r#"implementation "org.slf4j:slf4j-api:2.0.12""#));
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
        assert!(execution.file_edits.is_empty());
        assert_eq!(runner.commands.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn executes_batched_package_manager_command() {
        let runner = RecordingRunner::default();
        let plan = BumpDependencyPlan {
            manager_root: PackageManagerRoot {
                ecosystem: Ecosystem::Npm,
                manager: PackageManager::Npm,
                root: PathBuf::from("."),
                evidence_path: "package-lock.json".to_string(),
            },
            actions: vec![
                BumpDependencyAction {
                    dependency: "react".to_string(),
                    current_version: "^17.0.0".to_string(),
                    target: "^18.0.0".to_string(),
                    manifest_path: "package.json".to_string(),
                    dependency_type: Some("dependencies".to_string()),
                    mode: BumpDependencyMode::Ensure,
                    dry_run: false,
                },
                BumpDependencyAction {
                    dependency: "react-dom".to_string(),
                    current_version: "^17.0.0".to_string(),
                    target: "^18.0.0".to_string(),
                    manifest_path: "package.json".to_string(),
                    dependency_type: Some("dependencies".to_string()),
                    mode: BumpDependencyMode::Ensure,
                    dry_run: false,
                },
            ],
        };

        let execution =
            execute_bump_dependency_plan(&runner, &plan, Path::new("/repo"), &HashMap::new(), None)
                .await
                .unwrap();

        assert_eq!(execution.commands.len(), 1);
        assert_eq!(
            runner.commands.lock().unwrap().as_slice(),
            ["cd '/repo' && 'npm' 'install' 'react@^18.0.0' 'react-dom@^18.0.0' '--ignore-scripts'"]
        );
    }
}
