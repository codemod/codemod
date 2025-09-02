use anyhow::{Context, Result};
use butterflow_core::utils;
use butterflow_models::step::StepAction;
use clap::Args;
use std::path::{Path, PathBuf};

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
    // Parse workflow file
    let workflow = utils::parse_workflow_file(workflow_path).context(format!(
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
