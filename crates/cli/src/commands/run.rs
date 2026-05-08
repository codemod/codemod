use crate::engine::{create_engine, create_registry_client};
use crate::progress_bar::download_progress_bar;
use crate::utils::manifest::CodemodManifest;
use crate::utils::package_validation::{default_workflow_path, select_workflow_path};
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::workflow_runner::{run_workflow, workflow_has_manual_steps};
use crate::TelemetrySenderMutex;
use crate::CLI_VERSION;
use anyhow::{anyhow, Result};
use butterflow_core::diff::{generate_unified_diff, DiffConfig, FileDiff};
use butterflow_core::registry::RegistryError;
use butterflow_core::report::{convert_diffs, convert_metrics, ExecutionReport};
use butterflow_core::structured_log::OutputFormat;
use butterflow_core::utils::generate_execution_id;
use butterflow_core::utils::parse_params;
use clap::Args;
use codemod_telemetry::send_event::BaseEvent;
use console::{strip_ansi_codes, style};
use log::{debug, info};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

const WORKFLOW_FILE_NAME: &str = "workflow.yaml";

fn format_file_count(count: usize) -> String {
    if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    }
}

fn print_dry_run_summary(files_modified: usize, files_unmodified: usize, files_with_errors: usize) {
    println!("\n🔎 Dry run complete");
    println!("📝 Would modify: {}", format_file_count(files_modified));
    println!("✅ Unchanged: {}", format_file_count(files_unmodified));
    if files_with_errors > 0 {
        println!("❌ Errors: {}", format_file_count(files_with_errors));
    }
}

/// Represents a file change from legacy codemod JSON output
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyFileChange {
    kind: String,
    old_path: String,
    old_data: String,
    new_data: String,
}
#[derive(Args, Debug, Default)]
pub struct Command {
    /// Package name with optional version (e.g., @org/package@1.0.0)
    #[arg(value_name = "PACKAGE")]
    package: String,

    /// Registry URL
    #[arg(long)]
    registry: Option<String>,

    /// Force re-download even if cached
    #[arg(long)]
    force: bool,

    /// Dry run mode - don't make actual changes
    #[arg(long)]
    dry_run: bool,

    /// Additional arguments to pass to the codemod
    #[arg(long = "param", value_name = "KEY=VALUE")]
    params: Option<Vec<String>>,

    /// Allow dirty git status
    #[arg(long)]
    allow_dirty: bool,

    /// Optional target path to run the codemod on (default: current directory)
    #[arg(long = "target", short = 't')]
    target_path: Option<PathBuf>,

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
    #[arg(long, default_value_t = OutputFormat::Text)]
    format: OutputFormat,

    /// Name of the workflow to run when the package defines multiple workflows
    #[arg(long, value_name = "NAME")]
    workflow: Option<String>,
}

impl Command {
    pub fn from_package_for_prompt(package: String) -> Self {
        Self {
            package,
            ..Default::default()
        }
    }
}

