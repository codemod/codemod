use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use butterflow_core::report::{convert_diffs, ExecutionReport};
use butterflow_core::structured_log::OutputFormat;
use butterflow_core::utils;
use butterflow_core::utils::generate_execution_id;
use clap::Args;
use codemod_telemetry::send_event::BaseEvent;
use console::style;

use crate::commands::TelemetrySenderExt;
use crate::engine::create_registry_client;
use crate::pro_dry_run::{notify_pro_dry_run_required, ProDryRunReason};
use crate::utils::path_safety::normalize_target_path;
use crate::utils::workflow_source::resolve_workflow_source_with_name;
use crate::workflow_exec::{self, ExecRequest, ExecSettings, RunFlags, StartFlags};
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

    #[command(flatten)]
    start: StartFlags,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,

    /// Open a web-based execution report after the run completes
    #[arg(long)]
    report: bool,

    /// Name of the workflow to run when codemod.yaml defines multiple workflows
    #[arg(long = "workflow-name", value_name = "NAME")]
    workflow_name: Option<String>,

    #[command(flatten)]
    flags: RunFlags,
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
    utils::validate_codemod_dependencies(&definition, workflow_path.parent(), registry)
        .await
        .context("Invalid codemod dependencies")?;
    utils::find_dry_run_only_codemod_dependency(&definition, workflow_path.parent(), registry)
        .await
        .context("Failed to inspect bundled codemods")
}

/// Run a workflow
pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    if args.no_color {
        console::set_colors_enabled(false);
    }
    let format = args.flags.format;

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

    let is_yaml = !codemod_workflow::is_typescript(&workflow_path);
    let registry = create_registry_client(None)?;
    let pro_dependency = dry_run_only_dependency(&workflow_path, &registry).await?;
    if let Some(source) = pro_dependency.as_deref() {
        if !args.dry_run {
            notify_pro_dry_run_required(
                ProDryRunReason::BundledChild { source },
                args.flags.no_interactive,
            );
        }
    }
    let pro_dry_run = pro_dependency.is_some();
    let mut dry_run = args.dry_run || pro_dry_run;
    let mut flags = args.flags.clone();
    let mut params = params;
    let mut target_path = target_path;

    if crate::launcher::enabled(&flags) {
        use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
        let requested = resolve_capabilities(
            ResolveCapabilitiesArgs {
                allow_fs: false,
                allow_fetch: false,
                allow_child_process: false,
            },
            None,
            Some(workflow_dir.clone()),
        );
        let Some(choices) = crate::launcher::setup(crate::launcher::Plan {
            title: workflow_label.clone(),
            workflow: &workflow_path,
            target: target_path.clone(),
            params,
            dry_run,
            dry_run_locked: pro_dry_run,
            requested,
            flags: &flags,
        })
        .await?
        else {
            return Ok(());
        };
        choices.apply_to(&mut flags);
        target_path = normalize_target_path(choices.target)?;
        params = choices.params;
        dry_run = choices.dry_run || pro_dry_run;
    }

    let capabilities = flags.capabilities(None, Some(workflow_dir.clone()), dry_run);

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
            approve_all: args.start.approve || pro_dry_run,
            managed_git: args.start.managed_git,
            skip_shard_steps: pro_dry_run,
            nested_observer: Some(nested_observer),
            // Uncommitted changes are checked before the run's first write.
            allow_dirty: args.allow_dirty,
            ..flags.settings(capabilities, registry.clone(), &telemetry)
        },
    })
    .await?;
    let dry_run = outcome.dry_run;

    if crate::utils::report_prompt::should_show_report(
        args.report,
        flags.no_interactive,
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
            convert_diffs(&outcome.summary.diffs(), &target_path.display().to_string()),
        )
        .with_registry_link_url(Some(
            crate::utils::registry_link::registry_link_url_for_local_run(
                &registry.config.default_registry,
            ),
        ));
        crate::report_server::serve_report(report).await?;
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

    if format == OutputFormat::Text && is_yaml {
        upgrade_hint(&workflow_dir);
    }
    outcome.result()
}

/// Tell authors of a workflow.yaml how to move to workflow.ts. YAML keeps
/// working; this only points the way.
fn upgrade_hint(workflow_dir: &Path) {
    let dir = std::env::current_dir()
        .ok()
        .and_then(|cwd| workflow_dir.strip_prefix(&cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| workflow_dir.to_path_buf());
    let command = if dir.as_os_str().is_empty() {
        "codemod workflow upgrade".to_string()
    } else {
        format!("codemod workflow upgrade {}", dir.display())
    };
    eprintln!(
        "\n{} workflow.yaml runs as before. To move this codemod to workflow.ts, run `{command}`.",
        style("Tip:").cyan().bold()
    );
}
