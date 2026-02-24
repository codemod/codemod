use crate::engine::{create_engine, create_registry_client};
use crate::progress_bar::download_progress_bar;
use crate::utils::manifest::CodemodManifest;
use crate::utils::package_validation::{
    detect_package_behavior_shape_with_manifest_hint, expected_workflow_path, PackageBehaviorShape,
};
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::utils::skill_layout::AGENTS_SKILL_ROOT_RELATIVE_PATH;
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
use inquire::Confirm;
use log::info;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

const WORKFLOW_FILE_NAME: &str = "workflow.yaml";
const SKILL_INSTALL_PROJECT_FLAG: &str = "--project";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SkillInstallOfferContext {
    SkillOnly,
    HybridPostRun,
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

    let codemod_config_path = resolved_package.package_dir.join("codemod.yaml");
    let codemod_config = load_codemod_manifest(&codemod_config_path)?;

    let package_behavior_shape = detect_package_behavior_shape_for_run(
        &resolved_package.package_dir,
        codemod_config.as_ref(),
    );

    if package_behavior_shape == PackageBehaviorShape::SkillOnly {
        if maybe_offer_skill_install(args, SkillInstallOfferContext::SkillOnly, &telemetry).await? {
            return Ok(());
        }
        let error = skill_only_package_run_error(&args.package, &resolved_package.package_dir);
        let error_msg = error.to_string();
        send_failure_event(&telemetry, &args.package, &error_msg).await;
        return Err(error);
    }

    if package_behavior_shape == PackageBehaviorShape::Missing {
        let error = missing_behavior_run_error(&args.package, &resolved_package.package_dir);
        let error_msg = error.to_string();
        send_failure_event(&telemetry, &args.package, &error_msg).await;
        return Err(error);
    }

    let workflow_path =
        workflow_path_for_run(&resolved_package.package_dir, codemod_config.as_ref());
    if !workflow_path.exists() {
        let error = missing_workflow_error(&args.package, &workflow_path);
        let error_msg = error.to_string();
        send_failure_event(&telemetry, &args.package, &error_msg).await;
        return Err(error);
    }

    let params = parse_params(args.params.as_deref().unwrap_or(&[]))
        .map_err(|e| anyhow::anyhow!("Failed to parse parameters: {}", e))?;

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
        package_behavior_shape.includes_skill(),
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

    if package_behavior_shape == PackageBehaviorShape::Hybrid {
        let _ =
            maybe_offer_skill_install(args, SkillInstallOfferContext::HybridPostRun, &telemetry)
                .await?;
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

fn load_codemod_manifest(codemod_config_path: &Path) -> Result<Option<CodemodManifest>> {
    if !codemod_config_path.exists() {
        return Ok(None);
    }

    let codemod_config_content = fs::read_to_string(codemod_config_path)?;
    let codemod_config: CodemodManifest = serde_yaml::from_str(&codemod_config_content)
        .map_err(|e| anyhow!("Failed to parse codemod.yaml: {}", e))?;
    Ok(Some(codemod_config))
}

fn detect_package_behavior_shape_for_run(
    package_dir: &Path,
    manifest: Option<&CodemodManifest>,
) -> PackageBehaviorShape {
    detect_package_behavior_shape_with_manifest_hint(package_dir, manifest)
}

fn workflow_path_for_run(package_dir: &Path, manifest: Option<&CodemodManifest>) -> PathBuf {
    if let Some(manifest) = manifest {
        return expected_workflow_path(package_dir, manifest);
    }

    package_dir.join(WORKFLOW_FILE_NAME)
}

fn should_prompt_for_skill_install(no_interactive: bool) -> bool {
    should_prompt_for_skill_install_with_tty(
        no_interactive,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
    )
}

fn should_prompt_for_skill_install_with_tty(
    no_interactive: bool,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
) -> bool {
    !no_interactive && stdin_is_tty && stdout_is_tty
}

fn skill_install_command(package_id: &str) -> String {
    format!("npx codemod {package_id} --skill {SKILL_INSTALL_PROJECT_FLAG}")
}

fn skill_install_prompt_message(context: SkillInstallOfferContext) -> &'static str {
    match context {
        SkillInstallOfferContext::SkillOnly => {
            "Install this package skill now so your harness can execute it?"
        }
        SkillInstallOfferContext::HybridPostRun => {
            "Install this package skill now for harness-assisted follow-up workflows?"
        }
    }
}

async fn maybe_offer_skill_install(
    args: &Command,
    context: SkillInstallOfferContext,
    telemetry: &TelemetrySenderMutex,
) -> Result<bool> {
    let install_command = skill_install_command(&args.package);
    match context {
        SkillInstallOfferContext::SkillOnly => {
            println!(
                "\nℹ️ Package `{}` is skill-only (workflow contains `install-skill` steps but no executable steps).",
                args.package
            );
        }
        SkillInstallOfferContext::HybridPostRun => {
            println!(
                "\nℹ️ Package `{}` also includes skill behavior under `{}`.",
                args.package, AGENTS_SKILL_ROOT_RELATIVE_PATH
            );
        }
    }

    if !should_prompt_for_skill_install(args.no_interactive) {
        println!("Install skill with: `{install_command}`");
        return Ok(false);
    }

    let should_install = match Confirm::new(skill_install_prompt_message(context))
        .with_default(true)
        .prompt()
    {
        Ok(answer) => answer,
        Err(error) => {
            println!(
                "Skipped skill install prompt ({error}). Install later with: `{install_command}`"
            );
            return Ok(false);
        }
    };

    if !should_install {
        println!("Skipped skill install. You can install later with: `{install_command}`");
        return Ok(false);
    }

    let install_args = vec![
        args.package.clone(),
        "--skill".to_string(),
        SKILL_INSTALL_PROJECT_FLAG.to_string(),
    ];
    let handled =
        crate::commands::package_skill::handle_direct_install(&install_args, telemetry).await?;
    if !handled {
        return Err(anyhow!(
            "Failed to invoke package skill install for `{}`.",
            args.package
        ));
    }

    Ok(true)
}

fn skill_only_package_run_error(package_id: &str, package_dir: &Path) -> anyhow::Error {
    anyhow!(
        "Package `{}` at {} is a skill-only package (workflow contains `install-skill` steps but no executable steps). `codemod run` executes workflow behavior only. Install this package as a skill with `{}`.",
        package_id,
        package_dir.display(),
        skill_install_command(package_id)
    )
}

fn missing_behavior_run_error(package_id: &str, package_dir: &Path) -> anyhow::Error {
    anyhow!(
        "Package `{}` at {} has no executable workflow steps and no installable skill behavior. Add executable workflow steps, or add `install-skill` plus authored files under `{}`.",
        package_id,
        package_dir.display(),
        AGENTS_SKILL_ROOT_RELATIVE_PATH,
    )
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
    use std::fs;
    use tempfile::tempdir;

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
            workflow: workflow.to_string(),
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
    fn test_detect_package_behavior_shape_for_run() {
        let temp_dir = tempdir().unwrap();
        fs::write(
            temp_dir.path().join(WORKFLOW_FILE_NAME),
            r#"
version: "1"
nodes:
  - id: install
    name: Install
    type: automatic
    steps:
      - id: install-skill
        name: Install
        install-skill:
          package: "@codemod/sample"
"#,
        )
        .unwrap();

        let shape = detect_package_behavior_shape_for_run(temp_dir.path(), None);
        assert_eq!(shape, PackageBehaviorShape::SkillOnly);
    }

    #[test]
    fn test_detect_package_behavior_shape_for_run_missing_without_workflow() {
        let temp_dir = tempdir().unwrap();

        let shape = detect_package_behavior_shape_for_run(temp_dir.path(), None);
        assert_eq!(shape, PackageBehaviorShape::Missing);
    }

    #[test]
    fn test_workflow_path_for_run_prefers_manifest_workflow_path() {
        let package_dir = Path::new("/tmp/sample");
        let manifest = manifest_with_workflow("custom/workflow.yaml");

        let workflow_path = workflow_path_for_run(package_dir, Some(&manifest));
        assert_eq!(workflow_path, package_dir.join("custom/workflow.yaml"));
    }

    #[test]
    fn test_skill_only_package_run_error_has_guidance() {
        let error = skill_only_package_run_error("@codemod/mcs", Path::new("/tmp/mcs"));
        let message = error.to_string();

        assert!(message.contains("skill-only package"));
        assert!(message.contains("install-skill"));
        assert!(message.contains("--skill"));
        assert!(message.contains("--project"));
        assert!(message.contains("@codemod/mcs"));
    }

    #[test]
    fn test_missing_workflow_error_mentions_expected_path() {
        let error = missing_workflow_error("@codemod/any", Path::new("/tmp/any/workflow.yaml"));
        let message = error.to_string();

        assert!(message.contains("missing required workflow file"));
        assert!(message.contains("/tmp/any/workflow.yaml"));
    }

    #[test]
    fn test_missing_behavior_error_mentions_install_skill_and_authored_dir() {
        let error = missing_behavior_run_error("@codemod/any", Path::new("/tmp/any"));
        let message = error.to_string();

        assert!(message.contains("no executable workflow steps"));
        assert!(message.contains("install-skill"));
        assert!(message.contains(AGENTS_SKILL_ROOT_RELATIVE_PATH));
    }

    #[test]
    fn test_skill_install_command_defaults_to_project_scope() {
        let command = skill_install_command("@codemod/jest-to-vitest");
        assert_eq!(
            command,
            "npx codemod @codemod/jest-to-vitest --skill --project"
        );
    }

    #[test]
    fn test_should_prompt_for_skill_install_disables_when_no_interactive() {
        assert!(!should_prompt_for_skill_install(true));
    }

    #[test]
    fn test_should_prompt_for_skill_install_with_tty_truth_table() {
        assert!(should_prompt_for_skill_install_with_tty(false, true, true));
        assert!(!should_prompt_for_skill_install_with_tty(true, true, true));
        assert!(!should_prompt_for_skill_install_with_tty(
            false, false, true
        ));
        assert!(!should_prompt_for_skill_install_with_tty(
            false, true, false
        ));
        assert!(!should_prompt_for_skill_install_with_tty(
            false, false, false
        ));
    }

    #[test]
    fn test_skill_install_prompt_message_by_context() {
        assert!(
            skill_install_prompt_message(SkillInstallOfferContext::SkillOnly)
                .contains("harness can execute")
        );
        assert!(
            skill_install_prompt_message(SkillInstallOfferContext::HybridPostRun)
                .contains("follow-up workflows")
        );
    }
}
