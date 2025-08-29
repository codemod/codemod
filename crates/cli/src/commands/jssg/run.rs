use anyhow::Result;
use butterflow_core::execution::CodemodExecutionConfig;
use clap::Args;
use codemod_sandbox::sandbox::{
    engine::execute_codemod_with_quickjs, filesystem::RealFileSystem, resolvers::OxcResolver,
};
use log::{debug, error, info, warn};
use rand::Rng;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use crate::dirty_git_check;
use crate::engine::create_progress_callback;
use codemod_sandbox::utils::project_discovery::find_tsconfig;
use codemod_telemetry::send_event::{BaseEvent, TelemetrySender};

#[derive(Args, Debug)]
pub struct Command {
    /// Path to the JavaScript file to execute
    pub js_file: String,

    /// Optional target path to run the codemod on (default: current directory)
    #[arg(long = "target", short = 't')]
    pub target_path: Option<PathBuf>,

    /// Set maximum number of concurrent threads (default: CPU cores)
    #[arg(long)]
    pub max_threads: Option<usize>,

    /// Perform a dry run without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Language to process
    #[arg(long)]
    pub language: String,

    /// Allow dirty git status
    #[arg(long)]
    pub allow_dirty: bool,
}

pub async fn handler(args: &Command, telemetry: &dyn TelemetrySender) -> Result<()> {
    let js_file_path = Path::new(&args.js_file);
    let target_directory = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    let dirty_check = dirty_git_check::dirty_check();
    dirty_check(&target_directory, args.allow_dirty);

    // Verify the JavaScript file exists
    if !js_file_path.exists() {
        anyhow::bail!(
            "JavaScript file '{}' does not exist",
            js_file_path.display()
        );
    }

    // Set up the new modular system with OxcResolver
    let filesystem = Arc::new(RealFileSystem::new());
    let script_base_dir = js_file_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let tsconfig_path = find_tsconfig(&script_base_dir);

    let resolver = Arc::new(OxcResolver::new(script_base_dir.clone(), tsconfig_path)?);

    let config = CodemodExecutionConfig {
        pre_run_callback: None,
        progress_callback: Arc::new(Some(create_progress_callback())),
        target_path: Some(target_directory.to_path_buf()),
        base_path: None,
        include_globs: None,
        exclude_globs: None,
        dry_run: args.dry_run,
        languages: Some(vec![args.language.clone()]),
    };

    let started = Instant::now();

    let _ = config.execute(|file_path, _config| {
        // Only process files
        if !file_path.is_file() {
            return;
        }

        info!("Processing file with JS AST grep: {}", file_path.display());

        // Use a tokio runtime to handle the async execution within the sync callback
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Read file content
            let content = match tokio::fs::read_to_string(&file_path).await {
                Ok(content) => content,
                Err(e) => {
                    warn!("Failed to read file {}: {}", file_path.display(), e);
                    return;
                }
            };

            // Execute the codemod on this file
            match execute_codemod_with_quickjs(
                js_file_path,
                filesystem.clone(),
                resolver.clone(),
                args.language
                    .clone()
                    .parse()
                    .unwrap_or_else(|_| panic!("Invalid language: {}", args.language)),
                file_path,
                &content,
            )
            .await
            {
                Ok(execution_output) => {
                    // Handle the execution output (write back if modified and not dry run)
                    if let Some(ref new_content) = execution_output.content {
                        if new_content != &content {
                            if !config.dry_run {
                                if let Err(e) = tokio::fs::write(&file_path, new_content).await {
                                    error!(
                                        "Failed to write modified file {}: {}",
                                        file_path.display(),
                                        e
                                    );
                                } else {
                                    debug!("Modified file: {}", file_path.display());
                                }
                            } else if config.dry_run {
                                debug!("Would modify file (dry run): {}", file_path.display());
                            }
                        }
                    }

                    // Handle execution errors
                    if let Some(ref error_msg) = execution_output.error {
                        warn!(
                            "Execution completed with error for {}: {}",
                            file_path.display(),
                            error_msg
                        );
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to execute codemod on {}: {:?}",
                        file_path.display(),
                        e
                    );
                }
            }
        });
    });

    let seconds = started.elapsed().as_millis() as f64 / 1000.0;
    println!("✨ Done in {seconds:.3}s");

    let execution_id: [u8; 20] = rand::thread_rng().gen();
    let execution_id = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        execution_id,
    );
    let cli_version = env!("CARGO_PKG_VERSION");

    telemetry
        .send_event(
            BaseEvent {
                kind: "localJssgExecuted".to_string(),
                properties: HashMap::from([
                    ("executionId".to_string(), execution_id.clone()),
                    ("runTimeSeconds".to_string(), seconds.to_string()),
                    ("language".to_string(), args.language.clone()),
                    ("dirtyRun".to_string(), args.allow_dirty.to_string()),
                    ("dryRun".to_string(), args.dry_run.to_string()),
                    ("cliVersion".to_string(), cli_version.to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                ]),
            },
            None,
        )
        .await;

    Ok(())
}
