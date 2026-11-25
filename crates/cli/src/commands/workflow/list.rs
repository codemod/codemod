use anyhow::Result;
use clap::Args;
use tabled::{Table, Tabled};

use super::status::{run_json, ListFormat};
use crate::workflow_exec::{status_label, store};

#[derive(Args, Debug)]
pub struct Command {
    /// Number of workflow runs to show
    #[arg(short, long, default_value = "10")]
    limit: usize,

    /// Output format: "text" (default), or "json": an array of runs, without their steps
    #[arg(long, value_enum, default_value_t = ListFormat::Text)]
    format: ListFormat,
}

#[derive(Tabled)]
struct RunRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "Name")]
    pub name: String,
    #[tabled(rename = "Status")]
    pub status: String,
    #[tabled(rename = "Started")]
    pub started_at: String,
}

/// List workflow runs, most recent first
pub async fn handler(args: &Command) -> Result<()> {
    let store = store();
    let runs = store.list_runs(args.limit)?;
    if args.format == ListFormat::Json {
        let runs: Vec<serde_json::Value> = runs
            .iter()
            .map(|run| run_json(store.as_ref(), run, None))
            .collect();
        println!("{}", serde_json::Value::from(runs));
        return Ok(());
    }
    if runs.is_empty() {
        println!("No workflow runs found.");
        return Ok(());
    }
    let rows: Vec<RunRow> = runs
        .into_iter()
        .map(|run| RunRow {
            name: run.name.clone().unwrap_or_else(|| {
                run.workflow_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
            status: status_label(store.as_ref(), &run),
            id: run.id,
            started_at: run.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        })
        .collect();
    println!("{}", Table::new(rows));
    Ok(())
}
