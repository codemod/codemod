use crate::utils::manifest::CodemodManifest;
use crate::utils::path_safety::{has_parent_path_components, resolve_relative_path_within_root};
use crate::utils::skill_layout::{
    expected_authored_skill_file, find_authored_skill_dir, resolve_configured_skill_file_path,
    AGENTS_SKILL_ROOT_RELATIVE_PATH, SKILL_FILE_NAME,
};
use anyhow::{anyhow, Result};
use butterflow_core::utils::parse_workflow_file;
use butterflow_core::Workflow;
use butterflow_models::step::StepAction;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_WORKFLOW_FILE_NAME: &str = "workflow.yaml";
const CODEMOD_COMPATIBILITY_MARKER_PREFIX: &str = "codemod-compatibility:";
const CODEMOD_VERSION_MARKER_PREFIX: &str = "codemod-skill-version:";
const REQUIRED_FRONTMATTER_KEYS: [&str; 3] = ["name:", "description:", "allowed-tools:"];
const REFERENCES_DIR_NAME: &str = "references";
const REFERENCES_INDEX_FILE_NAME: &str = "index.md";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageBehaviorShape {
    WorkflowOnly,
    SkillOnly,
    WorkflowAndSkill,
    Missing,
}

impl PackageBehaviorShape {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowOnly => "workflow-only",
            Self::SkillOnly => "skill-only",
            Self::WorkflowAndSkill => "workflow-and-skill",
            Self::Missing => "missing-behavior",
        }
    }

    pub(crate) fn includes_workflow(self) -> bool {
        matches!(self, Self::WorkflowOnly | Self::WorkflowAndSkill)
    }

    pub(crate) fn includes_skill(self) -> bool {
        matches!(self, Self::SkillOnly | Self::WorkflowAndSkill)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillValidationSummary {
    pub skill_dir: PathBuf,
    pub linked_reference_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthoredSkillFileCandidate {
    pub path: PathBuf,
    pub explicit: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WorkflowBehaviorSummary {
    has_install_skill_steps: bool,
    has_executable_steps: bool,
}

impl WorkflowBehaviorSummary {
    fn merge(&mut self, other: Self) {
        self.has_install_skill_steps |= other.has_install_skill_steps;
        self.has_executable_steps |= other.has_executable_steps;
    }
}

pub(crate) fn validate_package_behavior_structure(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> Result<()> {
    let workflow_path = expected_workflow_path(package_path, manifest);
    if !workflow_path.is_file() {
        return Err(anyhow!(
            "Workflow file is missing at {}.",
            workflow_path.display()
        ));
    }

    let workflow_summary = workflow_behavior_summary_from_path(&workflow_path)?;
    let authored_skill_candidate =
        authored_skill_file_candidate(package_path, Some(manifest), &manifest.name)?;
    let has_skill_layout = if authored_skill_candidate.explicit {
        authored_skill_candidate.path.is_file()
    } else {
        find_authored_skill_dir(package_path, Some(&manifest.name)).is_some()
    };

    if workflow_summary.has_install_skill_steps && !has_skill_layout {
        return Err(anyhow!(
            "Workflow contains `install-skill` step(s), but authored skill files are missing at {}.",
            authored_skill_candidate.path.display()
        ));
    }

    if has_skill_layout && !workflow_summary.has_install_skill_steps {
        return Err(anyhow!(
            "Authored skill files exist under `{}`, but workflow does not contain any `install-skill` steps.",
            AGENTS_SKILL_ROOT_RELATIVE_PATH
        ));
    }

    Ok(())
}

pub(crate) fn detect_package_behavior_shape(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> PackageBehaviorShape {
    let workflow_summary = workflow_behavior_summary(package_path, manifest).unwrap_or_default();

    let supports_skill = workflow_summary.has_install_skill_steps;
    let supports_workflow = workflow_summary.has_executable_steps;

    match (supports_workflow, supports_skill) {
        (true, true) => PackageBehaviorShape::WorkflowAndSkill,
        (true, false) => PackageBehaviorShape::WorkflowOnly,
        (false, true) => PackageBehaviorShape::SkillOnly,
        (false, false) => PackageBehaviorShape::Missing,
    }
}

pub(crate) fn detect_package_behavior_shape_with_manifest_hint(
    package_path: &Path,
    manifest: Option<&CodemodManifest>,
) -> PackageBehaviorShape {
    if let Some(manifest) = manifest {
        return detect_package_behavior_shape(package_path, manifest);
    }

    let workflow_summary = workflow_behavior_summary_from_path_optional(
        &package_path.join(DEFAULT_WORKFLOW_FILE_NAME),
    )
    .unwrap_or_default();

    let supports_skill = workflow_summary.has_install_skill_steps;
    let supports_workflow = workflow_summary.has_executable_steps;

    match (supports_workflow, supports_skill) {
        (true, true) => PackageBehaviorShape::WorkflowAndSkill,
        (true, false) => PackageBehaviorShape::WorkflowOnly,
        (false, true) => PackageBehaviorShape::SkillOnly,
        (false, false) => PackageBehaviorShape::Missing,
    }
}

pub(crate) fn validate_skill_behavior(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> Result<SkillValidationSummary> {
    let authored_skill_candidate =
        authored_skill_file_candidate(package_path, Some(manifest), &manifest.name)?;
    let skill_file_path = if authored_skill_candidate.explicit {
        authored_skill_candidate.path.clone()
    } else {
        let skill_dir =
            find_authored_skill_dir(package_path, Some(&manifest.name)).ok_or_else(|| {
                anyhow!(
                    "Skill behavior is missing. Expected authored skill at {}.",
                    authored_skill_candidate.path.display()
                )
            })?;
        skill_dir.join(SKILL_FILE_NAME)
    };

    if !skill_file_path.is_file() {
        return Err(anyhow!(
            "Skill behavior is missing SKILL.md at {}.",
            skill_file_path.display()
        ));
    }

    let Some(skill_dir) = skill_file_path.parent().map(Path::to_path_buf) else {
        return Err(anyhow!(
            "Skill behavior path {} has no parent directory.",
            skill_file_path.display()
        ));
    };

    let skill_content = fs::read_to_string(&skill_file_path).map_err(|error| {
        anyhow!(
            "Failed to read skill file {}: {error}",
            skill_file_path.display()
        )
    })?;
    validate_skill_markers_and_frontmatter(&skill_content, &skill_file_path)?;

    let references_dir = skill_dir.join(REFERENCES_DIR_NAME);
    if !references_dir.is_dir() {
        return Err(anyhow!(
            "Skill references directory is missing at {}.",
            references_dir.display()
        ));
    }

    let references_index_path = references_dir.join(REFERENCES_INDEX_FILE_NAME);
    if !references_index_path.is_file() {
        return Err(anyhow!(
            "Skill references index is missing at {}.",
            references_index_path.display()
        ));
    }

    let references_index_content = fs::read_to_string(&references_index_path).map_err(|error| {
        anyhow!(
            "Failed to read skill references index {}: {error}",
            references_index_path.display()
        )
    })?;

    let linked_references = extract_reference_links(&references_index_content);
    for link in &linked_references {
        if is_external_or_anchor_link(link) {
            continue;
        }

        let link_target = strip_link_anchor(link);
        if link_target.is_empty() {
            continue;
        }

        let resolved_path = resolve_relative_path_within_root(&references_dir, link_target)
            .ok_or_else(|| {
                anyhow!(
                    "Skill references index {} has invalid relative link target `{}`. Links must stay within {}.",
                    references_index_path.display(),
                    link_target,
                    references_dir.display()
                )
            })?;
        if !resolved_path.exists() {
            return Err(anyhow!(
                "Skill references index {} links missing path: {}",
                references_index_path.display(),
                resolved_path.display()
            ));
        }
    }

    Ok(SkillValidationSummary {
        skill_dir,
        linked_reference_count: linked_references.len(),
    })
}

pub(crate) fn authored_skill_file_candidate(
    package_path: &Path,
    manifest: Option<&CodemodManifest>,
    package_name: &str,
) -> Result<AuthoredSkillFileCandidate> {
    if let Some(configured_file_path) =
        configured_skill_file_from_workflow(package_path, manifest, package_name)?
    {
        return Ok(AuthoredSkillFileCandidate {
            path: configured_file_path,
            explicit: true,
        });
    }

    Ok(AuthoredSkillFileCandidate {
        path: expected_authored_skill_file(package_path, package_name),
        explicit: false,
    })
}

pub(crate) fn expected_workflow_path(package_path: &Path, manifest: &CodemodManifest) -> PathBuf {
    package_path.join(configured_workflow_path(manifest))
}

pub(crate) fn configured_workflow_path(manifest: &CodemodManifest) -> &str {
    let workflow = manifest.workflow.trim();
    if workflow.is_empty() {
        DEFAULT_WORKFLOW_FILE_NAME
    } else {
        workflow
    }
}

fn workflow_behavior_summary(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> Result<WorkflowBehaviorSummary> {
    workflow_behavior_summary_from_path_optional(&expected_workflow_path(package_path, manifest))
}

fn configured_skill_file_from_workflow(
    package_path: &Path,
    manifest: Option<&CodemodManifest>,
    package_name: &str,
) -> Result<Option<PathBuf>> {
    let workflow_path = workflow_path_with_manifest_hint(package_path, manifest);
    if !workflow_path.is_file() {
        return Ok(None);
    }

    let workflow = parse_workflow_file(&workflow_path).map_err(|error| {
        anyhow!(
            "Failed to parse workflow file {}: {error}",
            workflow_path.display()
        )
    })?;
    let install_skill_steps = collect_install_skill_steps(&workflow);
    let exact_matches = install_skill_steps
        .iter()
        .filter(|install_skill| {
            exact_package_identifier_match(&install_skill.package, package_name)
        })
        .collect::<Vec<_>>();

    if !exact_matches.is_empty() {
        return configured_path_from_matching_steps(
            package_path,
            &workflow_path,
            package_name,
            &exact_matches,
        );
    }

    Ok(None)
}

fn collect_install_skill_steps(
    workflow: &Workflow,
) -> Vec<butterflow_models::step::UseInstallSkill> {
    let template_by_id = workflow
        .templates
        .iter()
        .map(|template| (template.id.clone(), template))
        .collect::<HashMap<_, _>>();

    let mut install_skill_steps = Vec::new();
    for node in &workflow.nodes {
        for step in &node.steps {
            collect_install_skill_steps_from_action(
                &step.action,
                &template_by_id,
                &mut HashSet::new(),
                &mut install_skill_steps,
            );
        }
    }

    install_skill_steps
}

fn collect_install_skill_steps_from_action(
    action: &StepAction,
    template_by_id: &HashMap<String, &butterflow_models::template::Template>,
    visiting_templates: &mut HashSet<String>,
    install_skill_steps: &mut Vec<butterflow_models::step::UseInstallSkill>,
) {
    match action {
        StepAction::InstallSkill(install_skill) => {
            install_skill_steps.push(install_skill.clone());
        }
        StepAction::UseTemplate(template_use) => {
            let Some(template) = template_by_id.get(&template_use.template) else {
                return;
            };

            if !visiting_templates.insert(template_use.template.clone()) {
                return;
            }

            for step in &template.steps {
                collect_install_skill_steps_from_action(
                    &step.action,
                    template_by_id,
                    visiting_templates,
                    install_skill_steps,
                );
            }
            visiting_templates.remove(&template_use.template);
        }
        _ => {}
    }
}

fn configured_path_from_matching_steps(
    package_path: &Path,
    workflow_path: &Path,
    package_name: &str,
    matching_steps: &[&butterflow_models::step::UseInstallSkill],
) -> Result<Option<PathBuf>> {
    let mut resolved_paths = HashSet::new();
    for install_skill in matching_steps {
        let Some(configured_path) = install_skill.path.as_deref() else {
            continue;
        };

        let trimmed = configured_path.trim();
        if trimmed.is_empty() {
            return Err(anyhow!(
                "Workflow file {} has an install-skill step with an empty `path` value for package `{}`.",
                workflow_path.display(),
                install_skill.package.trim()
            ));
        }
        if Path::new(trimmed).is_absolute() {
            return Err(anyhow!(
                "Workflow file {} has an install-skill step with absolute `path` value `{}` for package `{}`. Use a package-relative path.",
                workflow_path.display(),
                trimmed,
                install_skill.package.trim()
            ));
        }
        if has_parent_path_components(Path::new(trimmed)) {
            return Err(anyhow!(
                "Workflow file {} has an install-skill step with parent-directory traversal in `path` value `{}` for package `{}`. Use a package-relative path inside the package root.",
                workflow_path.display(),
                trimmed,
                install_skill.package.trim()
            ));
        }

        let configured_skill_file_path = resolve_configured_skill_file_path(package_path, trimmed)
            .ok_or_else(|| {
                anyhow!(
                    "Workflow file {} has an install-skill step with invalid `path` value `{}` for package `{}`.",
                    workflow_path.display(),
                    trimmed,
                    install_skill.package.trim()
                )
            })?;
        resolved_paths.insert(configured_skill_file_path);
    }

    match resolved_paths.len() {
        0 => Ok(None),
        1 => Ok(resolved_paths.into_iter().next()),
        _ => Err(anyhow!(
            "Workflow file {} has conflicting install-skill `path` values for package `{}`. Ensure matching install-skill steps resolve to a single skill path.",
            workflow_path.display(),
            package_name
        )),
    }
}

fn exact_package_identifier_match(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn workflow_path_with_manifest_hint(
    package_path: &Path,
    manifest: Option<&CodemodManifest>,
) -> PathBuf {
    if let Some(manifest) = manifest {
        expected_workflow_path(package_path, manifest)
    } else {
        package_path.join(DEFAULT_WORKFLOW_FILE_NAME)
    }
}

fn workflow_behavior_summary_from_path_optional(
    workflow_path: &Path,
) -> Result<WorkflowBehaviorSummary> {
    if !workflow_path.is_file() {
        return Ok(WorkflowBehaviorSummary::default());
    }
    workflow_behavior_summary_from_path(workflow_path)
}

fn workflow_behavior_summary_from_path(workflow_path: &Path) -> Result<WorkflowBehaviorSummary> {
    let workflow = parse_workflow_file(workflow_path).map_err(|error| {
        anyhow!(
            "Failed to parse workflow file {}: {error}",
            workflow_path.display()
        )
    })?;
    Ok(analyze_workflow_behavior(&workflow))
}

fn analyze_workflow_behavior(workflow: &Workflow) -> WorkflowBehaviorSummary {
    let template_by_id = workflow
        .templates
        .iter()
        .map(|template| (template.id.clone(), template))
        .collect::<HashMap<_, _>>();

    let mut summary = WorkflowBehaviorSummary::default();
    for node in &workflow.nodes {
        for step in &node.steps {
            summary.merge(analyze_step_action(
                &step.action,
                &template_by_id,
                &mut HashSet::new(),
            ));
        }
    }

    summary
}

fn analyze_step_action(
    action: &StepAction,
    template_by_id: &HashMap<String, &butterflow_models::template::Template>,
    visiting_templates: &mut HashSet<String>,
) -> WorkflowBehaviorSummary {
    match action {
        StepAction::InstallSkill(_) => WorkflowBehaviorSummary {
            has_install_skill_steps: true,
            has_executable_steps: false,
        },
        StepAction::UseTemplate(template_use) => {
            let Some(template) = template_by_id.get(&template_use.template) else {
                // Template existence is validated separately.
                return WorkflowBehaviorSummary::default();
            };

            if !visiting_templates.insert(template_use.template.clone()) {
                return WorkflowBehaviorSummary::default();
            }

            let mut summary = WorkflowBehaviorSummary::default();
            for step in &template.steps {
                summary.merge(analyze_step_action(
                    &step.action,
                    template_by_id,
                    visiting_templates,
                ));
            }
            visiting_templates.remove(&template_use.template);
            summary
        }
        _ => WorkflowBehaviorSummary {
            has_install_skill_steps: false,
            has_executable_steps: true,
        },
    }
}

fn validate_skill_markers_and_frontmatter(content: &str, skill_file_path: &Path) -> Result<()> {
    let Some(frontmatter) = extract_frontmatter(content) else {
        return Err(anyhow!(
            "Skill file {} is missing YAML frontmatter.",
            skill_file_path.display()
        ));
    };

    if let Some(required_key) = missing_required_frontmatter_key(frontmatter) {
        return Err(anyhow!(
            "Skill file {} is missing required frontmatter key: {required_key}",
            skill_file_path.display()
        ));
    }

    serde_yaml::from_str::<serde_yaml::Value>(frontmatter).map_err(|error| {
        anyhow!(
            "Skill file {} frontmatter is invalid YAML: {error}",
            skill_file_path.display()
        )
    })?;

    if !content.contains(CODEMOD_COMPATIBILITY_MARKER_PREFIX) {
        return Err(anyhow!(
            "Skill file {} is missing compatibility marker (`codemod-compatibility`).",
            skill_file_path.display()
        ));
    }

    if !content.contains(CODEMOD_VERSION_MARKER_PREFIX) {
        return Err(anyhow!(
            "Skill file {} is missing version marker (`codemod-skill-version`).",
            skill_file_path.display()
        ));
    }

    Ok(())
}

fn extract_frontmatter(content: &str) -> Option<&str> {
    if !content.starts_with("---") {
        return None;
    }

    let remaining = &content[3..];
    let end_marker_index = remaining.find("\n---")?;
    Some(remaining[..end_marker_index].trim())
}

fn missing_required_frontmatter_key(frontmatter: &str) -> Option<&'static str> {
    REQUIRED_FRONTMATTER_KEYS
        .iter()
        .find(|key| {
            !frontmatter
                .lines()
                .any(|line| line.trim().starts_with(**key))
        })
        .copied()
}

fn extract_reference_links(markdown: &str) -> Vec<String> {
    let mut links = Vec::new();
    for line in markdown.lines() {
        let mut rest = line;
        while let Some(start_index) = rest.find("](") {
            let after_start = &rest[start_index + 2..];
            let Some(end_index) = after_start.find(')') else {
                break;
            };
            let link = after_start[..end_index].trim();
            if !link.is_empty() {
                links.push(link.to_string());
            }
            rest = &after_start[end_index + 1..];
        }
    }
    links
}

fn is_external_or_anchor_link(link: &str) -> bool {
    let trimmed = link.trim();
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with('#')
}

fn strip_link_anchor(link: &str) -> &str {
    link.split('#').next().unwrap_or(link).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn manifest_with(name: &str) -> CodemodManifest {
        CodemodManifest {
            schema_version: "1.0".to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: "description".to_string(),
            author: "author".to_string(),
            license: None,
            copyright: None,
            repository: None,
            homepage: None,
            bugs: None,
            registry: None,
            workflow: DEFAULT_WORKFLOW_FILE_NAME.to_string(),
            targets: None,
            dependencies: None,
            keywords: None,
            category: None,
            readme: None,
            changelog: None,
            documentation: None,
            validation: None,
            capabilities: None,
        }
    }

    fn write_valid_skill_bundle_to_dir(skill_dir: &Path) {
        fs::create_dir_all(skill_dir.join("references")).unwrap();
        fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            r#"---
name: "example"
description: "description"
allowed-tools:
  - Bash(codemod *)
---
codemod-compatibility: skill-package-v1
codemod-skill-version: 0.1.0
"#,
        )
        .unwrap();
        fs::write(
            skill_dir.join("references/index.md"),
            "- [Usage](./usage.md)\n",
        )
        .unwrap();
        fs::write(skill_dir.join("references/usage.md"), "# Usage\n").unwrap();
    }

    fn write_valid_skill_bundle(package_path: &Path, skill_name: &str) {
        let skill_dir = package_path
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join(skill_name);
        write_valid_skill_bundle_to_dir(&skill_dir);
    }

    fn write_workflow(path: &Path, body: &str) {
        fs::write(path.join(DEFAULT_WORKFLOW_FILE_NAME), body).unwrap();
    }

    #[test]
    fn validate_skill_behavior_accepts_valid_bundle() {
        let temp_dir = tempdir().unwrap();
        write_valid_skill_bundle(temp_dir.path(), "example");
        let manifest = manifest_with("example");

        let validation = validate_skill_behavior(temp_dir.path(), &manifest).unwrap();
        assert!(validation.skill_dir.ends_with("example"));
        assert_eq!(validation.linked_reference_count, 1);
    }

    #[test]
    fn validate_skill_behavior_rejects_missing_reference_target() {
        let temp_dir = tempdir().unwrap();
        write_valid_skill_bundle(temp_dir.path(), "example");
        let manifest = manifest_with("example");
        fs::write(
            temp_dir
                .path()
                .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
                .join("example/references/index.md"),
            "- [Missing](./missing.md)\n",
        )
        .unwrap();

        let error = validate_skill_behavior(temp_dir.path(), &manifest).unwrap_err();
        assert!(error.to_string().contains("links missing path"));
    }

    #[test]
    fn validate_skill_behavior_supports_custom_install_skill_path() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("@codemod/example");
        let custom_skill_dir = temp_dir.path().join("skills/agents/example");
        write_valid_skill_bundle_to_dir(&custom_skill_dir);
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - name: install
        install-skill:
          package: "@codemod/example"
          path: "./skills/agents/example/SKILL.md"
"#,
        );

        validate_package_behavior_structure(temp_dir.path(), &manifest)
            .expect("custom install-skill path should validate");
        let validation = validate_skill_behavior(temp_dir.path(), &manifest)
            .expect("skill validation should use custom path");
        assert!(validation.skill_dir.ends_with("skills/agents/example"));
    }

    #[test]
    fn validate_skill_behavior_rejects_reference_links_with_parent_traversal() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("@codemod/example");
        write_valid_skill_bundle(temp_dir.path(), "example");

        let references_index = temp_dir
            .path()
            .join("agents/skill/example/references/index.md");
        fs::write(references_index, "[escape](../outside.md)\n").unwrap();

        let error = validate_skill_behavior(temp_dir.path(), &manifest).unwrap_err();
        assert!(error
            .to_string()
            .contains("has invalid relative link target"));
    }

    #[test]
    fn authored_skill_file_candidate_rejects_parent_traversal_path() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("@codemod/example");
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - name: install
        install-skill:
          package: "@codemod/example"
          path: "../outside/SKILL.md"
"#,
        );

        let error = authored_skill_file_candidate(temp_dir.path(), Some(&manifest), &manifest.name)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("parent-directory traversal in `path` value"));
    }

    #[test]
    fn authored_skill_file_candidate_prefers_exact_package_match() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("@codemod/example");
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - name: install-one
        install-skill:
          package: "@other/example"
          path: "./skills/agents/other/SKILL.md"
      - name: install-two
        install-skill:
          package: "@codemod/example"
          path: "./skills/agents/exact/SKILL.md"
"#,
        );

        let candidate =
            authored_skill_file_candidate(temp_dir.path(), Some(&manifest), &manifest.name)
                .expect("candidate should resolve");
        assert!(candidate.explicit);
        assert_eq!(
            candidate.path,
            temp_dir.path().join("skills/agents/exact/SKILL.md")
        );
    }

    #[test]
    fn detect_package_behavior_shape_identifies_workflow_only() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("example");
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: setup
    name: Setup
    type: automatic
    steps:
      - name: setup
        run: echo hello
