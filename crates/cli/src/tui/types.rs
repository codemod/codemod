use clap::Args;
use uuid::Uuid;

#[derive(Args, Debug)]
pub struct Command {
    /// Number of workflow runs to show
    #[arg(short, long, default_value = "25")]
    pub limit: usize,

    /// Auto-refresh interval in seconds (0 to disable)
    #[arg(long, default_value = "1")]
    pub refresh_interval: u64,

    /// Dry run mode - don't make actual changes
    #[arg(long)]
    pub dry_run: bool,

    /// Allow fs access
    #[arg(long)]
    pub allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    pub allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    pub allow_child_process: bool,
}

/// Current screen in the step-by-step flow
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum Screen {
    /// Step 1: List of workflow runs
    Workflows,
    /// Step 2: Tasks/Nodes for selected workflow
    Tasks,
    /// Step 3: Actions (triggers, logs, details) for selected task
    Actions,
    /// Terminal view for running task execution
    Terminal,
}

/// Trigger action type
#[derive(Debug, Clone)]
pub enum TriggerAction {
    All,
    Single(Uuid),
}

/// Popup dialog type
#[derive(Debug)]
pub enum Popup {
    None,
    ConfirmCancel(Uuid),
    ConfirmTrigger(TriggerAction),
    ConfirmQuit,
    StatusMessage(String, std::time::Instant),
    Error(String),
    Help,
}
