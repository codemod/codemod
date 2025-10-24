use crate::engine::create_progress_callback;
use crate::TelemetrySenderMutex;
use crate::CLI_VERSION;
use crate::{capabilities_security_callback::capabilities_security_callback, dirty_git_check};
use anyhow::Result;
use butterflow_core::utils::generate_execution_id;
use butterflow_core::utils::parse_params;
use butterflow_core::{execution::CodemodExecutionConfig, execution::PreRunCallback};
use clap::Args;
use codemod_llrt_capabilities::module_builder::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::ExecutionResult;
use codemod_sandbox::sandbox::engine::JssgExecutionOptions;
use codemod_sandbox::sandbox::{
    engine::execute_codemod_with_quickjs, filesystem::RealFileSystem, resolvers::OxcResolver,
};
use codemod_sandbox::utils::project_discovery::find_tsconfig;
use codemod_telemetry::send_event::BaseEvent;
use log::{debug, error, warn};
use std::sync::Arc;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Instant,
};
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

    /// Parameters to pass to the codemod
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Option<Vec<String>>,

    /// Allow fs access
    #[arg(long)]
    pub allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    pub allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    pub allow_child_process: bool,
}

pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
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
    let _filesystem = Arc::new(RealFileSystem::new());
    let script_base_dir = js_file_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let tsconfig_path = find_tsconfig(&script_base_dir);

    let resolver = Arc::new(OxcResolver::new(script_base_dir.clone(), tsconfig_path)?);

    let mut capabilities = Vec::new();
    if args.allow_fs {
<<<<<<< HEAD
        capabilities.push(LlrtSupportedModules::Fs);
    }
    if args.allow_fetch {
        capabilities.push(LlrtSupportedModules::Fetch);
    }
    if args.allow_child_process {
        capabilities.push(LlrtSupportedModules::ChildProcess);
=======
        capabilities.push("fs".to_string());
    }
    if args.allow_fetch {
        capabilities.push("fetch".to_string());
    }
    if args.allow_child_process {
        capabilities.push("child_process".to_string());
>>>>>>> 722b83c9 (refactor: add capabilities feature for native jssg codemod run)
    }
    let capabilities = if capabilities.is_empty() {
        None
    } else {
<<<<<<< HEAD
        Some(capabilities.into_iter().collect())
=======
        Some(capabilities)
>>>>>>> 722b83c9 (refactor: add capabilities feature for native jssg codemod run)
    };

    let capabilities_security_callback = capabilities_security_callback();
    let pre_run_callback = PreRunCallback {
        callback: Arc::new(Box::new(move |_, _, config: &CodemodExecutionConfig| {
<<<<<<< HEAD
            capabilities_security_callback(config).unwrap_or_else(|e| {
                error!("Failed to check capabilities: {e}");
                std::process::exit(1);
            });
=======
            capabilities_security_callback(config);
>>>>>>> 722b83c9 (refactor: add capabilities feature for native jssg codemod run)
        })),
    };

    let config = CodemodExecutionConfig {
        pre_run_callback: Some(pre_run_callback),
        progress_callback: Arc::new(Some(create_progress_callback())),
        target_path: Some(target_directory.to_path_buf()),
        base_path: None,
        include_globs: None,
        exclude_globs: None,
        dry_run: args.dry_run,
        languages: Some(vec![args.language.clone()]),
        threads: args.max_threads,
        capabilities: capabilities.clone(),
    };

    let started = Instant::now();

    let params = parse_params(args.params.as_deref().unwrap_or(&[]))
        .map_err(|e| anyhow::anyhow!("Failed to parse parameters: {}", e))?;

    let capabilities_for_closure = config.capabilities.clone();
    let _ = config.execute(move |file_path, _config| {
        // Only process files
        if !file_path.is_file() {
            return;
        }

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

            let options = JssgExecutionOptions {
                script_path: js_file_path,
                resolver: resolver.clone(),
                language: args
                    .language
                    .clone()
                    .parse()
                    .unwrap_or_else(|_| panic!("Invalid language: {}", args.language)),
                file_path,
                content: &content,
                selector_config: None,
                params: Some(params.clone()),
                matrix_values: None,
<<<<<<< HEAD
                capabilities: capabilities_for_closure.as_deref().map(|v| v.to_vec()),
=======
                capabilities: config.capabilities.as_deref().map(|v| v.to_vec()),
>>>>>>> 722b83c9 (refactor: add capabilities feature for native jssg codemod run)
            };

            // Execute the codemod on this file
            match execute_codemod_with_quickjs(options).await {
                Ok(execution_output) => {
                    // Handle the execution output (write back if modified and not dry run)
                    if let ExecutionResult::Modified(ref new_content) = execution_output {
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
                Err(e) => {
                    error!(
                        "Failed to execute codemod on {}:\n{:?}",
                        file_path.display(),
                        e
                    );
                }
            }
        });
    });

    let seconds = started.elapsed().as_millis() as f64 / 1000.0;
    println!("✨ Done in {seconds:.3}s");

    // Generate a 20-byte execution ID (160 bits of entropy for collision resistance)
    let execution_id = generate_execution_id();

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
