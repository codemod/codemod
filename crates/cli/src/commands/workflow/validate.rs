use crate::utils::ancestor_search::find_in_ancestors;
use crate::utils::manifest::CodemodManifest;
use crate::utils::package_validation::{
    detect_package_behavior_shape, expected_workflow_path, validate_package_behavior_structure,
    validate_skill_behavior, PackageBehaviorShape, DEFAULT_WORKFLOW_FILE_NAME,
};
use anyhow::{anyhow, Context, Result};
use butterflow_core::utils;
use butterflow_models::step::StepAction;
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
pub struct Command {
    /// Path to workflow file or package directory
    #[arg(short, long, value_name = "FILE")]
    workflow: PathBuf,
}

/// Validate a codemod package or workflow file
pub fn handler(args: &Command) -> Result<()> {
    validate_target(&args.workflow)
}

fn validate_target(input_path: &Path) -> Result<()> {
    if let Some(package_root) = resolve_package_root(input_path) {
        return validate_package(&package_root);
    }

    let workflow_path = normalize_non_package_workflow_path(input_path)?;
    validate_workflow_file(&workflow_path)
}

fn resolve_package_root(input_path: &Path) -> Option<PathBuf> {
    if input_path.is_dir() && input_path.join("codemod.yaml").is_file() {
        return Some(input_path.to_path_buf());
    }

    let start_dir = if input_path.is_dir() {
        input_path
    } else {
        input_path.parent().unwrap_or(input_path)
    };

    find_in_ancestors(start_dir, "codemod.yaml")
        .and_then(|manifest_path| manifest_path.parent().map(Path::to_path_buf))
}

fn validate_package(package_root: &Path) -> Result<()> {
    let manifest = load_manifest(package_root)?;
    validate_package_behavior_structure(package_root, &manifest)?;

    let behavior_shape = detect_package_behavior_shape(package_root, &manifest);
    if behavior_shape == PackageBehaviorShape::Missing {
        return Err(anyhow!(
            "❌ Package at {} must include executable workflow steps and/or skill installation steps.",
            package_root.display()
        ));
    }

    println!(
        "✅ Package behavior detected: {} ({})",
        manifest.name,
        behavior_shape.as_str()
    );

    if behavior_shape.includes_workflow() {
        let workflow_path = expected_workflow_path(package_root, &manifest);
        if !workflow_path.is_file() {
            return Err(anyhow!(
                "❌ Workflow behavior declared but workflow file not found at {}.",
                workflow_path.display()
            ));
        }
        validate_workflow_file(&workflow_path)?;
    }

    if behavior_shape.includes_skill() {
        let skill_summary = validate_skill_behavior(package_root, &manifest)?;
        println!(
            "✅ Skill structure: Valid ({})",
            skill_summary.skill_dir.display()
        );
        println!(
            "✅ Skill references: Valid ({} links checked)",
            skill_summary.linked_reference_count
        );
    }

    println!(
        "✅ Package validation successful ({})",
        behavior_shape.as_str()
    );
    Ok(())
}

