use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::utils::resolve_capabilities::{
    prompt_capabilities, resolve_capabilities, ResolveCapabilitiesArgs,
};
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
#[cfg(unix)]
use crate::workflow_runner::{run_workflow_with_tui, workflow_has_manual_nodes};

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

    /// Coding agent to use for AI steps (e.g. claude, codex, aider)
    #[arg(long)]
    agent: Option<String>,
    /// Execute install-skill steps when running in non-interactive mode
    #[arg(long)]
    install_skill: bool,

    /// Disable colored diff output in dry-run mode
    #[arg(long)]
    no_color: bool,

    /// Open a web-based execution report after the run completes
    #[arg(long)]
    report: bool,

    /// Output format: "text" (default) or "jsonl" for structured logging
    #[arg(long, default_value = "text")]
    format: String,
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

    // Build set of capabilities explicitly granted via CLI flags (skip prompting for these)
    let mut cli_granted = std::collections::HashSet::new();
    if args.allow_fs {
        cli_granted.insert(codemod_llrt_capabilities::types::LlrtSupportedModules::Fs);
    }
    if args.allow_fetch {
        cli_granted.insert(codemod_llrt_capabilities::types::LlrtSupportedModules::Fetch);
    }
    if args.allow_child_process {
        cli_granted.insert(codemod_llrt_capabilities::types::LlrtSupportedModules::ChildProcess);
    }

    let capabilities = prompt_capabilities(capabilities, &cli_granted, args.no_interactive);

    let target_path = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    // Always collect diffs so we can offer report interactively
    let diff_collector = Some(Arc::new(Mutex::new(Vec::<FileDiff>::new())));

    let started = std::time::Instant::now();

    let output_format: butterflow_core::structured_log::OutputFormat = args
        .format
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;

    let (mut engine, mut config) = create_engine(
        workflow_file_path,
        target_path.clone(),
        args.dry_run,
        args.allow_dirty,
        params,
        None,
        Some(capabilities.clone()),
        args.no_interactive,
        args.no_color,
        diff_collector.clone(),
        args.no_interactive && !args.install_skill,
        output_format,
        Some(capabilities),
        args.agent.clone(),
        Some(crate::commands::package_skill::create_install_skill_executor(telemetry.clone())),
    )?;

    // Set the workflow name so it's stored on the WorkflowRun for TUI display
    engine.set_name(Some(args.workflow.clone()));

    // Check if workflow has manual nodes and should launch TUI (Unix only)
    #[cfg(unix)]
    let use_tui = {
        let workflow =
            butterflow_core::utils::parse_workflow_file(engine.get_workflow_file_path())?;
        !args.no_interactive && workflow_has_manual_nodes(&workflow)
    };
    #[cfg(not(unix))]
    let use_tui = false;

    if use_tui {
        config.quiet = true;
        config.progress_callback = Arc::new(None);
        engine.set_quiet(true);
    }

    // Run workflow -- with TUI if manual nodes, otherwise text-based
    #[cfg(unix)]
    let (_, seconds) = if use_tui {
        run_workflow_with_tui(&engine, config).await?
    } else {
        run_workflow(&engine, config).await?
    };
    #[cfg(not(unix))]
    let (_, seconds) = run_workflow(&engine, config).await?;

    let duration_ms = started.elapsed().as_millis() as f64;

    let metrics_data = engine.metrics_context.get_all();

    let stats = engine.execution_stats.clone();
    let files_modified = stats.files_modified.load(Ordering::Relaxed);
    let files_unmodified = stats.files_unmodified.load(Ordering::Relaxed);
    let files_with_errors = stats.files_with_errors.load(Ordering::Relaxed);

    if !use_tui {
        if crate::utils::metrics::should_show_report(
            args.report,
            args.no_interactive,
            &metrics_data,
            files_modified,
        ) {
            let collected_diffs = diff_collector
                .map(|c| c.lock().unwrap().clone())
                .unwrap_or_default();

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
