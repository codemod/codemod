use anyhow::Result;
use ast_grep_config::CombinedScan;
use ast_grep_language::SupportLang;
use butterflow_core::execution::CodemodExecutionConfig;
use clap::Args;
use codemod_sandbox::sandbox::engine::{extract_selector_with_quickjs, SelectorEngineOptions};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::scan_file_with_combined_scan;
use codemod_sandbox::utils::project_discovery::find_tsconfig;
use std::sync::Arc;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};

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

    /// Language to process
    #[arg(long)]
    pub language: String,

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

pub async fn handler(args: &Command) -> Result<()> {
    let js_file_path = Path::new(&args.js_file);
    let target_directory = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    // Verify the JavaScript file exists
    if !js_file_path.exists() {
        anyhow::bail!(
            "JavaScript file '{}' does not exist",
            js_file_path.display()
        );
    }

    // Set up the new modular system with OxcResolver
    let script_base_dir = js_file_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let tsconfig_path = find_tsconfig(&script_base_dir);

    let resolver = Arc::new(OxcResolver::new(script_base_dir.clone(), tsconfig_path)?);

    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        None,
        Some(script_base_dir.to_path_buf()),
    );

    let config = CodemodExecutionConfig {
        pre_run_callback: None,
        progress_callback: Arc::new(None),
        target_path: Some(target_directory.to_path_buf()),
        base_path: None,
        include_globs: None,
        exclude_globs: None,
        dry_run: false,
        languages: Some(vec![args.language.clone()]),
        threads: args.max_threads,
        capabilities: Some(capabilities),
    };

    let selector_config = extract_selector_with_quickjs(SelectorEngineOptions {
        script_path: js_file_path,
        language: args.language.parse().unwrap(),
        resolver: resolver.clone(),
        capabilities: config.capabilities.clone(),
        console_log_collector: None,
    })
    .await?;
    let combined_scan: Option<Arc<CombinedScan<SupportLang>>> = selector_config
        .as_ref()
        .map(|c| Arc::new(CombinedScan::new(vec![c])));

    let started = Instant::now();

    let combined_scan_cloned = combined_scan.clone();
    let _ = config.execute(move |file_path, _config| {
        // Only process files
        if !file_path.is_file() {
            return;
        }

        if let Some(cs) = &combined_scan_cloned {
            let result = scan_file_with_combined_scan(file_path, cs.as_ref(), false);
            if let Ok((matches, _, _)) = result {
                if !matches.is_empty() {
                    let file_path_string = file_path.display().to_string();
                    println!("[Applicable] {file_path_string}");
                }
            }
        }
    });

    let seconds = started.elapsed().as_millis() as f64 / 1000.0;
    println!("✨ Done in {seconds:.3}s");

    Ok(())
}
