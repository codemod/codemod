use anyhow::Result;
use clap::Args;
use uuid::Uuid;

use crate::engine::create_engine;
use crate::tui;

#[derive(Args, Debug)]
pub struct Command {
    /// Go directly to the task list for a specific workflow run ID
    #[arg(short, long)]
    id: Option<Uuid>,

    /// Maximum number of workflow runs to display
    #[arg(short, long, default_value = "20")]
    limit: usize,
}

pub async fn handler(args: &Command) -> Result<()> {
    let target_path = std::env::current_dir()?;

    // Use a minimal engine config for browsing -- no workflow file needed
    // We create a dummy config since we just need the state adapter
    let (engine, _config) = create_engine(
        target_path.join("workflow.yaml"), // dummy path
        target_path,
        false,
        true, // allow dirty -- we're just browsing
        std::collections::HashMap::new(),
        None,
        None,
        true, // no_interactive
        false,
        None,
        false,
        butterflow_core::structured_log::OutputFormat::Text,
        None,
        None,
        None,
    )?;

    if let Some(workflow_run_id) = args.id {
        tui::run_tui_for_run(engine, workflow_run_id).await
    } else {
        tui::run_tui(engine, args.limit).await
    }
}
