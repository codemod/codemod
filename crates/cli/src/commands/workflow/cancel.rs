use anyhow::Result;
use clap::Args;
use codemod_workflow::{cancel_run, Canceled};

use crate::workflow_exec::store;

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: String,
}

/// Cancel a workflow run. A run in progress (in this or another terminal)
/// stops after the step it is on; any other unfinished run is marked
/// canceled.
pub async fn handler(args: &Command) -> Result<()> {
    match cancel_run(store().as_ref(), &args.id)? {
        Canceled::Requested => println!(
            "Asked run {} to stop; it stops after the step it is on.",
            args.id
        ),
        Canceled::Marked => println!("Run {} canceled.", args.id),
        Canceled::AlreadyFinished(status) => {
            println!("Run {} already {}.", args.id, status.as_str())
        }
    }
    Ok(())
}
