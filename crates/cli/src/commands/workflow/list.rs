use anyhow::{Context, Result};
use butterflow_core::utils;
use clap::Args;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

use crate::engine::create_engine;

#[derive(Args, Debug)]
pub struct Command {
    /// Number of workflow runs to show
    #[arg(short, long, default_value = "10")]
    limit: usize,
}

#[derive(Tabled)]
pub struct WorkflowRunRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "Name")]
    pub name: String,
    #[tabled(rename = "Status")]
    pub status: String,
    #[tabled(rename = "Started At")]
    pub started_at: String,
    #[tabled(rename = "Ended At")]
    pub ended_at: String,
    #[tabled(rename = "Duration")]
    pub duration: String,
}

/// List workflow runs
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
        None,
    )?;

    // Get workflow runs
    let workflow_runs = engine
        .list_workflow_runs(args.limit)
        .await
        .context("Failed to list workflow runs")?;

    if workflow_runs.is_empty() {
        println!("No workflow runs found");
        return Ok(());
    }

    println!("Recent workflow runs:");
    let rows: Vec<WorkflowRunRow> = workflow_runs
        .iter()
        .map(|workflow_run| WorkflowRunRow {
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
                            .num_seconds()
                    })
                    .unwrap_or(0) as u64,
            ),
        })
        .collect();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left())); // align all columns left
    println!("{table}");

    Ok(())
}