async fn send_failure_event(
    telemetry: &TelemetrySenderMutex,
    codemod_name: &str,
    error_message: &str,
) {
    telemetry
        .send_event(
            BaseEvent {
                kind: "failedToExecuteCommand".to_string(),
                properties: HashMap::from([
                    ("codemodName".to_string(), codemod_name.to_string()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    (
                        "commandName".to_string(),
                        "codemod.executeCodemod".to_string(),
                    ),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                    ("errorMessage".to_string(), error_message.to_string()),
                ]),
            },
            None,
        )
        .await;
}

pub async fn handler(
    args: &Command,
    telemetry: TelemetrySenderMutex,
    disable_analytics: bool,
) -> Result<()> {
    // Resolve the package (local path or registry package)
    let download_progress_bar = Some(download_progress_bar());
    let registry_client = create_registry_client(args.registry.clone())?;
    let registry_url = registry_client.config.default_registry.clone();
    println!(
        "{} 🔍 Resolving package from registry: {} ...",
        style("[1/2]").bold().dim(),
        registry_url
    );
    let resolved_package = match registry_client
        .resolve_package(
            &args.package,
            Some(&registry_url),
            args.force,
            download_progress_bar,
        )
        .await
    {
        Ok(package) => package,
        Err(RegistryError::LegacyPackage { package }) => {
            info!("Package {package} is legacy, running npx codemod@legacy");
            println!(
                "{}",
                style(format!("⚠️ Package {package} is legacy")).yellow()
            );
            println!(
                "{} 🏁 Running codemod: {}",
                style("[2/2]").bold().dim(),
                args.package,
            );
            return run_legacy_codemod(args, disable_analytics).await;
        }
        Err(e) => {
            let error_msg = format!("Registry error: {}", e);
            send_failure_event(&telemetry, &args.package, &error_msg).await;
            return Err(anyhow::anyhow!("{}", error_msg));
        }
    };

    // Auto-force dry-run for non-pro users accessing pro codemods.
    // Show an informational notice explaining what free preview covers and
    // how to unlock applying changes + advanced insights.
    let (resolved_package, dry_run) = if resolved_package.dry_run_only {
        if !args.dry_run && !args.no_interactive {
            let notice = style(
                "This is a Pro codemod. Preview changes and insights for free with no login or code sharing. \
                 Applying changes and advanced insights requires a Pro plan and signing in. \
                 Learn more: codemod.com/contact."
            )
            .yellow();

            // Only block for a keypress when both stdin and stdout are attached
            // to a terminal. Otherwise the notice (or the "press any key" line)
            // would be hidden from the user — e.g. `codemod run <pro> > out.txt`
            // would silently hang. In that case, route the notice to stderr so
            // it stays visible and proceed straight into dry-run.
            if io::stdin().is_terminal() && io::stdout().is_terminal() {
                println!("{notice}");
                println!("{}", style("Press any key to proceed.").dim());
                wait_for_any_key();
            } else {
                eprintln!("{notice}");
            }
        }

        (resolved_package, true)
    } else {
        (resolved_package, args.dry_run)
    };

    info!(
        "Resolved codemod package: {} -> {}",
        args.package,
        resolved_package.package_dir.display()
    );

    println!(
        "{} 🏁 Running codemod: {}",
        style("[2/2]").bold().dim(),
        args.package,
    );

    let target_path = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    let codemod_config_path = resolved_package.package_dir.join("codemod.yaml");
    let codemod_config = load_codemod_manifest(&codemod_config_path)?;

    let (workflow_path, selected_workflow_name) = match select_workflow_for_run(
        &resolved_package.package_dir,
        codemod_config.as_ref(),
        args.workflow.as_deref(),
        args.no_interactive,
    ) {
        Ok(selected) => selected,
        Err(error) => {
            let error_msg = error.to_string();
            send_failure_event(&telemetry, &args.package, &error_msg).await;
            return Err(error);
        }
    };
    if !workflow_path.exists() {
        let error = missing_workflow_error(&args.package, &workflow_path);
        let error_msg = error.to_string();
        send_failure_event(&telemetry, &args.package, &error_msg).await;
        return Err(error);
    }

    let params = parse_params(args.params.as_deref().unwrap_or(&[]))
        .map_err(|e| anyhow::anyhow!("Failed to parse parameters: {}", e))?;
    let workflow_definition = butterflow_core::utils::parse_workflow_file(&workflow_path)
        .map_err(|e| anyhow::anyhow!("Failed to parse workflow before run: {}", e))?;
    let auto_launch_tui =
        should_auto_launch_package_run_tui(args.no_interactive, dry_run, &workflow_definition);

    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        codemod_config,
        None,
    );

    // Always collect diffs so report output remains available for interactive flows.
    let diff_collector = Some(Arc::new(Mutex::new(Vec::<FileDiff>::new())));

    let started = std::time::Instant::now();

    let output_format = args.format;

    // Run workflow using the extracted workflow runner
    let (mut engine, config) = create_engine(
        workflow_path,
        target_path.clone(),
        dry_run,
        args.allow_dirty || resolved_package.dry_run_only,
        params,
        args.registry.clone(),
        Some(capabilities.clone()),
        args.no_interactive,
        diff_collector.clone(),
        args.no_interactive && !args.install_skill,
        output_format,
        Some(capabilities),
        args.agent.clone(),
        Some(crate::commands::package_skill::create_install_skill_executor(telemetry.clone())),
    )?;

    // Set the package name so it's stored on the WorkflowRun
    engine.set_name(Some(args.package.clone()));
    apply_package_run_mode_to_config(engine.workflow_run_config_mut(), auto_launch_tui);
    if auto_launch_tui {
        engine.set_quiet(true);
        engine.set_progress_callback(Arc::new(None));
    }

    // For pro codemod dry-run: streamline execution — auto-trigger manual
    // steps, skip shards, skip state writes, flatten matrix to one task per node.
    // Set on both engine (used at runtime) and config (passed to run_workflow).
    if resolved_package.dry_run_only {
        let engine_config = engine.workflow_run_config_mut();
        engine_config.auto_trigger_manual_steps = true;
        engine_config.skip_shard_steps = true;
        engine_config.skip_state_writes = true;
        engine_config.flatten_matrix_tasks = true;
    }

    let run_result = run_workflow(&mut engine, config).await;

    if let Err(e) = run_result {
        // Clean up cached pro codemod on failure too
        if resolved_package.dry_run_only {
            let _ = std::fs::remove_dir_all(&resolved_package.package_dir);
        }
        let error_msg = format!("Workflow execution failed: {}", e);
        send_failure_event(&telemetry, &args.package, &error_msg).await;
        return Err(e);
    }

    let duration_ms = started.elapsed().as_millis() as f64;

    let metrics_data = engine.metrics_context.get_all();

    let stats = engine.execution_stats.clone();
    let files_modified = stats.files_modified.load(Ordering::Relaxed);
    let files_unmodified = stats.files_unmodified.load(Ordering::Relaxed);
    let files_with_errors = stats.files_with_errors.load(Ordering::Relaxed);

    if dry_run {
        print_dry_run_summary(files_modified, files_unmodified, files_with_errors);
    } else {
        println!("\n📝 Modified files: {files_modified}");
        println!("✅ Unmodified files: {files_unmodified}");
        if files_with_errors > 0 {
            println!("❌ Files with errors: {files_with_errors}");
        }
    }

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
            args.package.clone(),
            None,
            duration_ms,
            dry_run,
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

    let execution_id = generate_execution_id();

    let mut executed_props = HashMap::from([
        ("codemodName".to_string(), args.package.clone()),
        ("executionId".to_string(), execution_id.clone()),
        ("fileCount".to_string(), files_modified.to_string()),
        ("cliVersion".to_string(), CLI_VERSION.to_string()),
        ("os".to_string(), std::env::consts::OS.to_string()),
        ("arch".to_string(), std::env::consts::ARCH.to_string()),
    ]);
    if let Some(name) = &selected_workflow_name {
        executed_props.insert("workflowName".to_string(), name.clone());
    }
    telemetry
        .send_event(
            BaseEvent {
                kind: "codemodExecuted".to_string(),
                properties: executed_props,
            },
            None,
        )
        .await;

    if resolved_package.dry_run_only {
        if let Err(e) = std::fs::remove_dir_all(&resolved_package.package_dir) {
            debug!("Failed to remove cached pro codemod: {}", e);
        }
    }

    Ok(())
}

