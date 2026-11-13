use anyhow::{anyhow, Result};
use clap::Args;
use tabled::{Table, Tabled};

use crate::workflow_exec::{status_label, store};

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: String,
}

#[derive(Tabled)]
struct StepRow {
    #[tabled(rename = "#")]
    position: u64,
    #[tabled(rename = "Step")]
    label: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Error")]
    error: String,
}

/// Show a workflow run and its steps
pub async fn handler(args: &Command) -> Result<()> {
    let store = store();
    let run = store
        .get_run(&args.id)?
        .ok_or_else(|| anyhow!("run '{}' not found", args.id))?;
    let steps = store.load_steps(&args.id)?;

    println!("Run:       {}", run.id);
    if let Some(name) = &run.name {
        println!("Name:      {name}");
    }
    println!("Workflow:  {}", run.workflow_path.display());
    println!("Target:    {}", run.target.display());
    println!("Status:    {}", status_label(store.as_ref(), &run));
    println!("Started:   {}", run.started_at.format("%Y-%m-%d %H:%M:%S"));
    if let Some(ended) = run.ended_at {
        println!(
            "Ended:     {} ({})",
            ended.format("%Y-%m-%d %H:%M:%S"),
            humantime(ended - run.started_at)
        );
    }
    if let Some(awaiting) = &run.awaiting {
        println!("Waiting:   {awaiting}");
    }
    if let Some(error) = &run.error {
        println!("Error:     {error}");
    }

    if steps.is_empty() {
        println!("\nNo steps recorded yet.");
        return Ok(());
    }
    let rows: Vec<StepRow> = steps
        .iter()
        .map(|step| StepRow {
            position: step.position,
            label: step.label.clone(),
            status: step.status.as_str().to_string(),
            error: step
                .error
                .as_deref()
                .map(|e| e.lines().next().unwrap_or_default().to_string())
                .unwrap_or_default(),
        })
        .collect();
    println!("\n{}", Table::new(rows));
    Ok(())
}

fn humantime(duration: chrono::TimeDelta) -> String {
    let seconds = duration.num_milliseconds() as f64 / 1000.0;
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{}m {}s", (seconds / 60.0) as u64, (seconds % 60.0) as u64)
    }
}
