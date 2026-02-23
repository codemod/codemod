use anyhow::{Context, Result};
use butterflow_core::utils;
use butterflow_models::step::StepAction;
use clap::Args;
use std::path::{Path, PathBuf};

const SKILL_FILE_NAME: &str = "SKILL.md";
const WORKFLOW_FILE_NAME: &str = "workflow.yaml";

#[derive(Args, Debug)]
pub struct Command {
    /// Path to workflow file
    #[arg(short, long, value_name = "FILE")]
    workflow: PathBuf,
}

/// Validate a workflow file
pub fn handler(args: &Command) -> Result<()> {
    validate_workflow(&args.workflow)
}

fn validate_workflow(workflow_path: &Path) -> Result<()> {
    let workflow_path = normalize_workflow_path(workflow_path)?;

    // Parse workflow file
    let workflow = utils::parse_workflow_file(&workflow_path).context(format!(
        "❌ Failed to parse workflow file: {}",
        workflow_path.display()
    ))?;

    let parent_dir = workflow_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "❌ Cannot get parent directory for path: {}",
            workflow_path.display()
        )
    })?;

    // Validate workflow
    utils::validate_workflow(&workflow, parent_dir).context("❌ Workflow validation failed")?;

    println!("✅ Workflow definition is valid");
    println!("✅ Schema validation: Passed");
    println!(
        "✅ Node dependencies: Valid ({} nodes, {} dependency relationships)",
        workflow.nodes.len(),
        workflow
            .nodes
            .iter()
            .map(|n| n.depends_on.len())
            .sum::<usize>()
    );
    println!(
        "✅ Template references: Valid ({} templates, {} references)",
        workflow.templates.len(),
        workflow
            .nodes
            .iter()
            .flat_map(|n| n.steps.iter())
            .filter_map(|s| {
                match &s.action {
                    StepAction::UseTemplate(template_use) => Some(template_use),
                    _ => None,
                }
            })
            .count()
    );

    // Count matrix nodes
    let matrix_nodes = workflow
        .nodes
        .iter()
        .filter(|n| n.strategy.is_some())
        .count();
    println!("✅ Matrix strategies: Valid ({matrix_nodes} matrix nodes)");

    Ok(())
}

fn normalize_workflow_path(input_path: &Path) -> Result<PathBuf> {
    if is_skill_file_path(input_path) {
        let package_dir = input_path.parent().unwrap_or(input_path);
        return Err(skill_only_validation_error(package_dir));
    }

    if input_path.is_dir() {
        if is_skill_only_package(input_path) {
            return Err(skill_only_validation_error(input_path));
        }
        return Ok(input_path.join(WORKFLOW_FILE_NAME));
    }

    if is_workflow_file_path(input_path)
        && !input_path.exists()
        && input_path.parent().is_some_and(is_skill_only_package)
    {
        return Err(skill_only_validation_error(
            input_path.parent().unwrap_or(input_path),
        ));
    }

    Ok(input_path.to_path_buf())
}

fn is_skill_file_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == SKILL_FILE_NAME)
}

fn is_workflow_file_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == WORKFLOW_FILE_NAME)
}

fn is_skill_only_package(package_dir: &Path) -> bool {
    package_dir.join(SKILL_FILE_NAME).is_file() && !package_dir.join(WORKFLOW_FILE_NAME).is_file()
}

fn skill_only_validation_error(package_dir: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "❌ Skill-only package detected at {} (found `{}` but no `{}`). `codemod workflow validate` requires a workflow package. Install this package as a skill with `npx codemod <package-id> --skill`.",
        package_dir.display(),
        SKILL_FILE_NAME,
        WORKFLOW_FILE_NAME
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn returns_skill_only_error_when_input_is_skill_file() {
        let temp_dir = tempdir().unwrap();
        let skill_path = temp_dir.path().join(SKILL_FILE_NAME);
        fs::write(&skill_path, "# Skill\n").unwrap();

        let err = validate_workflow(&skill_path).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("Skill-only package detected"));
        assert!(message.contains("--skill"));
    }

    #[test]
    fn returns_skill_only_error_when_workflow_is_missing() {
        let temp_dir = tempdir().unwrap();
        fs::write(temp_dir.path().join(SKILL_FILE_NAME), "# Skill\n").unwrap();

        let missing_workflow_path = temp_dir.path().join(WORKFLOW_FILE_NAME);
        let err = validate_workflow(&missing_workflow_path).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("Skill-only package detected"));
        assert!(message.contains("workflow validate"));
    }

    #[test]
    fn validates_workflow_when_directory_contains_workflow_file() {
        let temp_dir = tempdir().unwrap();
        let workflow_path = temp_dir.path().join(WORKFLOW_FILE_NAME);
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

        let result = validate_workflow(temp_dir.path());

        assert!(result.is_ok(), "expected workflow to validate: {result:?}");
    }

    #[test]
    fn validates_workflow_when_directory_is_hybrid_package() {
        let temp_dir = tempdir().unwrap();
        fs::write(temp_dir.path().join(SKILL_FILE_NAME), "# Skill\n").unwrap();
        let workflow_path = temp_dir.path().join(WORKFLOW_FILE_NAME);
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

        let result = validate_workflow(temp_dir.path());

        assert!(
            result.is_ok(),
            "expected hybrid package workflow to validate: {result:?}"
        );
    }
}
