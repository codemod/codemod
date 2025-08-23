use std::path::PathBuf;

use anyhow::{Context, Result};
use butterflow_core::utils;
use clap::Args;

use crate::engine::{create_download_progress_callback, create_engine};
use crate::workflow_runner::{resolve_workflow_source, run_workflow};

#[derive(Args, Debug)]
pub struct Command {
    /// Path to workflow file or directory
    #[arg(short, long, value_name = "PATH")]
    workflow: String,

    /// Workflow parameters (format: key=value)
    #[arg(long = "param", value_name = "KEY=VALUE")]
    params: Vec<String>,

    /// Allow dirty git status
    #[arg(long)]
    allow_dirty: bool,

    /// Optional target path to run the codemod on (default: current directory)
    #[arg(long = "target", short = 't')]
    target_path: Option<PathBuf>,

    /// Dry run mode - don't make actual changes
    #[arg(long)]
    dry_run: bool,
}

/// Run a workflow
pub async fn handler(args: &Command) -> Result<()> {
    // Resolve workflow file and bundle path
    let (workflow_file_path, _) = resolve_workflow_source(&args.workflow)?;

    // Parse parameters
    let params = utils::parse_params(&args.params).context("Failed to parse parameters")?;

    let target_path = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    let download_progress_callback = create_download_progress_callback();
    let (engine, config) = create_engine(
        workflow_file_path,
        target_path,
        args.dry_run,
        args.allow_dirty,
        params,
        None,
        Some(download_progress_callback),
    )?;

    // Run workflow using the extracted workflow runner
    run_workflow(&engine, config).await?;

    Ok(())
}
