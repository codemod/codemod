use anyhow::{Context, Result};
use butterflow_core::config::WorkflowRunConfig;
use butterflow_core::engine::Engine;
use butterflow_core::utils;
use butterflow_models::{Task, TaskStatus, WorkflowStatus};
use log::{error, info};
use std::path::PathBuf;
use uuid::Uuid;

/// Run a workflow with the given configuration
pub async fn run_workflow(engine: &Engine, config: WorkflowRunConfig) -> Result<(String, f64)> {
    // Parse workflow file
    let workflow = utils::parse_workflow_file(engine.get_workflow_file_path()).context(format!(
        "Failed to parse workflow file: {}",
        engine.get_workflow_file_path().display()
    ))?;

    let started = std::time::Instant::now();

    // Run workflow
    let workflow_run_id = engine
        .run_workflow(
            workflow,
            config.params,
            Some(config.bundle_path),
            Some(config.target_path),
            config.capabilities.as_ref(),
        )
        .await
        .context("Failed to run workflow")?;

    println!("💥 Workflow started with ID: {workflow_run_id}");

    if config.wait_for_completion {
        wait_for_workflow_completion(engine, workflow_run_id.to_string()).await?;
    }

    let seconds = started.elapsed().as_millis() as f64 / 1000.0;
    println!("✨ Done in {seconds:.3}s");

    Ok((workflow_run_id.to_string(), seconds))
}

/// Wait for workflow to complete or pause
pub async fn wait_for_workflow_completion(engine: &Engine, workflow_run_id: String) -> Result<()> {
    loop {
        // Get workflow status
        let status = engine
            .get_workflow_status(workflow_run_id.clone().parse::<Uuid>()?)
            .await
            .context("Failed to get workflow status")?;

        match status {
            WorkflowStatus::Completed => {
                info!("Workflow completed successfully");
                break;
            }
            WorkflowStatus::Failed => {
                error!("Workflow failed");
                return Err(anyhow::anyhow!("Workflow failed"));
            }
            WorkflowStatus::AwaitingTrigger => {
                // Get tasks awaiting trigger
                let tasks = engine
                    .get_tasks(workflow_run_id.clone().parse::<Uuid>()?)
                    .await
                    .context("Failed to get tasks")?;

                let awaiting_tasks: Vec<&Task> = tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::AwaitingTrigger)
                    .collect();

                info!("Workflow paused: Manual triggers required");
                info!("");
                info!("Workflow is awaiting manual triggers for the following tasks:");
                for task in awaiting_tasks {
                    info!("- {} ({})", task.id, task.node_id);
                }
                info!("");
                info!("Use 'codemod workflow status -i {workflow_run_id}' to check status");
                info!(
                    "Run 'codemod workflow resume -i {workflow_run_id} -t <TASK_ID>' to trigger a specific task"
                );
                info!(
                    "Run 'codemod workflow resume -i {workflow_run_id} --trigger-all' to trigger all awaiting tasks"
                );
                break;
            }
            WorkflowStatus::Running => {
                // Still running, wait a bit before checking again
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            WorkflowStatus::Canceled => {
                info!("Workflow was canceled");
                break;
            }
            _ => {
                // Wait a bit before checking again
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }

    Ok(())
}

/// Resolves the workflow source string into the actual workflow file path
/// and the bundle's root directory path.
pub fn resolve_workflow_source(source: &str) -> Result<(PathBuf, PathBuf)> {
    let path = PathBuf::from(source);

    if !path.exists() {
        // TODO: Add registry lookup logic here in the future
        return Err(anyhow::anyhow!(
            "Workflow source path does not exist: {}",
            source
        ));
    }

    if path.is_dir() {
        let bundle_path = path.canonicalize().context(format!(
            "Failed to get absolute path for bundle directory: {}",
            path.display()
        ))?;
        // Look for default workflow files within the directory
        let default_files = [
            "workflow.yaml",
            "butterflow.yaml",
            "workflow.json",
            "butterflow.json",
        ];
        let mut workflow_file_path = None;

        for file_name in default_files.iter() {
            let potential_path = bundle_path.join(file_name);
            if potential_path.is_file() {
                workflow_file_path = Some(potential_path);
                break;
            }
        }

        match workflow_file_path {
            Some(file) => Ok((file, bundle_path)),
            None => Err(anyhow::anyhow!(
                "No default workflow file (e.g., workflow.yaml) found in directory: {}",
                bundle_path.display()
            )),
        }
    } else if path.is_file() {
        let workflow_file_path = path.canonicalize().context(format!(
            "Failed to get absolute path for workflow file: {}",
            path.display()
        ))?;
        let bundle_path = workflow_file_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Could not get parent directory for workflow file"))?
            .to_path_buf();
        Ok((workflow_file_path, bundle_path))
    } else {
        Err(anyhow::anyhow!(
            "Workflow source path is neither a file nor a directory: {}",
            source
        ))
    }
}
