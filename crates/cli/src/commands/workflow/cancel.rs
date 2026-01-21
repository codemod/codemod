use anyhow::{Context, Result};
use clap::Args;
use uuid::Uuid;

use crate::engine::create_engine;

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: Uuid,
}

/// Cancel a workflow
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

    println!("Canceling workflow run {}...", args.id);

    // Cancel workflow
    engine
        .cancel_workflow(args.id)
        .await
        .context("Failed to cancel workflow")?;

    println!("✅ Workflow run canceled successfully");

    Ok(())
}
