use anyhow::{Context, Result};
use butterflow_core::config::WorkflowRunConfig;
use butterflow_core::engine::Engine;
use butterflow_core::utils;
use butterflow_models::node::NodeType;
use butterflow_models::trigger::TriggerType;
use butterflow_models::{Task, TaskStatus, Workflow, WorkflowStatus};
use log::{error, info};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::tui::{create_tui_progress_callback, run_workflow_tui_with_session};
use butterflow_core::workflow_runtime::WorkflowSession;

pub fn workflow_has_manual_steps(workflow: &Workflow) -> bool {
    workflow.nodes.iter().any(node_requires_manual_tui)
}

fn node_requires_manual_tui(node: &butterflow_models::Node) -> bool {
    node_has_manual_gate(node) && !is_pull_request_only_manual_publish_node(node)
}

fn node_has_manual_gate(node: &butterflow_models::Node) -> bool {
    node.r#type == NodeType::Manual
        || node
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger.r#type == TriggerType::Manual)
}

fn is_pull_request_only_manual_publish_node(node: &butterflow_models::Node) -> bool {
    node.steps.is_empty() && node.pull_request.is_some()
}

/// Run a workflow with the given configuration
pub async fn run_workflow(engine: &mut Engine, config: WorkflowRunConfig) -> Result<(String, f64)> {
    // Parse workflow file
    let workflow = utils::parse_workflow_file(engine.get_workflow_file_path()).context(format!(
        "Failed to parse workflow file: {}",
        engine.get_workflow_file_path().display()
    ))?;
    let auto_launch_tui = !config.no_interactive && workflow_has_manual_steps(&workflow);

    let started = std::time::Instant::now();

    // Run workflow
    let workflow_run_id = if auto_launch_tui {
        let workflow_run_id = Uuid::new_v4();
        engine.set_progress_callback(Arc::new(Some(create_tui_progress_callback(
            workflow_run_id,
        ))));
        let session = WorkflowSession::start_workflow_with_id(
            engine.clone(),
            workflow_run_id,
            workflow,
            config.params,
            Some(config.bundle_path),
            config.capabilities.as_ref(),
        )
        .await
        .context("Failed to run workflow")?;
        println!("💥 Workflow started with ID: {workflow_run_id}");
        run_workflow_tui_with_session(engine.clone(), session, 20).await?;
        workflow_run_id
    } else {
        let workflow_run_id = engine
            .run_workflow(
                workflow,
                config.params,
                Some(config.bundle_path),
                config.capabilities.as_ref(),
            )
            .await
            .context("Failed to run workflow")?;
        println!("💥 Workflow started with ID: {workflow_run_id}");
        workflow_run_id
    };

    if !auto_launch_tui && config.wait_for_completion {
        wait_for_workflow_completion(engine, workflow_run_id.to_string(), config.no_interactive)
            .await?;
    }

    let seconds = started.elapsed().as_millis() as f64 / 1000.0;
    println!("✨ Done in {seconds:.3}s");

    Ok((workflow_run_id.to_string(), seconds))
}