/// Returns an error for a failed legacy codemod command.
fn legacy_command_error(exit_code: Option<i32>) -> anyhow::Error {
    anyhow!(
        "Legacy codemod command failed with exit code: {:?}",
        exit_code
    )
}

/// Block until the user presses any key. Falls back to a no-op when either
/// stdin or stdout isn't a terminal (e.g. piped input or redirected output)
/// or when raw mode can't be enabled. Callers must ensure any prompt they want
/// the user to read has already been printed to a stream the user can see.
fn wait_for_any_key() {
    use crossterm::event::{read, Event, KeyEventKind};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return;
    }
    if enable_raw_mode().is_err() {
        return;
    }
    loop {
        match read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let _ = disable_raw_mode();
}

fn load_codemod_manifest(codemod_config_path: &Path) -> Result<Option<CodemodManifest>> {
    if !codemod_config_path.exists() {
        return Ok(None);
    }

    let codemod_config_content = fs::read_to_string(codemod_config_path)?;
    let codemod_config: CodemodManifest = serde_yaml::from_str(&codemod_config_content)
        .map_err(|e| anyhow!("Failed to parse codemod.yaml: {}", e))?;
    Ok(Some(codemod_config))
}

fn workflow_path_for_run(
    package_dir: &Path,
    manifest: Option<&CodemodManifest>,
    workflow_name: Option<&str>,
) -> Result<PathBuf> {
    if let Some(manifest) = manifest {
        if workflow_name.is_some() {
            return Ok(select_workflow_path(package_dir, manifest, workflow_name)?.path);
        }
        return default_workflow_path(package_dir, manifest);
    }

    if workflow_name.is_some() {
        return Err(anyhow!(
            "Cannot select a named workflow without a codemod.yaml manifest."
        ));
    }
    Ok(package_dir.join(WORKFLOW_FILE_NAME))
}