fn load_manifest(package_root: &Path) -> Result<CodemodManifest> {
    let manifest_path = package_root.join("codemod.yaml");
    if !manifest_path.is_file() {
        return Err(anyhow!(
            "❌ codemod.yaml not found in {}.",
            package_root.display()
        ));
    }

    let manifest_content = fs::read_to_string(&manifest_path).map_err(|error| {
        anyhow!(
            "❌ Failed to read manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    serde_yaml::from_str::<CodemodManifest>(&manifest_content).map_err(|error| {
        anyhow!(
            "❌ Failed to parse manifest {}: {error}",
            manifest_path.display()
        )
    })
}

fn validate_workflow_file(workflow_path: &Path) -> Result<()> {
    let workflow = utils::parse_workflow_file(workflow_path).context(format!(
        "❌ Failed to parse workflow file: {}",
        workflow_path.display()
    ))?;

    let parent_dir = workflow_path.parent().ok_or_else(|| {
        anyhow!(
            "❌ Cannot get parent directory for path: {}",
            workflow_path.display()
        )
    })?;

    utils::validate_workflow(&workflow, parent_dir).context("❌ Workflow validation failed")?;

    println!("✅ Workflow definition is valid");
    println!("✅ Schema validation: Passed");
    println!(
        "✅ Node dependencies: Valid ({} nodes, {} dependency relationships)",
        workflow.nodes.len(),
        workflow
            .nodes
            .iter()
            .map(|node| node.depends_on.len())
            .sum::<usize>()
    );
    println!(
        "✅ Template references: Valid ({} templates, {} references)",
        workflow.templates.len(),
        workflow
            .nodes
            .iter()
            .flat_map(|node| node.steps.iter())
            .filter_map(|step| match &step.action {
                StepAction::UseTemplate(template_use) => Some(template_use),
                _ => None,
            })
            .count()
    );

    let matrix_nodes = workflow
        .nodes
        .iter()
        .filter(|node| node.strategy.is_some())
        .count();
    println!("✅ Matrix strategies: Valid ({matrix_nodes} matrix nodes)");

    Ok(())
}

fn normalize_non_package_workflow_path(input_path: &Path) -> Result<PathBuf> {
    if input_path.is_dir() {
        return Ok(input_path.join(DEFAULT_WORKFLOW_FILE_NAME));
    }

    if input_path
        .file_name()
        .is_some_and(|file_name| file_name == "SKILL.md")
    {
        return Err(anyhow!(
            "❌ Skill file path provided without package context. Run `codemod workflow validate -w <package-root>` where `codemod.yaml` is present."
        ));
    }

    Ok(input_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_manifest(package_dir: &Path, workflow: &str, name: &str) {
        let manifest = format!(
            r#"schema_version: "1.0"
name: "{name}"
version: "1.0.0"
description: "description"
author: "author"
license: "MIT"
workflow: "{workflow}"
capabilities: []"#
        );
        fs::write(package_dir.join("codemod.yaml"), manifest).unwrap();
    }

    fn write_valid_skill_bundle(package_dir: &Path, skill_name: &str) {
        let skill_dir = package_dir.join("agents/skill").join(skill_name);
        fs::create_dir_all(skill_dir.join("references")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: "sample"
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
    fn validates_skill_only_package_when_structure_is_valid() {
        let temp_dir = tempdir().unwrap();
        write_manifest(temp_dir.path(), DEFAULT_WORKFLOW_FILE_NAME, "sample-skill");
        write_valid_skill_bundle(temp_dir.path(), "sample-skill");
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install skill
        install-skill:
          package: "@codemod/sample-skill"
"#,
        )
        .unwrap();

        let result = validate_target(temp_dir.path());
        assert!(
            result.is_ok(),
            "expected skill-only validation to pass: {result:?}"
        );
    }

    #[test]
    fn fails_skill_validation_when_reference_link_is_missing() {
        let temp_dir = tempdir().unwrap();
        write_manifest(temp_dir.path(), DEFAULT_WORKFLOW_FILE_NAME, "sample-skill");
        write_valid_skill_bundle(temp_dir.path(), "sample-skill");
        fs::write(
            temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install skill
        install-skill:
          package: "@codemod/sample-skill"
"#,
        )
        .unwrap();
        fs::write(
            temp_dir
                .path()
                .join("agents/skill/sample-skill/references/index.md"),
            "- [Missing](./missing.md)\n",
        )
        .unwrap();

        let error = validate_target(temp_dir.path()).unwrap_err();
        assert!(error.to_string().contains("links missing path"));
    }

    #[test]
    fn validates_workflow_when_directory_contains_workflow_file() {
        let temp_dir = tempdir().unwrap();
        let workflow_path = temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME);
        fs::write(
            workflow_path,
            r#"
version: "1"
nodes:
  - id: setup
    name: Setup
    type: automatic
    steps:
      - id: init
        name: Initialize
        run: echo hello
"#,
        )
        .unwrap();

        let result = validate_target(temp_dir.path());
        assert!(result.is_ok(), "expected workflow to validate: {result:?}");
    }

    #[test]
    fn validates_workflow_when_directory_is_hybrid_package() {
        let temp_dir = tempdir().unwrap();
        write_manifest(temp_dir.path(), DEFAULT_WORKFLOW_FILE_NAME, "sample-skill");
        write_valid_skill_bundle(temp_dir.path(), "sample-skill");
        let workflow_path = temp_dir.path().join(DEFAULT_WORKFLOW_FILE_NAME);
        fs::write(
            workflow_path,
            r#"
version: "1"
nodes:
  - id: setup
    name: Setup
    type: automatic
    steps:
      - id: init
        name: Initialize
        run: echo hello
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install skill
        install-skill:
          package: "@codemod/sample-skill"
"#,
        )
        .unwrap();

        let result = validate_target(temp_dir.path());
        assert!(
            result.is_ok(),
            "expected hybrid package workflow to validate: {result:?}"
        );
    }

    #[test]
    fn skill_file_without_package_context_is_actionable_error() {
        let temp_dir = tempdir().unwrap();
        let skill_path = temp_dir.path().join("SKILL.md");
        fs::write(&skill_path, "# Skill\n").unwrap();

        let error = validate_target(&skill_path).unwrap_err();
        assert!(error.to_string().contains("package context"));
    }
}
