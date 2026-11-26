use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::engine::create_registry_client;
use crate::workflow_exec::{self, ExecSettings, RunFlags};
use crate::TelemetrySenderMutex;

/// What `--approve` alone stands for: every approval.
const ALL: &str = "*";

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: String,

    /// Give an approval: the one with this question id (e.g. approval:3, as `workflow status`
    /// shows it). Without an id, give the one the run is waiting for and every later one.
    /// Repeat it to give several
    #[arg(
        long,
        value_name = "QUESTION_ID",
        num_args = 0..=1,
        default_missing_value = ALL,
        action = clap::ArgAction::Append
    )]
    approve: Vec<String>,

    /// Same as --approve (kept for existing scripts)
    #[arg(long, hide = true)]
    trigger_all: bool,

    /// Continue a run that was canceled or rejected, or one another codemod version made whose
    /// finished function steps would run again
    #[arg(long)]
    force: bool,

    /// Let branch, commit and pull request steps change the git repository (experimental)
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

    /// Write even if the target has uncommitted changes, for a run that stopped before it first
    /// wrote (a run that already wrote is not checked again)
    #[arg(long)]
    allow_dirty: bool,

    /// Removed in 2.0 (refused with what replaced it): approvals have question ids
    #[arg(long = "tasks_ids", value_name = "TASK", hide = true, action = clap::ArgAction::Append)]
    tasks_ids: Vec<String>,

    /// Removed in 2.0 (refused with what replaced it): dry runs are not saved
    #[arg(long, hide = true)]
    dry_run: bool,
}

/// Why a 1.x flag `resume` no longer takes is refused, and what to use.
fn removed_flag(args: &Command) -> Option<String> {
    if !args.tasks_ids.is_empty() {
        return Some(format!(
            "--tasks_ids was removed in codemod 2.0. Approvals now have question ids: see them with `codemod workflow status --id {id}`, then resume with `--approve <question-id>` (or `--approve` to give every approval).",
            id = args.id
        ));
    }
    if args.dry_run {
        return Some(
            "--dry-run was removed from resume in codemod 2.0: a dry run is not saved, so there is nothing to resume. Start a new one with `codemod workflow run --dry-run`."
                .to_string(),
        );
    }
    None
}

/// Resume a failed, paused or waiting workflow run
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    if let Some(message) = removed_flag(args) {
        anyhow::bail!(message);
    }
    let approve_all = args.trigger_all || args.approve.iter().any(|id| id == ALL);
    let approve_ids = args
        .approve
        .iter()
        .filter(|id| *id != ALL)
        .cloned()
        .collect();
    args.flags.warn_deprecated_capabilities();
    let outcome = workflow_exec::resume(
        &args.id,
        ExecSettings {
            approve_all,
            approve_ids,
            managed_git: args.managed_git,
            force: args.force,
            allow_dirty: args.allow_dirty,
            ..args.flags.settings(
                std::collections::HashSet::new(),
                create_registry_client(None)?,
                &telemetry,
            )
        },
    )
    .await?;
    outcome.result()
}
