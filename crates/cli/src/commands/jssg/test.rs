use anyhow::Result;
use clap::Args;
use codemod_sandbox::sandbox::engine::{CodemodOutput, ExecutionResult, JssgExecutionOptions};
use codemod_sandbox::MetricsData;
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::CodemodLang;
use codemod_sandbox::MetricsContext;
use codemod_sandbox::{
    sandbox::{
        engine::{execute_codemod_with_quickjs, language_data::get_extensions_for_language},
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use testing_utils::{TestOptions, TestRunner, TestSource, TransformOutput, TransformationResult};

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

    /// Comparison strictness level: strict (string equality), cst (compare CSTs),
    /// ast (compare ASTs, ignores formatting), loose (compare AST, ignores ordering)
    #[arg(long, value_name = "LEVEL", default_value = "strict")]
    pub strictness: String,

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

    let default_language_enum: CodemodLang = default_language_str
        .parse()
        .map_err(|e: String| anyhow::anyhow!("{}", e))?;

    let strictness: testing_utils::Strictness = args
        .strictness
        .parse()
        .map_err(|e: String| anyhow::anyhow!("{}", e))?;

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
        strictness,
        language: global_config.language.clone(),
        expected_extension: global_config.expected_extension.clone(),
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
    let update_snapshots = args.update_snapshots;
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
                let language_enum: CodemodLang = language_str
                    .parse()
                    .map_err(|e: String| anyhow::anyhow!("{}", e))?;

                let metrics_context = MetricsContext::new();

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
                    metrics_context: Some(metrics_context.clone()),
                    test_mode: true,
                    target_directory: None,
                };
                let CodemodOutput { primary, .. } = execute_codemod_with_quickjs(options).await?;

                // Handle metrics snapshot
                let metrics_data = metrics_context.get_all();
                let metrics_path = test_case_dir.join("metrics.json");

                if !metrics_data.is_empty() {
                    let actual_json = metrics_to_canonical_json(&metrics_data)?;

                    if metrics_path.exists() {
                        let expected_json = std::fs::read_to_string(&metrics_path)?;
                        if actual_json != expected_json {
                            if update_snapshots {
                                std::fs::write(&metrics_path, &actual_json)?;
                            } else {
                                anyhow::bail!(
                                    "Metrics mismatch:\n--- expected\n+++ actual\n{}",
                                    generate_metrics_diff(&expected_json, &actual_json)
                                );
                            }
                        }
                    } else {
                        std::fs::write(&metrics_path, &actual_json)?;
                    }
                } else if metrics_path.exists() {
                    // Codemod produced no metrics but a snapshot exists — stale snapshot
                    if update_snapshots {
                        std::fs::remove_file(&metrics_path)?;
                    } else {
                        anyhow::bail!(
                            "Metrics snapshot exists at {} but codemod produced no metrics. \
                             Run with --update-snapshots to remove the stale snapshot.",
                            metrics_path.display()
                        );
                    }
                }

                match primary {
                    ExecutionResult::Modified(modified) => {
                        Ok(TransformationResult::Success(TransformOutput {
                            content: modified.content,
                            rename_to: modified.rename_to,
                        }))
                    }
                    ExecutionResult::Unmodified | ExecutionResult::Skipped => {
                        Ok(TransformationResult::Success(TransformOutput {
                            content: input_code,
                            rename_to: None,
                        }))
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

/// Serialize MetricsData to a canonical JSON string using RFC 8785 (JCS).
/// Deterministic regardless of HashMap iteration order.
fn metrics_to_canonical_json(metrics: &MetricsData) -> Result<String> {
    let json_value = serde_json::to_value(metrics)?;
    let canonical = String::from_utf8(serde_json_canonicalizer::to_vec(&json_value)?)?;
    // Re-parse and pretty-print the canonicalized JSON
    let reparsed: serde_json::Value = serde_json::from_str(&canonical)?;
    let pretty = serde_json::to_string_pretty(&reparsed)?;
    Ok(pretty)
}

fn generate_metrics_diff(expected: &str, actual: &str) -> String {
    use similar::{ChangeTag, TextDiff};
    use std::fmt::Write;

    let diff = TextDiff::from_lines(expected, actual);
    let mut result = String::new();

    for group in diff.grouped_ops(3) {
        for op in &group {
            for change in diff.iter_changes(op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                let _ = write!(result, "{sign}{change}");
            }
        }
    }

    result
}