/// Wait for workflow to complete or pause
pub async fn wait_for_workflow_completion(
    engine: &Engine,
    workflow_run_id: String,
    emit_heartbeat: bool,
) -> Result<()> {
    let workflow_run_uuid = workflow_run_id.parse::<Uuid>()?;
    let wait_started = Instant::now();
    let mut last_heartbeat = Instant::now();

    loop {
        // Get workflow status
        let status = engine
            .get_workflow_status(workflow_run_uuid)
            .await
            .context("Failed to get workflow status")?;

        match status {
            WorkflowStatus::Completed => {
                println!(
                    "✅ Workflow completed successfully in {:.1}s",
                    wait_started.elapsed().as_secs_f64()
                );
                info!("Workflow completed successfully");
                break;
            }
            WorkflowStatus::Failed => {
                let tasks = engine
                    .get_tasks(workflow_run_uuid)
                    .await
                    .context("Failed to get tasks")?;
                let summary = summarize_tasks(&tasks);
                println!(
                    "❌ Workflow failed after {:.1}s{}",
                    wait_started.elapsed().as_secs_f64(),
                    format_summary_suffix(&summary)
                );
                error!("Workflow failed");
                return Err(anyhow::anyhow!("Workflow failed"));
            }
            WorkflowStatus::AwaitingTrigger => {
                // Get tasks awaiting trigger
                let tasks = engine
                    .get_tasks(workflow_run_uuid)
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
                if emit_heartbeat && last_heartbeat.elapsed() >= Duration::from_secs(15) {
                    let tasks = engine
                        .get_tasks(workflow_run_uuid)
                        .await
                        .context("Failed to get tasks")?;
                    let summary = summarize_tasks(&tasks);
                    println!(
                        "⏳ Workflow still running ({:.0}s elapsed{})",
                        wait_started.elapsed().as_secs_f64(),
                        format_summary_suffix(&summary)
                    );
                    last_heartbeat = Instant::now();
                }
                // Still running, wait a bit before checking again
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            WorkflowStatus::Canceled => {
                println!(
                    "🛑 Workflow was canceled after {:.1}s",
                    wait_started.elapsed().as_secs_f64()
                );
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

#[derive(Default)]
struct TaskSummary {
    running: usize,
    pending: usize,
    awaiting_trigger: usize,
    blocked: usize,
    completed: usize,
    failed: usize,
    wont_do: usize,
    active_nodes: Vec<String>,
}

fn summarize_tasks(tasks: &[Task]) -> TaskSummary {
    let mut summary = TaskSummary::default();

    for task in tasks {
        match task.status {
            TaskStatus::Running => {
                summary.running += 1;
                summary.active_nodes.push(task.node_id.clone());
            }
            TaskStatus::Pending => summary.pending += 1,
            TaskStatus::AwaitingTrigger => summary.awaiting_trigger += 1,
            TaskStatus::Blocked => summary.blocked += 1,
            TaskStatus::Completed => summary.completed += 1,
            TaskStatus::Failed => summary.failed += 1,
            TaskStatus::WontDo => summary.wont_do += 1,
        }
    }

    summary.active_nodes.sort();
    summary.active_nodes.dedup();
    summary
}

fn format_summary_suffix(summary: &TaskSummary) -> String {
    let mut parts = Vec::new();

    if summary.running > 0 {
        parts.push(format!("{} running", summary.running));
    }
    if summary.pending > 0 {
        parts.push(format!("{} pending", summary.pending));
    }
    if summary.awaiting_trigger > 0 {
        parts.push(format!("{} awaiting trigger", summary.awaiting_trigger));
    }
    if summary.blocked > 0 {
        parts.push(format!("{} blocked", summary.blocked));
    }
    if summary.completed > 0 {
        parts.push(format!("{} completed", summary.completed));
    }
    if summary.failed > 0 {
        parts.push(format!("{} failed", summary.failed));
    }
    if summary.wont_do > 0 {
        parts.push(format!("{} skipped", summary.wont_do));
    }

    if !summary.active_nodes.is_empty() {
        let preview = summary
            .active_nodes
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let remainder = summary.active_nodes.len().saturating_sub(3);
        if remainder > 0 {
            parts.push(format!("active nodes: {preview} (+{remainder} more)"));
        } else {
            parts.push(format!("active nodes: {preview}"));
        }
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("; {}", parts.join(", "))
    }
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
#[cfg(test)]
mod tests {
    use super::{
        format_summary_suffix, node_requires_manual_tui, summarize_tasks, workflow_has_manual_steps,
    };
    use butterflow_models::node::NodeType;
    use butterflow_models::step::{PullRequestConfig, Step, StepAction};
    use butterflow_models::{Node, Task, TaskStatus, Workflow};
    use std::collections::HashMap;
    use uuid::Uuid;

    fn task(node_id: &str, status: TaskStatus) -> Task {
        Task {
            id: Uuid::new_v4(),
            workflow_run_id: Uuid::new_v4(),
            node_id: node_id.to_string(),
            status,
            is_master: false,
            master_task_id: None,
            matrix_values: None,
            started_at: None,
            ended_at: None,
            error: None,
            logs: Vec::new(),
        }
    }

    #[test]
    fn summarize_tasks_tracks_counts_and_active_nodes() {
        let summary = summarize_tasks(&[
            task("scan-js", TaskStatus::Running),
            task("scan-js", TaskStatus::Running),
            task("apply-fix", TaskStatus::Pending),
            task("manual-review", TaskStatus::AwaitingTrigger),
            task("publish", TaskStatus::Blocked),
            task("cleanup", TaskStatus::Completed),
            task("report", TaskStatus::Failed),
            task("noop", TaskStatus::WontDo),
        ]);

        assert_eq!(summary.running, 2);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.awaiting_trigger, 1);
        assert_eq!(summary.blocked, 1);
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.wont_do, 1);
        assert_eq!(summary.active_nodes, vec!["scan-js".to_string()]);
    }

    #[test]
    fn format_summary_suffix_includes_active_node_preview() {
        let summary = summarize_tasks(&[
            task("a", TaskStatus::Running),
            task("b", TaskStatus::Running),
            task("c", TaskStatus::Running),
            task("d", TaskStatus::Running),
            task("pending", TaskStatus::Pending),
        ]);

        let suffix = format_summary_suffix(&summary);
        assert!(suffix.contains("4 running"));
        assert!(suffix.contains("1 pending"));
        assert!(suffix.contains("active nodes: a, b, c (+1 more)"));
    }

    #[test]
    fn manual_pull_request_only_node_does_not_require_tui() {
        let workflow = Workflow {
            version: "1".to_string(),
            state: None,
            params: None,
            templates: vec![],
            nodes: vec![Node {
                id: "publish".to_string(),
                name: "Publish".to_string(),
                description: None,
                r#type: NodeType::Manual,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
                branch_name: Some("codemod-test".to_string()),
                pull_request: Some(PullRequestConfig {
                    title: "Test PR".to_string(),
                    body: None,
                    draft: Some(true),
                    base: None,
                }),
            }],
        };

        assert!(!node_requires_manual_tui(&workflow.nodes[0]));
        assert!(!workflow_has_manual_steps(&workflow));
    }

    #[test]
    fn manual_node_with_steps_still_requires_tui() {
        let workflow = Workflow {
            version: "1".to_string(),
            state: None,
            params: None,
            templates: vec![],
            nodes: vec![Node {
                id: "review".to_string(),
                name: "Review".to_string(),
                description: None,
                r#type: NodeType::Manual,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![Step {
                    id: None,
                    name: "noop".to_string(),
                    action: StepAction::RunScript("echo hi".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: Some("codemod-test".to_string()),
                pull_request: Some(PullRequestConfig {
                    title: "Test PR".to_string(),
                    body: None,
                    draft: Some(true),
                    base: None,
                }),
            }],
        };

        assert!(node_requires_manual_tui(&workflow.nodes[0]));
        assert!(workflow_has_manual_steps(&workflow));
    }

    #[test]
    fn manual_gate_without_steps_still_requires_tui_when_not_pr_only() {
        let workflow = Workflow {
            version: "1".to_string(),
            state: None,
            params: None,
            templates: vec![],
            nodes: vec![Node {
                id: "checkpoint".to_string(),
                name: "Checkpoint".to_string(),
                description: None,
                r#type: NodeType::Manual,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            }],
        };

        assert!(node_requires_manual_tui(&workflow.nodes[0]));
        assert!(workflow_has_manual_steps(&workflow));
    }
}
