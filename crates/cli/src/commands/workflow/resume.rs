use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::engine::create_registry_client;
use crate::workflow_exec::{self, ExecSettings, RunFlags};
use crate::TelemetrySenderMutex;

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: String,

    /// Give the approval the run is waiting for, and any later ones
    #[arg(long)]
    approve: bool,

    /// Same as --approve (kept for existing scripts)
    #[arg(long, hide = true)]
    trigger_all: bool,

    /// Let branch, commit and pull request steps change the git repository
    #[arg(long)]
    managed_git: bool,

    #[command(flatten)]
    flags: RunFlags,

    /// Ignored: the run remembers its workflow
    #[arg(short, long, hide = true)]
    workflow: Option<String>,

    /// Ignored: the run remembers its target
    #[arg(long = "target", short = 't', hide = true)]
    target_path: Option<PathBuf>,

    /// Ignored: a run checks for uncommitted changes when it starts, not when it resumes
    #[arg(long, hide = true)]
    allow_dirty: bool,
}

/// Resume a failed, canceled or paused workflow run
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    let outcome = workflow_exec::resume(
        &args.id,
        ExecSettings {
            approve_all: args.approve || args.trigger_all,
            managed_git: args.managed_git,
            ..args.flags.settings(
                args.flags.granted(),
                create_registry_client(None)?,
                &telemetry,
            )
        },
    )
    .await?;
    outcome.result()
}
