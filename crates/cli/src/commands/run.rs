use crate::commands::publish::CodemodManifest;
use crate::engine::{create_engine, create_registry_client};
use crate::progress_bar::download_progress_bar;
use crate::workflow_runner::run_workflow;
use crate::TelemetrySenderMutex;
use crate::CLI_VERSION;
use anyhow::{anyhow, Result};
use butterflow_core::registry::RegistryError;
use butterflow_core::utils::generate_execution_id;
use butterflow_core::utils::parse_params;
use clap::Args;
use codemod_telemetry::send_event::BaseEvent;
use console::style;
use log::info;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::atomic::Ordering;
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
        Err(e) => return Err(anyhow::anyhow!("Registry error: {}", e)),
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

    let mut params = parse_params(args.params.as_deref().unwrap_or(&[]))
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

    let capabilities = codemod_config.and_then(|config| config.capabilities);

    if let Some(capabilities) = capabilities {
        let capabilities_str = capabilities.join(",");
        params.insert("capabilities".to_string(), capabilities_str);
    }

    // Run workflow using the extracted workflow runner
    let (engine, config) = create_engine(
        workflow_path,
        target_path,
        args.dry_run,
        args.allow_dirty,
        params,
        args.registry.clone(),
    )?;

    run_workflow(&engine, config).await?;

    telemetry
        .send_event(
            BaseEvent {
                kind: "failedToExecuteCommand".to_string(),
                properties: HashMap::from([
                    ("codemodName".to_string(), args.package.clone()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    (
                        "commandName".to_string(),
                        "codemod.executeCodemod".to_string(),
                    ),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;

    let stats = engine.execution_stats;
    let files_modified = stats.files_modified.load(Ordering::Relaxed);
    let files_unmodified = stats.files_unmodified.load(Ordering::Relaxed);
    let files_with_errors = stats.files_with_errors.load(Ordering::Relaxed);
    println!("\n📝 Modified files: {files_modified}");
    println!("✅ Unmodified files: {files_unmodified}");
    println!("❌ Files with errors: {files_with_errors}");

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

pub async fn run_legacy_codemod_with_raw_args(raw_args: &[String]) -> Result<()> {
    let mut cmd = if cfg!(target_os = "windows") {
        // On Windows, use cmd.exe to resolve npx properly
        let mut cmd = ProcessCommand::new("cmd");
        cmd.args(["/C", "npx", "codemod@legacy"]);
        cmd.args(raw_args);
        cmd
    } else {
        // On Unix systems, npx can be called directly
        let mut cmd = ProcessCommand::new("npx");
        cmd.arg("codemod@legacy");
        cmd.args(raw_args);
        cmd
    };

    info!(
        "Executing: {} with args: {:?}",
        if cfg!(target_os = "windows") {
            "cmd /C npx codemod@legacy"
        } else {
            "npx codemod@legacy"
        },
        cmd.get_args().collect::<Vec<_>>()
    );

    let status = cmd.status()?;

    if !status.success() {
        return Err(anyhow::anyhow!(
            "Legacy codemod command failed with exit code: {:?}",
            status.code()
        ));
    }

    Ok(())
}

async fn run_legacy_codemod(args: &Command, disable_analytics: bool) -> Result<()> {
    let mut legacy_args = vec![args.package.clone()];
    if let Some(target_path) = args.target_path.as_ref() {
        legacy_args.push(format!("--target {}", target_path.to_string_lossy()));
    }
    if disable_analytics {
        legacy_args.push("--no-telemetry".to_string());
    }
    run_legacy_codemod_with_raw_args(&legacy_args).await
}
