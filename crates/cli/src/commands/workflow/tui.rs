use anyhow::Result;
use clap::Args;
use uuid::Uuid;

use crate::engine::create_engine;
use crate::tui::run_workflow_tui;

#[derive(Args, Debug)]
pub struct Command {
    /// Existing workflow run ID to attach to
    #[arg(short, long)]
    pub id: Option<Uuid>,

    /// Number of workflow runs to show in the browser
    #[arg(short, long, default_value = "20")]
    pub limit: usize,
}

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
        false,
        Default::default(),
        None,
        None,
        None,
    )?;

    run_workflow_tui(engine, args.id, args.limit).await
}
