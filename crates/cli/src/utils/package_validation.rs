use crate::utils::manifest::CodemodManifest;
use crate::utils::skill_layout::{
    expected_authored_skill_file, find_authored_skill_dir, AGENTS_SKILL_ROOT_RELATIVE_PATH,
    SKILL_FILE_NAME,
};
use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_WORKFLOW_FILE_NAME: &str = "workflow.yaml";
const SKILL_PROVIDES_NAMES: [&str; 1] = ["skill"];
const WORKFLOW_PROVIDES_NAMES: [&str; 1] = ["workflow"];
const CODEMOD_COMPATIBILITY_MARKER_PREFIX: &str = "codemod-compatibility:";
const CODEMOD_VERSION_MARKER_PREFIX: &str = "codemod-skill-version:";
const REQUIRED_FRONTMATTER_KEYS: [&str; 3] = ["name:", "description:", "allowed-tools:"];
const REFERENCES_DIR_NAME: &str = "references";
const REFERENCES_INDEX_FILE_NAME: &str = "index.md";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageBehaviorShape {
    WorkflowOnly,
    SkillOnly,
    Hybrid,
    Missing,
}

impl PackageBehaviorShape {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowOnly => "workflow-only",
            Self::SkillOnly => "skill-only",
            Self::Hybrid => "hybrid",
            Self::Missing => "missing-behavior",
        }
    }

    pub(crate) fn includes_workflow(self) -> bool {
        matches!(self, Self::WorkflowOnly | Self::Hybrid)
    }

    pub(crate) fn includes_skill(self) -> bool {
        matches!(self, Self::SkillOnly | Self::Hybrid)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillValidationSummary {
    pub skill_dir: PathBuf,
    pub linked_reference_count: usize,
}

pub(crate) fn validate_manifest_provides_declarations(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> Result<()> {
    let declares_skill = manifest_declares_provides(manifest, &SKILL_PROVIDES_NAMES);
    let declares_workflow = manifest_declares_provides(manifest, &WORKFLOW_PROVIDES_NAMES)
        || configured_workflow_path(manifest).is_some();
    let has_skill_file = find_authored_skill_dir(package_path, Some(&manifest.name)).is_some();
    let has_workflow_file = workflow_file_exists(package_path, manifest);

    if declares_skill && !has_skill_file {
        return Err(anyhow!(
            "`provides: [skill]` declared in codemod.yaml but `{}` is missing at {}.",
            AGENTS_SKILL_ROOT_RELATIVE_PATH,
            expected_authored_skill_file(package_path, &manifest.name).display()
        ));
    }

    if declares_workflow && !has_workflow_file {
        return Err(anyhow!(
            "`provides: [workflow]` declared in codemod.yaml but workflow file is missing at {}.",
            expected_workflow_path(package_path, manifest).display()
        ));
    }

    Ok(())
}

pub(crate) fn detect_package_behavior_shape(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> PackageBehaviorShape {
    let has_skill = find_authored_skill_dir(package_path, Some(&manifest.name)).is_some();
    let has_workflow = workflow_file_exists(package_path, manifest);
    let declares_skill = manifest_declares_provides(manifest, &SKILL_PROVIDES_NAMES);
    let declares_workflow = manifest_declares_provides(manifest, &WORKFLOW_PROVIDES_NAMES)
        || configured_workflow_path(manifest).is_some();

    let supports_skill = has_skill || declares_skill;
    let supports_workflow = has_workflow || declares_workflow;

    match (supports_workflow, supports_skill) {
        (true, true) => PackageBehaviorShape::Hybrid,
        (true, false) => PackageBehaviorShape::WorkflowOnly,
        (false, true) => PackageBehaviorShape::SkillOnly,
        (false, false) => PackageBehaviorShape::Missing,
    }
}

pub(crate) fn detect_package_behavior_shape_with_manifest_hint(
    package_path: &Path,
    manifest: Option<&CodemodManifest>,
    package_name_hint: Option<&str>,
) -> PackageBehaviorShape {
    if let Some(manifest) = manifest {
        return detect_package_behavior_shape(package_path, manifest);
    }

    let has_skill = find_authored_skill_dir(package_path, package_name_hint).is_some();
    let has_workflow = package_path.join(DEFAULT_WORKFLOW_FILE_NAME).is_file();

    match (has_workflow, has_skill) {
        (true, true) => PackageBehaviorShape::Hybrid,
        (true, false) => PackageBehaviorShape::WorkflowOnly,
        (false, true) => PackageBehaviorShape::SkillOnly,
        (false, false) => PackageBehaviorShape::Missing,
    }
}

pub(crate) fn validate_skill_behavior(
    package_path: &Path,
    manifest: &CodemodManifest,
) -> Result<SkillValidationSummary> {
    let skill_dir =
        find_authored_skill_dir(package_path, Some(&manifest.name)).ok_or_else(|| {
            anyhow!(
                "Skill behavior is missing. Expected authored skill at {}.",
                expected_authored_skill_file(package_path, &manifest.name).display()
            )
        })?;
    let skill_file_path = skill_dir.join(SKILL_FILE_NAME);

    if !skill_file_path.is_file() {
        return Err(anyhow!(
            "Skill behavior is missing SKILL.md at {}.",
            skill_file_path.display()
        ));
    }

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

        let resolved_path = references_dir.join(link_target);
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

pub(crate) fn workflow_file_exists(package_path: &Path, manifest: &CodemodManifest) -> bool {
    expected_workflow_path(package_path, manifest).is_file()
}

pub(crate) fn expected_workflow_path(package_path: &Path, manifest: &CodemodManifest) -> PathBuf {
    let workflow_file = configured_workflow_path(manifest).unwrap_or(DEFAULT_WORKFLOW_FILE_NAME);
    package_path.join(workflow_file)
}

pub(crate) fn configured_workflow_path(manifest: &CodemodManifest) -> Option<&str> {
    manifest
        .workflow
        .as_deref()
        .map(str::trim)
        .filter(|workflow| !workflow.is_empty())
}

pub(crate) fn manifest_declares_provides(manifest: &CodemodManifest, expected: &[&str]) -> bool {
    manifest.provides.as_ref().is_some_and(|provides| {
        provides
            .iter()
            .map(|provided_value| normalize_provided_value(provided_value))
            .any(|provided_value| expected.contains(&provided_value.as_str()))
    })
}

fn normalize_provided_value(provided_value: &str) -> String {
    provided_value.trim().to_ascii_lowercase().replace('_', "-")
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

    fn manifest_with(provides: Option<Vec<&str>>) -> CodemodManifest {
        CodemodManifest {
            schema_version: "1.0".to_string(),
            name: "example".to_string(),
            version: "1.0.0".to_string(),
            description: "description".to_string(),
            author: "author".to_string(),
            license: None,
            copyright: None,
            repository: None,
            homepage: None,
            bugs: None,
            registry: None,
            workflow: Some(DEFAULT_WORKFLOW_FILE_NAME.to_string()),
            targets: None,
            dependencies: None,
            keywords: None,
            category: None,
            readme: None,
            changelog: None,
            documentation: None,
            validation: None,
            provides: provides.map(|entries| entries.into_iter().map(str::to_string).collect()),
            capabilities: None,
        }
    }

    fn write_valid_skill_bundle(package_path: &Path) {
        let skill_dir = package_path
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join("example");
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

    #[test]
    fn validate_skill_behavior_accepts_valid_bundle() {
        let temp_dir = tempdir().unwrap();
        write_valid_skill_bundle(temp_dir.path());
        let manifest = manifest_with(Some(vec!["skill"]));

        let validation = validate_skill_behavior(temp_dir.path(), &manifest).unwrap();
        assert!(validation.skill_dir.ends_with("example"));
        assert_eq!(validation.linked_reference_count, 1);
    }

    #[test]
    fn validate_skill_behavior_rejects_missing_reference_target() {
        let temp_dir = tempdir().unwrap();
        write_valid_skill_bundle(temp_dir.path());
        let manifest = manifest_with(Some(vec!["skill"]));
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
    fn validate_manifest_provides_declarations_rejects_missing_skill_layout() {
        let temp_dir = tempdir().unwrap();
        let manifest = manifest_with(Some(vec!["skill"]));

        let error =
            validate_manifest_provides_declarations(temp_dir.path(), &manifest).unwrap_err();
        assert!(error.to_string().contains("`provides: [skill]`"));
    }

    #[test]
    fn detect_package_behavior_shape_identifies_hybrid() {
        let temp_dir = tempdir().unwrap();
        write_valid_skill_bundle(temp_dir.path());
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
            "version: \"1\"\nnodes: []\n",
        )
        .unwrap();
        let manifest = manifest_with(Some(vec!["workflow", "skill"]));

        assert_eq!(
            detect_package_behavior_shape(temp_dir.path(), &manifest),
            PackageBehaviorShape::Hybrid
        );
    }
}
