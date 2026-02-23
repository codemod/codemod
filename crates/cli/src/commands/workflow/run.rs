use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::{TelemetrySenderMutex, CLI_VERSION};
use anyhow::{Context, Result};
use butterflow_core::diff::FileDiff;
use butterflow_core::report::{convert_diffs, convert_metrics, ExecutionReport};
use butterflow_core::utils;
use butterflow_core::utils::generate_execution_id;
use clap::Args;
use codemod_telemetry::send_event::BaseEvent;
use std::sync::atomic::Ordering;

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

    /// No interactive mode
    #[arg(long)]
    no_interactive: bool,

    /// Disable colored diff output in dry-run mode
    #[arg(long)]
    no_color: bool,

    /// Open a web-based execution report after the run completes
    #[arg(long)]
    report: bool,
}

/// Run a workflow
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    // Resolve workflow file and bundle path
    let (workflow_file_path, _) = resolve_workflow_source(&args.workflow)?;

    // Parse parameters
    let params = utils::parse_params(&args.params).context("Failed to parse parameters")?;
    let workflow_dir = workflow_file_path.parent().unwrap();

    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        None,
        Some(workflow_dir.to_path_buf()),
    );

    let target_path = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    // Always collect diffs so we can offer report interactively
    let diff_collector = Some(Arc::new(Mutex::new(Vec::<FileDiff>::new())));

    let started = std::time::Instant::now();

    let (engine, config) = create_engine(
        workflow_file_path,
        target_path.clone(),
        args.dry_run,
        args.allow_dirty,
        params,
        None,
        Some(capabilities),
        args.no_interactive,
        args.no_color,
        diff_collector.clone(),
    )?;

    // Run workflow using the extracted workflow runner
    let (_, seconds) = run_workflow(&engine, config).await?;

    let duration_ms = started.elapsed().as_millis() as f64;

    let metrics_data = engine.metrics_context.get_all();

    if crate::utils::metrics::should_show_report(args.report, args.no_interactive, &metrics_data) {
        let collected_diffs = diff_collector
            .map(|c| c.lock().unwrap().clone())
            .unwrap_or_default();

        let stats = engine.execution_stats.clone();
        let files_modified = stats.files_modified.load(Ordering::Relaxed);
        let files_unmodified = stats.files_unmodified.load(Ordering::Relaxed);
        let files_with_errors = stats.files_with_errors.load(Ordering::Relaxed);

        let report = ExecutionReport::build(
            args.workflow.clone(),
            None,
            duration_ms,
            args.dry_run,
            target_path.display().to_string(),
            CLI_VERSION.to_string(),
            files_modified,
            files_unmodified,
            files_with_errors,
            convert_metrics(&metrics_data),
            convert_diffs(&collected_diffs, &target_path.display().to_string()),
        );

        crate::report_server::serve_report(report).await?;
    } else {
        crate::utils::metrics::print_metrics(&metrics_data);
    }

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
