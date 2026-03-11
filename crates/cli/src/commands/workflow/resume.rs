use std::collections::HashMap;
use std::path::PathBuf;

use crate::engine::create_engine;
use crate::utils::path_safety::normalize_target_path;
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::workflow_runner::resolve_workflow_source;
use crate::TelemetrySenderMutex;
use anyhow::{Context, Result};
use butterflow_models::{Task, TaskStatus, WorkflowStatus};
use clap::Args;
use log::{error, info};
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::Table;
use uuid::Uuid;

use super::status::TaskRow;

#[derive(Args, Debug)]
pub struct Command {
    /// Path to workflow file or directory
    #[arg(short, long, value_name = "PATH")]
    workflow: String,

    /// Workflow run ID
    #[arg(short, long)]
    id: Uuid,

    /// Task ID to trigger (can be specified multiple times)
    #[arg(long = "tasks_ids")]
    task: Vec<Uuid>,

    /// Trigger all awaiting tasks
    #[arg(long)]
    trigger_all: bool,

    /// Allow dirty git status
    #[arg(long)]
    allow_dirty: bool,

    /// Dry run mode - don't make actual changes
    #[arg(long)]
    dry_run: bool,

    /// Optional target path to run the codemod on (default: current directory)
    #[arg(long = "target", short = 't')]
    target_path: Option<PathBuf>,

    /// Allow fs access
    #[arg(long)]
    allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    allow_child_process: bool,

    /// No interactive mode
    #[arg(long)]
    no_interactive: bool,

    /// Execute install-skill steps when running in non-interactive mode
    #[arg(long)]
    install_skill: bool,

    /// Output format: "text" (default) or "jsonl" for structured logging
    #[arg(long, default_value = "text")]
    format: String,

    /// Exit once the triggered tasks reach a terminal state.
    #[arg(long, hide = true)]
    exit_when_triggered_tasks_finish: bool,
}

/// Resume a workflow
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    let target_path = normalize_target_path(
        args.target_path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap()),
    )?;

    let (workflow_file_path, _) = resolve_workflow_source(&args.workflow)?;

    let workflow_dir = workflow_file_path.parent().unwrap();

    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        None,
        Some(workflow_dir.to_path_buf()),
    );
    println!(
        "Resuming workflow {} with capabilities: {:?}",
        args.id, capabilities
    );
    info!(
        "workflow resume invoked: workflow={}, id={}, target={}, trigger_all={}, task_count={}, dry_run={}, no_interactive={}",
        args.workflow,
        args.id,
        target_path.display(),
        args.trigger_all,
        args.task.len(),
        args.dry_run,
        args.no_interactive
    );

    let output_format: butterflow_core::structured_log::OutputFormat = args
        .format
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;

    let (engine, _) = create_engine(
        workflow_file_path,
        target_path,
        args.dry_run,
        args.allow_dirty,
        // TODO: Load params from workflow run
        HashMap::new(),
        None,
        Some(capabilities),
        args.no_interactive,
        false,
        None,
        args.no_interactive && !args.install_skill,
        output_format,
        None,
        None,
        Some(crate::commands::package_skill::create_install_skill_executor(telemetry)),
    )?;

    let tracked_task_ids = if args.trigger_all {
        engine
            .get_tasks(args.id)
            .await
            .context("Failed to load tasks before trigger-all")?
            .into_iter()
            .filter(|task| !task.is_master && task.status == TaskStatus::AwaitingTrigger)
            .map(|task| task.id)
            .collect::<Vec<_>>()
    } else {
        args.task.clone()
    };

    if args.trigger_all {
        info!("Triggering all awaiting tasks for workflow {}", args.id);
        // Trigger all awaiting tasks
        let triggered = engine
            .trigger_all(args.id)
            .await
            .context("Failed to trigger all tasks")?;
        if !triggered {
            println!("No tasks awaiting trigger");
            return Ok(());
        }

        println!("Triggered all awaiting tasks");
    } else if !args.task.is_empty() {
        info!(
            "Triggering specific tasks for workflow {}: {:?}",
            args.id, args.task
        );
        // Trigger specific tasks
        engine
            .resume_workflow(args.id, args.task.to_vec())
            .await
            .context("Failed to resume workflow")?;

        println!("Triggered {} tasks", args.task.len());
    } else {
        error!("No tasks specified to trigger. Use --task or --trigger-all");
        return Ok(());
    }

    if args.exit_when_triggered_tasks_finish {
        wait_for_triggered_tasks(&engine, args.id, &tracked_task_ids).await?;
        return Ok(());
    }

    // Wait for workflow to complete or pause again
    let mut poll_count = 0u64;
    loop {
        // Get workflow status
        let status = engine
            .get_workflow_status(args.id)
            .await
            .context("Failed to get workflow status")?;
        poll_count += 1;
        if poll_count == 1 || poll_count % 5 == 0 {
            info!(
                "workflow resume poll {}: workflow {} status {:?}",
                poll_count, args.id, status
            );
        }

        match status {
            WorkflowStatus::Completed => {
                println!("✅ Workflow completed successfully");
                break;
            }
            WorkflowStatus::Failed => {
                println!("❌ Workflow failed");
                break;
            }
            WorkflowStatus::AwaitingTrigger => {
                // Get tasks awaiting trigger
                let tasks = engine
                    .get_tasks(args.id)
                    .await
                    .context("Failed to get tasks")?;

                let awaiting_tasks: Vec<&Task> = tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::AwaitingTrigger)
                    .collect();

                println!("⏸️ Workflow paused: Manual triggers still required");
                println!("Workflow is still awaiting manual triggers for the following tasks:");
                let mut tasks_table = Table::new(awaiting_tasks.iter().map(|t| TaskRow {
                    id: t.id.to_string(),
                    node_id: t.node_id.clone(),
                    status: format!("{:?}", t.status),
                    matrix_info: "-".to_string(),
                }));

                tasks_table
                    .with(Style::rounded())
                    .with(Modify::new(Columns::new(..)).with(Alignment::left())); // align all columns left
                println!("Tasks:");
                println!("{tasks_table}");

                break;
            }
            WorkflowStatus::Canceled => {
                println!("❌ Workflow was canceled");
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

fn is_terminal_task_status(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::WontDo
    )
}

async fn wait_for_triggered_tasks(
    engine: &butterflow_core::engine::Engine,
    workflow_run_id: Uuid,
    tracked_task_ids: &[Uuid],
) -> Result<()> {
    if tracked_task_ids.is_empty() {
        return Ok(());
    }

    let mut poll_count = 0u64;
    loop {
        let tasks = engine
            .get_tasks(workflow_run_id)
            .await
            .context("Failed to get tasks while waiting for triggered tasks")?;
        let tracked_tasks = tasks
            .iter()
            .filter(|task| tracked_task_ids.contains(&task.id))
            .collect::<Vec<_>>();

        poll_count += 1;
        if poll_count == 1 || poll_count % 5 == 0 {
            info!(
                "triggered-task poll {}: workflow {} tracked={} matched={} statuses={:?}",
                poll_count,
                workflow_run_id,
                tracked_task_ids.len(),
                tracked_tasks.len(),
                tracked_tasks
                    .iter()
                    .map(|task| (task.id, task.status))
                    .collect::<Vec<_>>()
            );
        }

        if tracked_tasks.len() == tracked_task_ids.len()
            && tracked_tasks
                .iter()
                .all(|task| is_terminal_task_status(task.status))
        {
            info!(
                "All triggered tasks reached terminal state for workflow {}: {:?}",
                workflow_run_id, tracked_task_ids
            );
            break;
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    Ok(())
}