fn select_workflow_for_run(
    package_dir: &Path,
    manifest: Option<&CodemodManifest>,
    workflow_name: Option<&str>,
    no_interactive: bool,
) -> Result<(PathBuf, Option<String>)> {
    let Some(manifest) = manifest else {
        let path = workflow_path_for_run(package_dir, None, workflow_name)?;
        return Ok((path, None));
    };

    if let Some(name) = workflow_name {
        let resolved = select_workflow_path(package_dir, manifest, Some(name))?;
        return Ok((resolved.path, Some(resolved.entry.name)));
    }

    let entries = manifest.resolved_workflows()?;
    if entries.len() == 1 {
        let entry = entries.into_iter().next().unwrap();
        return Ok((package_dir.join(&entry.path), Some(entry.name)));
    }

    let stdin_is_tty = io::stdin().is_terminal();
    let stdout_is_tty = io::stdout().is_terminal();
    if no_interactive || !stdin_is_tty || !stdout_is_tty {
        let entry = manifest.default_workflow()?;
        return Ok((package_dir.join(&entry.path), Some(entry.name)));
    }

    let chosen = prompt_workflow_selection(&entries)?;
    let path = package_dir.join(&chosen.path);
    Ok((path, Some(chosen.name)))
}

fn prompt_workflow_selection(
    entries: &[crate::utils::manifest::WorkflowEntry],
) -> Result<crate::utils::manifest::WorkflowEntry> {
    let labels: Vec<String> = entries
        .iter()
        .map(|entry| {
            let suffix = match (&entry.description, entry.default) {
                (Some(desc), true) => format!(" — {desc} (default)"),
                (Some(desc), false) => format!(" — {desc}"),
                (None, true) => " (default)".to_string(),
                (None, false) => String::new(),
            };
            format!("{}{}", entry.name, suffix)
        })
        .collect();
    let default_idx = entries.iter().position(|e| e.default).unwrap_or(0);

    let selected = inquire::Select::new("Select a workflow to run:", labels.clone())
        .with_starting_cursor(default_idx)
        .prompt()
        .map_err(|e| anyhow!("Workflow selection cancelled: {e}"))?;
    let index = labels
        .iter()
        .position(|l| l == &selected)
        .ok_or_else(|| anyhow!("Internal error: workflow selection mismatch"))?;
    Ok(entries[index].clone())
}

