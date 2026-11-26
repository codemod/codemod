use anyhow::Result;
use clap::Args;
use tabled::{Table, Tabled};

use butterflow_state::run_store::{RunRecord, UnreadableRun};
use serde_json::json;

use super::status::{run_json, ListFormat};
use crate::workflow_exec::{status_label, store, unreadable_label};

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

/// List workflow runs, most recent first. Runs this version cannot read
/// are listed too, as unreadable, with the version that made them.
pub async fn handler(args: &Command) -> Result<()> {
    let store = store();
    let runs = store.list_runs(args.limit)?;
    let unreadable = store.list_unreadable().unwrap_or_default();
    // Both, most recent first, `limit` in all.
    let mut entries: Vec<Entry> = runs
        .into_iter()
        .map(|run| Entry::Run(Box::new(run)))
        .chain(unreadable.into_iter().map(Entry::Unreadable))
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.started_at()));
    entries.truncate(args.limit);
    if args.format == ListFormat::Json {
        let runs: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| match entry {
                Entry::Run(run) => {
                    let steps = store.load_steps(&run.id).unwrap_or_default();
                    run_json(store.as_ref(), run, &steps, false)
                }
                Entry::Unreadable(run) => json!({
                    "id": run.id,
                    "status": "unreadable",
                    "startedAt": run.started_at.map(|at| at.to_rfc3339()),
                    "codemodVersion": run.codemod_version,
                    "error": run.error,
                }),
            })
            .collect();
        println!("{}", serde_json::Value::from(runs));
        return Ok(());
    }
    if entries.is_empty() {
        println!("No workflow runs found.");
        return Ok(());
    }
    let rows: Vec<RunRow> = entries
        .into_iter()
        .map(|entry| match entry {
            Entry::Run(run) => RunRow {
                name: run.name.clone().unwrap_or_else(|| {
                    run.workflow_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                status: status_label(store.as_ref(), &run),
                id: run.id,
                started_at: run.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            },
            Entry::Unreadable(run) => RunRow {
                name: String::new(),
                status: unreadable_label(&run),
                id: run.id,
                started_at: run
                    .started_at
                    .map(|at| at.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_default(),
            },
        })
        .collect();
    println!("{}", Table::new(rows));
    Ok(())
}

enum Entry {
    Run(Box<RunRecord>),
    Unreadable(UnreadableRun),
}

impl Entry {
    fn started_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Entry::Run(run) => Some(run.started_at),
            Entry::Unreadable(run) => run.started_at,
        }
    }
}
