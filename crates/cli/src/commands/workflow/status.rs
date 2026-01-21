use anyhow::{Context, Result};
use butterflow_core::utils;
use butterflow_models::{Task, TaskStatus};
use clap::Args;
use std::collections::HashMap;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};
use uuid::Uuid;

use crate::engine::create_engine;

use super::list::WorkflowRunRow;

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: Uuid,
}

#[derive(Tabled)]
pub struct TaskRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "Node ID")]
    pub node_id: String,
    #[tabled(rename = "Status")]
    pub status: String,
    #[tabled(rename = "Matrix Info")]
    pub matrix_info: String,
}

#[derive(Tabled)]
struct ManualTriggerRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Node ID")]
    node_id: String,
    #[tabled(rename = "Matrix Info")]
    matrix_info: String,
}

/// Show workflow status
pub async fn handler(args: &Command) -> Result<()> {
    let (engine, _) = create_engine(
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        None,
        None,
        false,
        false,
    )?;

    // Get workflow run
    let workflow_run = engine
        .get_workflow_run(args.id)
        .await
        .context("Failed to get workflow run")?;

    // Get tasks
    let tasks = engine
        .get_tasks(args.id)
        .await
        .context("Failed to get tasks")?;

    let rows = vec![WorkflowRunRow {
        id: workflow_run.id.to_string(),
        name: workflow_run
            .workflow
            .nodes
            .first()
            .map(|n| n.name.clone())
            .unwrap_or("unknown".to_string()),
        status: format!("{:?}", workflow_run.status),
        started_at: workflow_run.started_at.to_string(),
        ended_at: workflow_run
            .ended_at
            .map(|d| d.to_string())
            .unwrap_or("unknown".to_string()),
        duration: utils::format_duration(
            workflow_run
                .ended_at
                .map(|d| {
                    d.signed_duration_since(workflow_run.started_at)
                        .num_seconds() as u64
                })
                .unwrap_or(0),
        ),
    }];
    let mut table = Table::new(rows);

    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left())); // align all columns left

    println!("{table}");

    // Group tasks by node
    let mut tasks_by_node: HashMap<String, Vec<&Task>> = HashMap::new();
    for task in &tasks {
        tasks_by_node
            .entry(task.node_id.clone())
            .or_default()
            .push(task);
    }
    let mut tasks_rows = Vec::new();

    // Print tasks
    for node in &workflow_run.workflow.nodes {
        let empty_tasks: Vec<&Task> = Vec::new();
        let node_tasks = tasks_by_node.get(&node.id).unwrap_or(&empty_tasks);

        // Find master task for matrix nodes
        let master_task = node_tasks
            .iter()
            .find(|t| t.master_task_id.is_none() && t.matrix_values.is_none());

        if let Some(master_task) = master_task {
            tasks_rows.push(TaskRow {
                id: master_task.id.to_string(),
                node_id: node.id.clone(),
                status: format!("{:?}", master_task.status),
                matrix_info: "-".to_string(),
            });

            // Print matrix tasks
            for task in node_tasks.iter().filter(|t| t.master_task_id.is_some()) {
                let matrix_info = task
                    .matrix_values
                    .as_ref()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| format!("{k}: {v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                tasks_rows.push(TaskRow {
                    id: master_task.id.to_string(),
                    node_id: node.id.clone(),
                    status: format!("{:?}", master_task.status),
                    matrix_info,
                });
            }
        } else if !node_tasks.is_empty() {
            // Print regular task
            let task = node_tasks[0];
            tasks_rows.push(TaskRow {
                id: task.id.to_string(),
                node_id: node.id.clone(),
                status: format!("{:?}", task.status),
                matrix_info: "-".to_string(),
            });
        } else {
            println!("Not found task for node: {}", node.id);
        }
    }

    let mut tasks_table = Table::new(tasks_rows);

    tasks_table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));

    println!("Tasks:");
    println!("{tasks_table}");

    // Print manual triggers
    let awaiting_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::AwaitingTrigger)
        .collect();

    if awaiting_tasks.is_empty() {
        println!("Manual triggers: None");
    } else {
        let manual_trigger_rows: Vec<ManualTriggerRow> = awaiting_tasks
            .iter()
            .map(|t| {
                let node = workflow_run
                    .workflow
                    .nodes
                    .iter()
                    .find(|n| n.id == t.node_id)
                    .unwrap();
                let matrix_info = t
                    .matrix_values
                    .as_ref()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| format!("{k}: {v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_else(|| "-".to_string());

                ManualTriggerRow {
                    id: t.id.to_string(),
                    node_id: node.id.clone(),
                    matrix_info,
                }
            })
            .collect();

        let mut manual_triggers_table = Table::new(manual_trigger_rows);
        manual_triggers_table
            .with(Style::rounded())
            .with(Modify::new(Columns::new(..)).with(Alignment::left())); // align all columns left

        println!("Manual triggers:");
        println!("{manual_triggers_table}");
    }
    Ok(())
}
