use anyhow::Result;
use clap::Args;
use codemod_sandbox::sandbox::engine::{ExecutionResult, JssgExecutionOptions};
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use ast_grep_language::SupportLang;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::{
    sandbox::{
        engine::{execute_codemod_with_quickjs, language_data::get_extensions_for_language},
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use testing_utils::{TestOptions, TestRunner, TestSource, TransformationResult};

use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};

use super::config::{ResolvedTestConfig, TestConfig};

#[derive(Args, Debug, Clone)]
pub struct Command {
    /// Path to the codemod file to test
    pub codemod_file: String,

    /// Test directory containing test fixtures (default: tests)
    pub test_directory: Option<String>,

    /// Language to process (can be specified in config file)
    #[arg(long, short)]
    pub language: Option<String>,

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

    /// Enable workspace-wide semantic analysis for cross-file references.
    /// Uses the provided path as workspace root.
    #[arg(long)]
    pub semantic_workspace: Option<PathBuf>,

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

    if !codemod_path.exists() {
        anyhow::bail!("Codemod file '{}' does not exist", codemod_path.display());
    }

    std::env::set_var("CODEMOD_STEP_ID", "jssg");

    let current_dir = std::env::current_dir()?;
    let base_config = TestConfig::load_hierarchical(&current_dir, None)?;

    let test_directory = PathBuf::from(args.test_directory.as_deref().unwrap_or("tests"));

    let global_config = ResolvedTestConfig::resolve(args, &base_config, None)?;

    let default_language_str = global_config.language.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "Language must be specified either via --language argument or in a config file"
        )
    })?;

    let default_language_enum: SupportLang = default_language_str.parse()?;

    let options = TestOptions {
        filter: global_config.filter,
        update_snapshots: global_config.update_snapshots,
        verbose: global_config.verbose,
        parallel: !global_config.sequential,
        max_threads: global_config.max_threads,
        fail_fast: global_config.fail_fast,
        watch: global_config.watch,
        reporter: global_config.reporter,
        timeout: std::time::Duration::from_secs(global_config.timeout),
        ignore_whitespace: global_config.ignore_whitespace,
        context_lines: global_config.context_lines,
        expect_errors: global_config.expect_errors,
    };

    let script_base_dir = codemod_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    // Create and run test runner
    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        None,
        Some(script_base_dir.to_path_buf()),
    );

    let tsconfig_path = find_tsconfig(&script_base_dir);
    let resolver = Arc::new(OxcResolver::new(script_base_dir, tsconfig_path)?);

    let codemod_path_clone = codemod_path.to_path_buf();
    let base_config_clone = base_config.clone();
    let args_clone = args.clone();
    let current_dir_clone = current_dir.clone();
    let semantic_provider: Option<Arc<dyn SemanticProvider>> =
        if let Some(workspace_root) = &args.semantic_workspace {
            Some(Arc::new(LazySemanticProvider::workspace_scope(
                workspace_root.clone(),
            )))
        } else {
            Some(Arc::new(LazySemanticProvider::file_scope()))
        };
    let execution_fn = Box::new(
        move |input_code: &str,
              input_path: &Path,
              capabilities: Option<HashSet<LlrtSupportedModules>>| {
            let codemod_path = codemod_path_clone.clone();
            let resolver = resolver.clone();
            let input_code = input_code.to_string();
            let input_path = input_path.to_path_buf();
            let base_config = base_config_clone.clone();
            let args = args_clone.clone();
            let current_dir = current_dir_clone.clone();
            let semantic_provider = semantic_provider.clone();

            Box::pin(async move {
                let test_case_dir = input_path.parent().unwrap_or(input_path.as_path());
                let per_test_config =
                    TestConfig::load_hierarchical(test_case_dir, Some(current_dir.as_path()))?;

                let test_config =
                    ResolvedTestConfig::resolve(&args, &base_config, Some(&per_test_config))?;

                let language_str = test_config
                    .language
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Language must be specified for test case"))?;
                let language_enum: SupportLang = language_str.parse()?;

                let options = JssgExecutionOptions {
                    script_path: &codemod_path,
                    resolver,
                    language: language_enum,
                    file_path: &input_path,
                    content: &input_code,
                    selector_config: None,
                    params: test_config.params,
                    matrix_values: None,
                    capabilities,
                    semantic_provider,
                    console_log_collector: None,
                };
                let execution_output = execute_codemod_with_quickjs(options).await?;

                match execution_output {
                    ExecutionResult::Modified(content) => {
                        Ok(TransformationResult::Success(content))
                    }
                    ExecutionResult::Unmodified | ExecutionResult::Skipped => {
                        Ok(TransformationResult::Success(input_code))
                    }
                }
            })
                as Pin<
                    Box<
                        dyn std::future::Future<
                            Output = Result<TransformationResult, anyhow::Error>,
                        >,
                    >,
                >
        },
    );

    let test_source = TestSource::Directory(test_directory);

    let extensions = get_extensions_for_language(default_language_enum);

    let mut runner = TestRunner::new(options, test_source);
    let summary = runner
        .run_tests(&extensions, execution_fn, Some(capabilities))
        .await?;

    if !summary.is_success() {
        std::process::exit(1);
    }

    Ok(())
}
