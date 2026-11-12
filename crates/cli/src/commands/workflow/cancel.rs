use anyhow::{anyhow, Result};
use butterflow_state::run_store::RunStatus;
use chrono::Utc;
use clap::Args;

use crate::workflow_exec::store;

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: String,
}

/// Cancel a workflow run. A run in progress (in this or another terminal)
/// stops before its next step; a paused run is marked canceled.
pub async fn handler(args: &Command) -> Result<()> {
    let store = store()?;
    let mut run = store
        .get_run(&args.id)?
        .ok_or_else(|| anyhow!("run '{}' not found", args.id))?;
    match run.status {
        RunStatus::Running => {
            store.request_cancel(&args.id)?;
            println!("Asked run {} to stop.", args.id);
        }
        RunStatus::AwaitingApproval | RunStatus::Failed => {
            run.status = RunStatus::Canceled;
            run.awaiting = None;
            run.ended_at = Some(Utc::now());
            store.update_run(&run)?;
            println!("Run {} canceled.", args.id);
        }
        status => println!("Run {} already {}.", args.id, status.as_str()),
    }
    Ok(())
}
