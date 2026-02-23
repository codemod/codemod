use crate::engine::{create_engine, create_registry_client};
use crate::progress_bar::download_progress_bar;
use crate::utils::manifest::CodemodManifest;
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::workflow_runner::run_workflow;
use crate::TelemetrySenderMutex;
use crate::CLI_VERSION;
use anyhow::{anyhow, Result};
use butterflow_core::diff::{generate_unified_diff, DiffConfig, FileDiff};
use butterflow_core::registry::RegistryError;
use butterflow_core::report::{convert_diffs, convert_metrics, ExecutionReport};
use butterflow_core::utils::generate_execution_id;
use butterflow_core::utils::parse_params;
use clap::Args;
use codemod_telemetry::send_event::BaseEvent;
use console::{strip_ansi_codes, style};
use log::info;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Represents a file change from legacy codemod JSON output
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyFileChange {
    kind: String,
    old_path: String,
    old_data: String,
    new_data: String,
}
#[derive(Args, Debug)]
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

    /// Disable colored diff output in dry-run mode
    #[arg(long)]
    no_color: bool,

    /// Open a web-based execution report after the run completes
    #[arg(long)]
    report: bool,
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

    let workflow_path = resolved_package.package_dir.join("workflow.yaml");

    let params = parse_params(args.params.as_deref().unwrap_or(&[]))
        .map_err(|e| anyhow::anyhow!("Failed to parse parameters: {}", e))?;

    let codemod_config_path = resolved_package.package_dir.join("codemod.yaml");

    let codemod_config: Option<CodemodManifest> = if codemod_config_path.exists() {
        let codemod_config_content = fs::read_to_string(&codemod_config_path)?;

        let codemod_config: CodemodManifest = serde_yaml::from_str(&codemod_config_content)
            .map_err(|e| anyhow!("Failed to parse codemod.yaml: {}", e))?;

        Some(codemod_config)
    } else {
        None
    };

    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        codemod_config,
        None,
    );

    // Always collect diffs so we can offer report interactively
    let diff_collector = Some(Arc::new(Mutex::new(Vec::<FileDiff>::new())));

    let started = std::time::Instant::now();

    // Run workflow using the extracted workflow runner
    let (engine, config) = create_engine(
        workflow_path,
        target_path.clone(),
        args.dry_run,
        args.allow_dirty,
        params,
        args.registry.clone(),
        Some(capabilities),
        args.no_interactive,
        args.no_color,
        diff_collector.clone(),
    )?;

    if let Err(e) = run_workflow(&engine, config).await {
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

    if args.dry_run {
        println!("\n=== DRY RUN SUMMARY ===");
        println!("Files that would be modified: {files_modified}");
        println!("Files that would be unmodified: {files_unmodified}");
        if files_with_errors > 0 {
            println!("Files with errors: {files_with_errors}");
        }
        println!("No changes were made to the filesystem.");
    } else {
        println!("\n📝 Modified files: {files_modified}");
        println!("✅ Unmodified files: {files_unmodified}");
        if files_with_errors > 0 {
            println!("❌ Files with errors: {files_with_errors}");
        }
    }

    if crate::utils::metrics::should_show_report(args.report, args.no_interactive, &metrics_data) {
        let collected_diffs = diff_collector
            .map(|c| c.lock().unwrap().clone())
            .unwrap_or_default();

        let report = ExecutionReport::build(
            args.package.clone(),
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

    let execution_id = generate_execution_id();

    telemetry
        .send_event(
            BaseEvent {
                kind: "codemodExecuted".to_string(),
                properties: HashMap::from([
                    ("codemodName".to_string(), args.package.clone()),
                    ("executionId".to_string(), execution_id.clone()),
                    ("fileCount".to_string(), files_modified.to_string()),
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

/// Returns an error for a failed legacy codemod command.
fn legacy_command_error(exit_code: Option<i32>) -> anyhow::Error {
    anyhow!(
        "Legacy codemod command failed with exit code: {:?}",
        exit_code
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

                // Print summary
                println!("\n=== DRY RUN SUMMARY ===");
                println!("Files that would be modified: {}", files_modified);
                println!(
                    "Total: +{} additions, -{} deletions",
                    total_additions, total_deletions
                );
                println!("No changes were made to the filesystem.");
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
}