fn should_auto_launch_package_run_tui(
    no_interactive: bool,
    dry_run: bool,
    workflow_definition: &butterflow_core::Workflow,
) -> bool {
    !no_interactive && !dry_run && workflow_has_manual_steps(workflow_definition)
}

fn apply_package_run_mode_to_config(
    cfg: &mut butterflow_core::config::WorkflowRunConfig,
    auto_launch_tui: bool,
) {
    cfg.enable_managed_git = auto_launch_tui;
    cfg.enable_worktrees = auto_launch_tui;
    if auto_launch_tui {
        cfg.quiet = true;
        cfg.capture_stdout_in_quiet_mode = false;
    }
}

fn missing_workflow_error(package_id: &str, workflow_path: &Path) -> anyhow::Error {
    anyhow!(
        "Package `{}` is missing required workflow file at {}.",
        package_id,
        workflow_path.display()
    )
}

pub async fn run_legacy_codemod_with_raw_args(raw_args: &[String]) -> Result<()> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut cmd = ProcessCommand::new("cmd");
        cmd.args(["/C", "npx", "codemod@legacy"]);
        cmd.args(raw_args);
        cmd
    } else {
        let mut cmd = ProcessCommand::new("npx");
        cmd.arg("codemod@legacy");
        cmd.args(raw_args);
        cmd
    };

    let is_non_interactive = raw_args.iter().any(|arg| arg == "--no-interactive");

    if is_non_interactive {
        // Disable interactive features for CI/headless environments
        cmd.env("CI", "true")
            .env("TERM", "dumb")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }

    info!(
        "Executing: {} with args: {:?}",
        if cfg!(target_os = "windows") {
            "cmd /C npx codemod@legacy"
        } else {
            "npx codemod@legacy"
        },
        cmd.get_args().collect::<Vec<_>>()
    );

    if is_non_interactive {
        let output = cmd.output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let filtered_stdout = strip_ansi_codes(&stdout);
        let filtered_stderr = strip_ansi_codes(&stderr);

        if !filtered_stdout.is_empty() {
            print!("{filtered_stdout}");
        }
        if !filtered_stderr.is_empty() {
            eprint!("{filtered_stderr}");
        }

        if !output.status.success() {
            return Err(legacy_command_error(output.status.code()));
        }
    } else {
        let status = cmd.status()?;
        if !status.success() {
            return Err(legacy_command_error(status.code()));
        }
    }

    Ok(())
}

async fn run_legacy_codemod(args: &Command, disable_analytics: bool) -> Result<()> {
    // If dry-run mode, use JSON output and generate diffs ourselves
    if args.dry_run {
        return run_legacy_codemod_with_diff(args, disable_analytics).await;
    }

    let mut legacy_args = vec![args.package.clone()];
    if let Some(target_path) = args.target_path.as_ref() {
        legacy_args.push("--target".to_string());
        legacy_args.push(target_path.to_string_lossy().to_string());
    }
    if args.allow_dirty {
        legacy_args.push("--skip-git-check".to_string());
    }
    if args.no_interactive {
        legacy_args.push("--no-interactive".to_string());
    }
    if disable_analytics {
        legacy_args.push("--no-telemetry".to_string());
    }
    run_legacy_codemod_with_raw_args(&legacy_args).await
}

