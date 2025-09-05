use anyhow::Result;
use clap::Args;
use std::path::{Path, PathBuf};

use crate::commands::jssg::testing::{
    config::{ReporterType, TestOptions},
    runner::TestRunner,
};

#[derive(Args, Debug)]
pub struct Command {
    /// Path to the codemod file to test
    pub codemod_file: String,

    /// Test directory containing test fixtures (default: tests)
    pub test_directory: Option<String>,

    /// Language to process (required)
    #[arg(long, short)]
    pub language: String,

    /// Run only tests matching the pattern
    #[arg(long)]
    pub filter: Option<String>,

    /// Update expected outputs with actual results
    #[arg(long, short)]
    pub update_snapshots: bool,

    /// Show detailed output for each test
    #[arg(long, short)]
    pub verbose: bool,

    /// Run tests sequentially instead of in parallel
    #[arg(long)]
    pub sequential: bool,

    /// Maximum number of concurrent test threads
    #[arg(long)]
    pub max_threads: Option<usize>,

    /// Stop on first test failure
    #[arg(long)]
    pub fail_fast: bool,

    /// Watch for file changes and re-run tests
    #[arg(long)]
    pub watch: bool,

    /// Output format (console, json, terse)
    #[arg(long, default_value = "console")]
    pub reporter: String,

    /// Test timeout in seconds (default: 30)
    #[arg(long, default_value = "30")]
    pub timeout: u64,

    /// Ignore whitespace differences in comparisons
    #[arg(long)]
    pub ignore_whitespace: bool,

    /// Number of context lines in diff output (default: 3)
    #[arg(long, default_value = "3")]
    pub context_lines: usize,

    /// Test patterns that are expected to produce errors (comma-separated)
    #[arg(long)]
    pub expect_errors: Option<String>,

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
    let codemod_path = Path::new(&args.codemod_file);
    let test_directory = PathBuf::from(args.test_directory.as_deref().unwrap_or("tests"));

    // Verify the codemod file exists
    if !codemod_path.exists() {
        anyhow::bail!("Codemod file '{}' does not exist", codemod_path.display());
    }

    // Parse reporter type
    let reporter_type: ReporterType = args
        .reporter
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid reporter type: {}", e))?;

    // Parse expect_errors patterns
    let expect_errors = if let Some(patterns) = &args.expect_errors {
        patterns.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        Vec::new()
    };

    // Build test options
    let options = TestOptions {
        filter: args.filter.clone(),
        update_snapshots: args.update_snapshots,
        verbose: args.verbose,
        parallel: !args.sequential,
        max_threads: args.max_threads,
        fail_fast: args.fail_fast,
        watch: args.watch,
        reporter: reporter_type,
        timeout: std::time::Duration::from_secs(args.timeout),
        ignore_whitespace: args.ignore_whitespace,
        context_lines: args.context_lines,
        expect_errors,
    };

    // Create and run test runner
    let mut capabilities = Vec::new();
    if args.allow_fs {
        capabilities.push("fs".to_string());
    }
    if args.allow_fetch {
        capabilities.push("fetch".to_string());
    }
    if args.allow_child_process {
        capabilities.push("child_process".to_string());
    }
    let capabilities = if capabilities.is_empty() {
        None
    } else {
        Some(capabilities)
    };
    let mut runner = TestRunner::new(options, test_directory);
    let summary = runner
        .run_tests(codemod_path, &args.language, capabilities.clone())
        .await?;

    // Display test summary
    summary.display_summary();

    // Exit with error code if tests failed
    if !summary.is_success() {
        std::process::exit(1);
    }

    Ok(())
}
