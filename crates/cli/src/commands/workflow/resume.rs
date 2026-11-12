use std::path::PathBuf;

use anyhow::Result;
use butterflow_core::structured_log::OutputFormat;
use clap::Args;

use super::run::cli_granted_capabilities;
use crate::engine::create_registry_client;
use crate::workflow_exec::{self, ExecSettings};
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

    /// Allow fs access
    #[arg(long)]
    allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    allow_child_process: bool,

    /// No interactive mode
    #[arg(long)]
    no_interactive: bool,

    /// Coding agent to use for AI steps
    #[arg(long)]
    agent: Option<String>,

    /// Execute install-skill steps when running in non-interactive mode
    #[arg(long)]
    install_skill: bool,

    /// Output format: "text" (default) or "jsonl" for structured logging
    #[arg(long, default_value = "text")]
    format: String,

    /// Ignored: the run remembers its workflow
    #[arg(short, long, hide = true)]
    workflow: Option<String>,

    /// Ignored: the run remembers its target
    #[arg(long = "target", short = 't', hide = true)]
    target_path: Option<PathBuf>,

    /// Ignored: the run remembers whether dirty files were allowed
    #[arg(long, hide = true)]
    allow_dirty: bool,
}

/// Resume a failed, canceled or paused workflow run
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    let format: OutputFormat = args
        .format
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;
    let outcome = workflow_exec::resume(
        &args.id,
        ExecSettings {
            no_interactive: args.no_interactive,
            format,
            capabilities: cli_granted_capabilities(
                args.allow_fs,
                args.allow_fetch,
                args.allow_child_process,
            ),
            agent: args.agent.clone(),
            registry: Some(create_registry_client(None)?),
            install_skill: (!args.no_interactive || args.install_skill).then(|| {
                crate::commands::package_skill::create_install_skill_executor(telemetry.clone())
            }),
            approve_all: args.approve || args.trigger_all,
            managed_git: args.managed_git,
            skip_shard_steps: false,
            nested_observer: None,
        },
    )
    .await?;
    outcome.result()
}