/// Run legacy codemod in dry-run mode with diff output
async fn run_legacy_codemod_with_diff(args: &Command, disable_analytics: bool) -> Result<()> {
    let mut legacy_args = vec![args.package.clone()];
    if let Some(target_path) = args.target_path.as_ref() {
        legacy_args.push("--target".to_string());
        legacy_args.push(target_path.to_string_lossy().to_string());
    }
    legacy_args.push("--dry".to_string());
    legacy_args.push("--mode".to_string());
    legacy_args.push("json".to_string());
    legacy_args.push("--no-interactive".to_string());
    if args.allow_dirty {
        legacy_args.push("--skip-git-check".to_string());
    }
    if disable_analytics {
        legacy_args.push("--no-telemetry".to_string());
    }

    // Build command
    let mut cmd = if cfg!(target_os = "windows") {
        let mut cmd = ProcessCommand::new("cmd");
        cmd.args(["/C", "npx", "codemod@legacy"]);
        cmd.args(&legacy_args);
        cmd
    } else {
        let mut cmd = ProcessCommand::new("npx");
        cmd.arg("codemod@legacy");
        cmd.args(&legacy_args);
        cmd
    };

    // Capture output
    cmd.env("CI", "true")
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    info!(
        "Executing legacy codemod with JSON output: {:?}",
        cmd.get_args().collect::<Vec<_>>()
    );

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print stderr (contains progress info)
    if !stderr.is_empty() {
        let filtered_stderr = strip_ansi_codes(&stderr);
        eprint!("{}", filtered_stderr);
    }

    // Try to find JSON array in stdout (it may have other output before it)
    let json_start = stdout.find('[');
    let json_end = stdout.rfind(']');

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &stdout[start..=end];

        match serde_json::from_str::<Vec<LegacyFileChange>>(json_str) {
            Ok(changes) => {
                let diff_config = DiffConfig::with_color_control(args.no_color);

                let mut total_additions = 0;
                let mut total_deletions = 0;
                let files_modified = changes.len();

                for change in &changes {
                    if change.kind == "updateFile" {
                        let path = PathBuf::from(&change.old_path);
                        let diff = generate_unified_diff(
                            &path,
                            &change.old_data,
                            &change.new_data,
                            &diff_config,
                        );
                        diff.print();
                        total_additions += diff.additions;
                        total_deletions += diff.deletions;
                    }
                }

                println!("\n🔎 Dry run complete");
                println!("📝 Would modify: {}", format_file_count(files_modified));
                println!("Δ Changes: +{} -{}", total_additions, total_deletions);
            }
            Err(e) => {
                // JSON parsing failed, print raw output
                info!("Failed to parse JSON output: {}", e);
                let filtered_stdout = strip_ansi_codes(&stdout);
                if !filtered_stdout.is_empty() {
                    print!("{}", filtered_stdout);
                }
            }
        }
    } else {
        // No JSON found, print raw output
        let filtered_stdout = strip_ansi_codes(&stdout);
        if !filtered_stdout.is_empty() {
            print!("{}", filtered_stdout);
        }
    }

    if !output.status.success() {
        return Err(legacy_command_error(output.status.code()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with_workflow(workflow: &str) -> CodemodManifest {
        CodemodManifest {
            schema_version: "1.0".to_string(),
            name: "sample".to_string(),
            version: "0.1.0".to_string(),
            description: "sample".to_string(),
            author: "codemod".to_string(),
            license: None,
            copyright: None,
            repository: None,
            homepage: None,
            bugs: None,
            registry: None,
            workflow: Some(workflow.to_string()),
            workflows: None,
            targets: None,
            dependencies: None,
            keywords: None,
            category: None,
            readme: None,
            changelog: None,
            documentation: None,
            validation: None,
            capabilities: None,
        }
    }

    #[test]
    fn test_legacy_file_change_deserialization() {
        let json = r#"{
            "kind": "updateFile",
            "oldPath": "/path/to/file.js",
            "oldData": "const x = 1;",
            "newData": "const x = 2;"
        }"#;

        let change: LegacyFileChange = serde_json::from_str(json).unwrap();
        assert_eq!(change.kind, "updateFile");
        assert_eq!(change.old_path, "/path/to/file.js");
        assert_eq!(change.old_data, "const x = 1;");
        assert_eq!(change.new_data, "const x = 2;");
    }

    #[test]
    fn test_legacy_file_change_array_deserialization() {
        let json = r#"[
            {
                "kind": "updateFile",
                "oldPath": "/path/to/file1.js",
                "oldData": "import { it } from 'jest';",
                "newData": "import { it } from 'vitest';"
            },
            {
                "kind": "updateFile",
                "oldPath": "/path/to/file2.js",
                "oldData": "jest.fn()",
                "newData": "vi.fn()"
            }
        ]"#;

        let changes: Vec<LegacyFileChange> = serde_json::from_str(json).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].old_path, "/path/to/file1.js");
        assert_eq!(changes[1].old_path, "/path/to/file2.js");
    }

    #[test]
    fn test_extract_json_from_mixed_output() {
        // Simulates output with stderr noise before JSON
        let mixed_output = r#"- Fetching "jest/vitest"...
✔ Successfully fetched "jest/vitest" from local cache.
[
  {
    "kind": "updateFile",
    "oldPath": "/path/to/test.js",
    "oldData": "old content",
    "newData": "new content"
  }
]"#;

        let json_start = mixed_output.find('[');
        let json_end = mixed_output.rfind(']');

        assert!(json_start.is_some());
        assert!(json_end.is_some());

        let json_str = &mixed_output[json_start.unwrap()..=json_end.unwrap()];
        let changes: Vec<LegacyFileChange> = serde_json::from_str(json_str).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "updateFile");
        assert_eq!(changes[0].old_path, "/path/to/test.js");
    }

    #[test]
    fn test_workflow_path_for_run_prefers_manifest_workflow_path() {
        let package_dir = Path::new("/tmp/sample");
        let manifest = manifest_with_workflow("custom/workflow.yaml");

        let workflow_path = workflow_path_for_run(package_dir, Some(&manifest), None).unwrap();
        assert_eq!(workflow_path, package_dir.join("custom/workflow.yaml"));
    }

    fn manifest_with_named_workflows(entries: Vec<(&str, &str, bool)>) -> CodemodManifest {
        let mut manifest = manifest_with_workflow("workflow.yaml");
        manifest.workflow = None;
        manifest.workflows = Some(
            entries
                .into_iter()
                .map(
                    |(name, path, default)| crate::utils::manifest::WorkflowEntry {
                        name: name.to_string(),
                        path: path.to_string(),
                        description: None,
                        default,
                    },
                )
                .collect(),
        );
        manifest
    }

    #[test]
    fn test_workflow_path_for_run_uses_default_when_no_workflow_name() {
        let package_dir = Path::new("/tmp/multi");
        let manifest = manifest_with_named_workflows(vec![
            ("plain", "workflow.yaml", true),
            ("sharded", "workflows/sharded.yaml", false),
        ]);

        let path = workflow_path_for_run(package_dir, Some(&manifest), None).unwrap();
        assert_eq!(path, package_dir.join("workflow.yaml"));
    }

    #[test]
    fn test_workflow_path_for_run_resolves_named_workflow() {
        let package_dir = Path::new("/tmp/multi");
        let manifest = manifest_with_named_workflows(vec![
            ("plain", "workflow.yaml", true),
            ("sharded", "workflows/sharded.yaml", false),
        ]);

        let path = workflow_path_for_run(package_dir, Some(&manifest), Some("sharded")).unwrap();
        assert_eq!(path, package_dir.join("workflows/sharded.yaml"));
    }

    #[test]
    fn test_workflow_path_for_run_unknown_workflow_errors() {
        let package_dir = Path::new("/tmp/multi");
        let manifest = manifest_with_named_workflows(vec![
            ("plain", "workflow.yaml", true),
            ("sharded", "workflows/sharded.yaml", false),
        ]);

        let err = workflow_path_for_run(package_dir, Some(&manifest), Some("missing")).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_missing_workflow_error_mentions_expected_path() {
        let error = missing_workflow_error("@codemod/any", Path::new("/tmp/any/workflow.yaml"));
        let message = error.to_string();

        assert!(message.contains("missing required workflow file"));
        assert!(message.contains("/tmp/any/workflow.yaml"));
    }

    fn workflow_with_manual_step() -> butterflow_core::Workflow {
        butterflow_core::Workflow {
            version: "1".to_string(),
            state: None,
            params: None,
            templates: vec![],
            nodes: vec![butterflow_core::Node {
                id: "manual-node".to_string(),
                name: "Manual Node".to_string(),
                description: None,
                r#type: butterflow_models::node::NodeType::Manual,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![butterflow_core::Step {
                    id: None,
                    name: "noop".to_string(),
                    action: butterflow_models::step::StepAction::RunScript("echo hi".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: Some("codemod-test".to_string()),
                pull_request: Some(butterflow_models::step::PullRequestConfig {
                    title: "Test PR".to_string(),
                    body: None,
                    draft: Some(true),
                    base: None,
                }),
            }],
        }
    }

    fn workflow_with_manual_pull_request_only_node() -> butterflow_core::Workflow {
        butterflow_core::Workflow {
            version: "1".to_string(),
            state: None,
            params: None,
            templates: vec![],
            nodes: vec![butterflow_core::Node {
                id: "publish-node".to_string(),
                name: "Publish Node".to_string(),
                description: None,
                r#type: butterflow_models::node::NodeType::Manual,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
                branch_name: Some("codemod-test".to_string()),
                pull_request: Some(butterflow_models::step::PullRequestConfig {
                    title: "Test PR".to_string(),
                    body: None,
                    draft: Some(true),
                    base: None,
                }),
            }],
        }
    }

    #[test]
    fn non_tui_package_run_disables_managed_git_and_worktrees_even_with_manual_steps() {
        let workflow = workflow_with_manual_step();
        let auto_launch_tui = should_auto_launch_package_run_tui(true, false, &workflow);
        assert!(!auto_launch_tui);

        let mut cfg = butterflow_core::config::WorkflowRunConfig::default();
        apply_package_run_mode_to_config(&mut cfg, auto_launch_tui);
        assert!(!cfg.enable_managed_git);
        assert!(!cfg.enable_worktrees);
        assert!(!cfg.quiet);
        assert!(cfg.capture_stdout_in_quiet_mode);
    }

    #[test]
    fn interactive_manual_package_run_enables_tui_managed_git_mode() {
        let workflow = workflow_with_manual_step();
        let auto_launch_tui = should_auto_launch_package_run_tui(false, false, &workflow);
        assert!(auto_launch_tui);

        let mut cfg = butterflow_core::config::WorkflowRunConfig::default();
        apply_package_run_mode_to_config(&mut cfg, auto_launch_tui);
        assert!(cfg.enable_managed_git);
        assert!(cfg.enable_worktrees);
        assert!(cfg.quiet);
        assert!(!cfg.capture_stdout_in_quiet_mode);
    }

    #[test]
    fn manual_pull_request_only_package_run_does_not_enable_tui_mode() {
        let workflow = workflow_with_manual_pull_request_only_node();
        let auto_launch_tui = should_auto_launch_package_run_tui(false, false, &workflow);
        assert!(!auto_launch_tui);

        let mut cfg = butterflow_core::config::WorkflowRunConfig::default();
        apply_package_run_mode_to_config(&mut cfg, auto_launch_tui);
        assert!(!cfg.enable_managed_git);
        assert!(!cfg.enable_worktrees);
        assert!(!cfg.quiet);
        assert!(cfg.capture_stdout_in_quiet_mode);
    }

    #[test]
    fn dry_run_package_run_does_not_enable_tui_mode() {
        let workflow = workflow_with_manual_step();
        let auto_launch_tui = should_auto_launch_package_run_tui(false, true, &workflow);
        assert!(!auto_launch_tui);
    }
}
