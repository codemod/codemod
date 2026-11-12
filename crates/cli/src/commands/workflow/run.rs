use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use butterflow_core::report::{convert_diffs, convert_metrics, ExecutionReport};
use butterflow_core::structured_log::OutputFormat;
use butterflow_core::utils;
use butterflow_core::utils::generate_execution_id;
use clap::Args;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_telemetry::send_event::BaseEvent;

use crate::commands::TelemetrySenderExt;
use crate::engine::create_registry_client;
use crate::pro_dry_run::{notify_pro_dry_run_required, ProDryRunReason};
use crate::utils::path_safety::normalize_target_path;
use crate::utils::resolve_capabilities::{
    prompt_capabilities, resolve_capabilities, ResolveCapabilitiesArgs,
};
use crate::utils::workflow_source::resolve_workflow_source_with_name;
use crate::workflow_exec::{self, ExecRequest, ExecSettings};
use crate::{TelemetrySenderMutex, CLI_VERSION};

#[derive(Args, Debug)]
pub struct Command {
    /// Path to a workflow file (workflow.ts or workflow.yaml) or a directory
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

    /// Give every approval the workflow asks for (manual steps, pull requests)
    #[arg(long)]
    approve: bool,

    /// Let branch, commit and pull request steps change the git repository
    #[arg(long)]
    managed_git: bool,

    /// Coding agent to use for AI steps (e.g. claude, codex, aider)
    #[arg(long)]
    agent: Option<String>,

    /// Execute install-skill steps when running in non-interactive mode
    #[arg(long)]
    install_skill: bool,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,

    /// Open a web-based execution report after the run completes
    #[arg(long)]
    report: bool,

    /// Output format: "text" (default) or "jsonl" for structured logging
    #[arg(long, default_value = "text")]
    format: String,

    /// Name of the workflow to run when codemod.yaml defines multiple workflows
    #[arg(long = "workflow-name", value_name = "NAME")]
    workflow_name: Option<String>,
}

/// Capabilities granted with `--allow-*` flags.
pub(crate) fn cli_granted_capabilities(
    allow_fs: bool,
    allow_fetch: bool,
    allow_child_process: bool,
) -> HashSet<LlrtSupportedModules> {
    let mut granted = HashSet::new();
    if allow_fs {
        granted.insert(LlrtSupportedModules::Fs);
    }
    if allow_fetch {
        granted.insert(LlrtSupportedModules::Fetch);
    }
    if allow_child_process {
        granted.insert(LlrtSupportedModules::ChildProcess);
    }
    granted
}

/// A workflow.yaml codemod depends on, if any, that may only run as a dry run.
pub(crate) async fn dry_run_only_dependency(
    workflow_path: &Path,
    registry: &butterflow_core::registry::RegistryClient,
) -> Result<Option<String>> {
    if codemod_workflow::is_typescript(workflow_path) {
        return Ok(None);
    }
    let definition = utils::parse_workflow_file(workflow_path)
        .with_context(|| format!("Failed to parse workflow file: {}", workflow_path.display()))?;
    utils::find_dry_run_only_codemod_dependency(&definition, workflow_path.parent(), registry)
        .await
        .context("Failed to inspect bundled codemods")
}

/// Run a workflow
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    if args.no_color {
        console::set_colors_enabled(false);
    }
    let format: OutputFormat = args
        .format
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;

    let (workflow_path, workflow_dir) =
        resolve_workflow_source_with_name(&args.workflow, args.workflow_name.as_deref())?;
    let params = utils::parse_params(&args.params).context("Failed to parse parameters")?;
    let target_path = normalize_target_path(
        args.target_path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap()),
    )?;
    let workflow_label = workflow_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| args.workflow.clone());

    let registry = create_registry_client(None)?;
    let pro_dependency = dry_run_only_dependency(&workflow_path, &registry).await?;
    if let Some(source) = pro_dependency.as_deref() {
        if !args.dry_run {
            notify_pro_dry_run_required(
                ProDryRunReason::BundledChild { source },
                args.no_interactive,
            );
        }
    }
    let pro_dry_run = pro_dependency.is_some();
    let dry_run = args.dry_run || pro_dry_run;

    let granted =
        cli_granted_capabilities(args.allow_fs, args.allow_fetch, args.allow_child_process);
    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        None,
        Some(workflow_dir.clone()),
    );
    let capabilities = prompt_capabilities(capabilities, &granted, args.no_interactive, dry_run);

    if !dry_run {
        crate::dirty_git_check::dirty_check(args.no_interactive)(
            &target_path,
            args.allow_dirty,
            None,
        )?;
    }

    let execution_id = generate_execution_id();
    let nested_observer = crate::commands::run_telemetry::nested_codemod_run_observer(
        telemetry.clone(),
        execution_id.clone(),
        workflow_label.clone(),
        dry_run,
    );
    let outcome = workflow_exec::start(ExecRequest {
        workflow: workflow_path,
        target: target_path.clone(),
        params,
        name: Some(workflow_label),
        dry_run,
        settings: ExecSettings {
            no_interactive: args.no_interactive,
            format,
            capabilities,
            agent: args.agent.clone(),
            registry: Some(registry.clone()),
            install_skill: (!args.no_interactive || args.install_skill).then(|| {
                crate::commands::package_skill::create_install_skill_executor(telemetry.clone())
            }),
            approve_all: args.approve || pro_dry_run,
            managed_git: args.managed_git,
            skip_shard_steps: pro_dry_run,
            nested_observer: Some(nested_observer),
        },
    })
    .await?;

    let metrics = outcome.summary.metrics();
    if crate::utils::metrics::should_show_report(
        args.report,
        args.no_interactive,
        &metrics,
        outcome.files_modified(),
    ) {
        let report = ExecutionReport::build(
            args.workflow.clone(),
            None,
            outcome.duration.as_millis() as f64,
            dry_run,
            target_path.display().to_string(),
            CLI_VERSION.to_string(),
            outcome.files_modified(),
            outcome.files_unmodified(),
            outcome.files_with_errors(),
            convert_metrics(&metrics),
            convert_diffs(&outcome.summary.diffs(), &target_path.display().to_string()),
        )
        .with_registry_link_url(Some(
            crate::utils::registry_link::registry_link_url_for_local_run(
                &registry.config.default_registry,
            ),
        ));
        crate::report_server::serve_report(report).await?;
    } else if format == OutputFormat::Text {
        crate::utils::metrics::print_metrics(&metrics);
    }

    telemetry
        .send_event_logged(
            BaseEvent {
                kind: "localWorkflowExecuted".to_string(),
                properties: HashMap::from([
                    ("executionId".to_string(), execution_id),
                    (
                        "runTimeSeconds".to_string(),
                        outcome.duration.as_secs_f64().to_string(),
                    ),
                    ("dirtyRun".to_string(), args.allow_dirty.to_string()),
                    ("dryRun".to_string(), dry_run.to_string()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;

    outcome.result()
}
