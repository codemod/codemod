use anyhow::Result;
use clap::Args;
use codemod_sandbox::sandbox::engine::ExecutionResult;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use ast_grep_language::SupportLang;
use codemod_sandbox::{
    sandbox::{
        engine::{execute_codemod_with_quickjs, language_data::get_extensions_for_language},
        filesystem::RealFileSystem,
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use testing_utils::{ReporterType, TestOptions, TestRunner, TestSource, TransformationResult};

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
}

pub async fn handler(args: &Command) -> Result<()> {
    let codemod_path = Path::new(&args.codemod_file);
    let test_directory = PathBuf::from(args.test_directory.as_deref().unwrap_or("tests"));

    if !codemod_path.exists() {
        anyhow::bail!("Codemod file '{}' does not exist", codemod_path.display());
    }

    let language_enum: SupportLang = args.language.parse()?;

    let reporter_type: ReporterType = args
        .reporter
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid reporter type: {}", e))?;

    let expect_errors = if let Some(patterns) = &args.expect_errors {
        patterns.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        Vec::new()
    };

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

    let filesystem = Arc::new(RealFileSystem::new());
    let script_base_dir = codemod_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let tsconfig_path = find_tsconfig(&script_base_dir);
    let resolver = Arc::new(OxcResolver::new(script_base_dir, tsconfig_path)?);

    let codemod_path_clone = codemod_path.to_path_buf();
    let execution_fn = Box::new(move |input_code: &str, input_path: &Path| {
        let codemod_path = codemod_path_clone.clone();
        let _filesystem = filesystem.clone();
        let resolver = resolver.clone();
        let input_code = input_code.to_string();
        let input_path = input_path.to_path_buf();

        Box::pin(async move {
            let execution_output = execute_codemod_with_quickjs(
                &codemod_path,
                resolver,
                language_enum,
                &input_path,
                &input_code,
                None,
            )
            .await?;

            match execution_output {
                ExecutionResult::Modified(content) => Ok(TransformationResult::Success(content)),
                // use input code as the output if the codemod was unmodified
                ExecutionResult::Unmodified | ExecutionResult::Skipped => {
                    Ok(TransformationResult::Success(input_code))
                }
            }
        })
            as Pin<
                Box<dyn std::future::Future<Output = Result<TransformationResult, anyhow::Error>>>,
            >
    });

    let test_source = TestSource::Directory(test_directory);

    let extensions = get_extensions_for_language(language_enum);

    let mut runner = TestRunner::new(options, test_source);
    let summary = runner.run_tests(&extensions, execution_fn).await?;

    if !summary.is_success() {
        std::process::exit(1);
    }

    Ok(())
}
