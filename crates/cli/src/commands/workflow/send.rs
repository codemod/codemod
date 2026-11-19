use anyhow::{Context, Result};
use clap::Args;
use codemod_workflow::send_event;

use crate::workflow_exec::store;

#[derive(Args, Debug)]
pub struct Command {
    /// The event's name, as the workflow's `waitForEvent(name)` has it
    event: String,

    /// Workflow run ID
    #[arg(short, long)]
    id: String,

    /// What the waiting step resolves to, as JSON (default: null)
    #[arg(long)]
    data: Option<String>,
}

/// Send an event to a workflow run. A step waiting for it continues; if no
/// step waits yet, the event is kept until one does.
pub async fn handler(args: &Command) -> Result<()> {
    let data = match &args.data {
        Some(text) => serde_json::from_str(text).context("--data must be JSON")?,
        None => serde_json::Value::Null,
    };
    send_event(store().as_ref(), &args.id, &args.event, &data)?;
    println!("Sent '{}' to run {}.", args.event, args.id);
    Ok(())
}
