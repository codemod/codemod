use std::collections::HashMap;
use std::path::PathBuf;

use crate::{TelemetrySenderMutex, CLI_VERSION};
use anyhow::{Context, Result};
use butterflow_core::utils;
use butterflow_core::utils::generate_execution_id;
use clap::Args;
use codemod_telemetry::send_event::BaseEvent;

use crate::engine::create_engine;
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

    /// Allow fs access
    #[arg(long)]
    allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    allow_child_process: bool,
}

/// Run a workflow
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    // Resolve workflow file and bundle path
    let (workflow_file_path, _) = resolve_workflow_source(&args.workflow)?;

    // Parse parameters
    let mut params = utils::parse_params(&args.params).context("Failed to parse parameters")?;

    let mut capabilities = Vec::new();
    if args.allow_fs {
        capabilities.push("fs".to_string());
    }
    if args.allow_fetch {
        capabilities.push("fetch".to_string());
    }
    if args.allow_child_process {
        capabilities.push("child_process".to_string());
    }
    if !capabilities.is_empty() {
        params.insert("capabilities".to_string(), capabilities.join(","));
    }

    let target_path = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    let (engine, config) = create_engine(
        workflow_file_path,
        target_path,
        args.dry_run,
        args.allow_dirty,
        params,
        None,
    )?;

    // Run workflow using the extracted workflow runner
    let (_, seconds) = run_workflow(&engine, config).await?;

    // Generate a 20-byte execution ID (160 bits of entropy for collision resistance)
    telemetry
        .send_event(
            BaseEvent {
                kind: "localWorkflowExecuted".to_string(),
                properties: HashMap::from([
                    ("executionId".to_string(), generate_execution_id()),
                    ("runTimeSeconds".to_string(), seconds.to_string()),
                    ("dirtyRun".to_string(), args.allow_dirty.to_string()),
                    ("dryRun".to_string(), args.dry_run.to_string()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;

    Ok(())
}