"#,
        );

        assert_eq!(
            detect_package_behavior_shape(temp_dir.path(), &manifest),
            PackageBehaviorShape::WorkflowOnly
        );
    }

    #[test]
    fn detect_package_behavior_shape_identifies_skill_only() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("example");
        write_valid_skill_bundle(temp_dir.path(), "example");
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - name: install
        install-skill:
          package: "@codemod/example"
"#,
        );

        assert_eq!(
            detect_package_behavior_shape(temp_dir.path(), &manifest),
            PackageBehaviorShape::SkillOnly
        );
    }

    #[test]
    fn detect_package_behavior_shape_identifies_workflow_and_skill() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("example");
        write_valid_skill_bundle(temp_dir.path(), "example");
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: setup
    name: Setup
    type: automatic
    steps:
      - name: setup
        run: echo hello
  - id: install
    name: Install
    type: automatic
    steps:
      - name: install
        install-skill:
          package: "@codemod/example"
"#,
        );

        assert_eq!(
            detect_package_behavior_shape(temp_dir.path(), &manifest),
            PackageBehaviorShape::WorkflowAndSkill
        );
    }

    #[test]
    fn validate_package_behavior_structure_requires_install_skill_for_authored_skill() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("example");
        write_valid_skill_bundle(temp_dir.path(), "example");
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: setup
    name: Setup
    type: automatic
    steps:
      - name: setup
        run: echo hello
"#,
        );

        let error = validate_package_behavior_structure(temp_dir.path(), &manifest).unwrap_err();
        assert!(error
            .to_string()
            .contains("workflow does not contain any `install-skill` steps"));
    }

    #[test]
    fn validate_package_behavior_structure_requires_authored_skill_for_install_skill() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with("example");
        write_workflow(
            temp_dir.path(),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - name: install
        install-skill:
          package: "@codemod/example"
"#,
        );

        let error = validate_package_behavior_structure(temp_dir.path(), &manifest).unwrap_err();
        assert!(error
            .to_string()
            .contains("Workflow contains `install-skill` step(s)"));
    }
}
