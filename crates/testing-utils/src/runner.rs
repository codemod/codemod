use anyhow::{Context, Result};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::ExecutionResult;
use libtest_mimic::{run, Trial};
use similar::TextDiff;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::{collections::HashSet, future::Future};
use tempfile::TempDir;
use tokio::time::timeout;
use walkdir::WalkDir;

use crate::{
    config::{Strictness, TestOptions},
    fixtures::{FileSystemTestCase, FileSystemTestCaseLayout, TestSource, UnifiedTestCase},
    strictness::{ast_compare, cst_compare, detect_language, loose_compare},
};

/// Output of a successful transformation
#[derive(Debug, Clone)]
pub struct TransformOutput {
    pub content: String,
    pub rename_to: Option<std::path::PathBuf>,
}

/// Result of executing a transformation on input code
#[derive(Debug, Clone)]
pub enum TransformationResult {
    Success(TransformOutput),
    Error(String),
}

pub fn map_execution_result(
    primary: ExecutionResult,
    original_content: String,
) -> TransformationResult {
    match primary {
        ExecutionResult::Modified(modified) => TransformationResult::Success(TransformOutput {
            content: modified.content,
            rename_to: modified.rename_to,
        }),
        ExecutionResult::Unmodified | ExecutionResult::Skipped => {
            TransformationResult::Success(TransformOutput {
                content: original_content,
                rename_to: None,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    pub input_code: String,
    pub input_path: PathBuf,
    pub logical_input_path: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
}

/// Execution function type - takes input code and file path, returns transformation result
pub type ExecutionFn<'a> = Box<
    dyn Fn(
            ExecutionRequest,
            Option<HashSet<LlrtSupportedModules>>,
        ) -> Pin<Box<dyn Future<Output = Result<TransformationResult>> + 'a>>
        + 'a,
>;

/// Individual test result with name and optional error message.
#[derive(Debug, Clone)]
pub struct TestResultDetail {
    pub name: String,
    pub passed: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub ignored: usize,
    /// Detailed results for each test, including error messages for failures.
    pub details: Vec<TestResultDetail>,
}

impl TestSummary {
    pub fn from_libtest_result(result: libtest_mimic::Conclusion) -> Self {
        let total =
            (result.num_filtered_out + result.num_passed + result.num_failed + result.num_ignored)
                as usize;
        let passed = result.num_passed as usize;
        let failed = result.num_failed as usize;
        let ignored = result.num_ignored as usize;

        Self {
            total,
            passed,
            failed,
            errors: 0,
            ignored,
            details: Vec::new(),
        }
    }

    /// Check if all tests passed
    pub fn is_success(&self) -> bool {
        self.failed == 0 && self.errors == 0
    }
}

pub struct TestRunner {
    options: TestOptions,
    test_source: TestSource,
}

#[derive(Debug)]
struct AssertionResult {
    name: String,
    result: Result<()>,
}

struct DirectoryExecution {
    assertion_results: Vec<AssertionResult>,
    fixed_results: HashMap<PathBuf, Result<()>>,
    stopped_early: bool,
}

enum DiscoveredTestCase {
    Unified(UnifiedTestCase),
    Directory(FileSystemTestCase),
}

impl TestRunner {
    pub fn new(options: TestOptions, test_source: TestSource) -> Self {
        Self {
            options,
            test_source,
        }
    }

    /// Run tests with the provided execution function
    /// The extensions parameter should be a list of file extensions to look for (e.g., [".js", ".ts"])
    pub async fn run_tests<'a>(
        &mut self,
        extensions: &[&str],
        execution_fn: ExecutionFn<'a>,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<TestSummary> {
        if self.options.watch {
            return self
                .run_with_watch(extensions, execution_fn, capabilities)
                .await;
        }

        self.run_tests_once(extensions, execution_fn, capabilities)
            .await
    }

    async fn run_tests_once<'a>(
        &mut self,
        extensions: &[&str],
        execution_fn: ExecutionFn<'a>,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<TestSummary> {
        let discovered_cases = self
            .discover_test_cases(extensions)
            .map_err(|e| anyhow::anyhow!("Failed to load test cases: {}", e))?;
        let discovered_cases = self.filter_discovered_cases(discovered_cases);

        if discovered_cases.is_empty() {
            if let Some(filter) = &self.options.filter {
                return Err(anyhow::anyhow!(
                    "No test cases match the filter '{}'",
                    filter
                ));
            }
            return Err(anyhow::anyhow!("No test cases found"));
        }

        let mut test_results = Vec::new();
        for discovered_case in discovered_cases {
            let case_results = match discovered_case {
                DiscoveredTestCase::Unified(test_case) => {
                    let result = timeout(
                        self.options.timeout,
                        Self::execute_test_case(
                            &test_case,
                            &execution_fn,
                            &self.options,
                            capabilities.clone(),
                        ),
                    )
                    .await;

                    let final_result = result.unwrap_or_else(|_| {
                        Err(anyhow::anyhow!(
                            "Test '{}' timed out after {:?}",
                            test_case.name,
                            self.options.timeout
                        ))
                    });

                    vec![AssertionResult {
                        name: test_case.name.clone(),
                        result: final_result,
                    }]
                }
                DiscoveredTestCase::Directory(test_case) => {
                    Self::execute_directory_test_case(
                        &test_case,
                        &execution_fn,
                        &self.options,
                        capabilities.clone(),
                    )
                    .await?
                }
            };

            let filtered_results = self.filter_assertion_results(case_results);
            if filtered_results.is_empty() {
                continue;
            }

            let failed = filtered_results.iter().any(|result| result.result.is_err());
            test_results.extend(
                filtered_results
                    .into_iter()
                    .map(|result| (result.name, result.result)),
            );

            if self.options.should_fail_fast() && failed {
                println!("Stopping test execution due to --fail-fast and test failure");
                break;
            }
        }

        // Capture detailed results before passing to libtest_mimic
        let details: Vec<TestResultDetail> = test_results
            .iter()
            .map(|(name, result)| TestResultDetail {
                name: name.clone(),
                passed: result.is_ok(),
                error_message: result.as_ref().err().map(|e| e.to_string()),
            })
            .collect();

        let trials: Vec<Trial> = test_results
            .into_iter()
            .map(|(name, result)| {
                Trial::test(name, move || {
                    result.map_err(|e| libtest_mimic::Failed::from(format!("{e}")))
                })
            })
            .collect();

        let mut args = self.options.to_libtest_args();

        args.filter = None;

        let result = run(&args, trials);

        let mut summary = TestSummary::from_libtest_result(result);
        summary.details = details;
        Ok(summary)
    }

    async fn execute_test_case<'a>(
        test_case: &UnifiedTestCase,
        execution_fn: &ExecutionFn<'a>,
        options: &TestOptions,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<()> {
        let should_expect_error = test_case.should_expect_error(&options.expect_errors);

        let input_path = test_case
            .input_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("test_input"));
        let execution_result = execution_fn(
            ExecutionRequest {
                input_code: test_case.input_code.clone(),
                input_path,
                logical_input_path: test_case
                    .logical_input_path
                    .clone()
                    .or_else(|| test_case.input_path.clone()),
                workspace_root: None,
            },
            capabilities.clone(),
        )
        .await?;

        if should_expect_error {
            match execution_result {
                TransformationResult::Error(_) => {
                    println!("Test '{}' failed as expected", test_case.name);
                    return Ok(());
                }
                TransformationResult::Success(_) => {
                    return Err(anyhow::anyhow!(
                        "Test '{}' was expected to fail but succeeded",
                        test_case.name
                    ));
                }
            }
        }

        let actual_content = match execution_result {
            TransformationResult::Success(output) => output.content,
            TransformationResult::Error(error) => {
                return Err(anyhow::anyhow!(
                    "Transformation execution failed:\n{}",
                    error
                ));
            }
        };

        if !Self::contents_match(
            &test_case.expected_output_code,
            &actual_content,
            options,
            test_case
                .logical_input_path
                .as_deref()
                .or(test_case.input_path.as_deref()),
        ) {
            if options.update_snapshots {
                test_case
                    .update_expected_output(&actual_content)
                    .with_context(|| {
                        format!("Failed to update snapshot for test '{}'", test_case.name)
                    })?;
                return Ok(());
            } else {
                let diff =
                    Self::generate_diff(&test_case.expected_output_code, &actual_content, options);
                return Err(anyhow::anyhow!(
                    "Output mismatch for test '{}':\n{}",
                    test_case.name,
                    diff
                ));
            }
        }

        Ok(())
    }

    fn discover_test_cases(
        &self,
        extensions: &[&str],
    ) -> std::result::Result<Vec<DiscoveredTestCase>, crate::fixtures::TestError> {
        match &self.test_source {
            TestSource::Cases(_) => Ok(self
                .test_source
                .to_unified_test_cases(extensions, self.options.expected_extension.as_deref())?
                .into_iter()
                .map(DiscoveredTestCase::Unified)
                .collect()),
            TestSource::Directory(dir) => {
                let fs_cases = FileSystemTestCase::discover_in_directory(
                    dir,
                    extensions,
                    self.options.expected_extension.as_deref(),
                )?;
                let unified_cases = FileSystemTestCase::into_unified_cases(
                    fs_cases.clone(),
                    self.options.expected_extension.as_deref(),
                );

                let mut discovered = unified_cases
                    .into_iter()
                    .map(DiscoveredTestCase::Unified)
                    .collect::<Vec<_>>();

                discovered.extend(
                    fs_cases
                        .into_iter()
                        .filter(|case| {
                            matches!(
                                case.layout,
                                FileSystemTestCaseLayout::DirectorySnapshot { .. }
                            )
                        })
                        .map(DiscoveredTestCase::Directory),
                );

                discovered.sort_by(|left, right| {
                    let left_name = match left {
                        DiscoveredTestCase::Unified(test_case) => test_case.name.as_str(),
                        DiscoveredTestCase::Directory(test_case) => test_case.name.as_str(),
                    };
                    let right_name = match right {
                        DiscoveredTestCase::Unified(test_case) => test_case.name.as_str(),
                        DiscoveredTestCase::Directory(test_case) => test_case.name.as_str(),
                    };
                    left_name.cmp(right_name)
                });

                Ok(discovered)
            }
        }
    }

    fn filter_discovered_cases(&self, cases: Vec<DiscoveredTestCase>) -> Vec<DiscoveredTestCase> {
        let Some(filter) = &self.options.filter else {
            return cases;
        };

        cases
            .into_iter()
            .filter(|case| Self::discovered_case_matches_filter(case, filter))
            .collect()
    }

    fn discovered_case_matches_filter(case: &DiscoveredTestCase, filter: &str) -> bool {
        match case {
            DiscoveredTestCase::Unified(test_case) => test_case.name.contains(filter),
            DiscoveredTestCase::Directory(test_case) => {
                Self::planned_directory_assertion_names(test_case)
                    .into_iter()
                    .any(|name| name.contains(filter))
            }
        }
    }

    fn filter_assertion_results(&self, results: Vec<AssertionResult>) -> Vec<AssertionResult> {
        if let Some(filter) = &self.options.filter {
            results
                .into_iter()
                .filter(|result| result.name.contains(filter))
                .collect()
        } else {
            results
        }
    }

    async fn execute_directory_test_case<'a>(
        test_case: &FileSystemTestCase,
        execution_fn: &ExecutionFn<'a>,
        options: &TestOptions,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<Vec<AssertionResult>> {
        let (input_dir, expected_dir) = Self::directory_layout(test_case)?;
        let workspace = Self::create_directory_workspace(input_dir)?;
        let mut execution = Self::execute_directory_entrypoints(
            test_case,
            input_dir,
            workspace.path(),
            execution_fn,
            options,
            capabilities,
        )
        .await?;

        if execution.stopped_early {
            return Ok(execution.assertion_results);
        }

        let actual_files = Self::collect_workspace_files(workspace.path())?;
        let known_paths = Self::directory_assertion_paths(test_case, &actual_files);
        for relative_path in known_paths {
            let result = if let Some(result) = execution.fixed_results.remove(&relative_path) {
                result
            } else {
                Self::evaluate_directory_assertion(
                    test_case,
                    expected_dir,
                    workspace.path(),
                    &actual_files,
                    &relative_path,
                    options,
                )?
            };

            if Self::push_directory_assertion(
                &mut execution.assertion_results,
                &test_case.name,
                relative_path,
                result,
                options,
            ) {
                break;
            }
        }

        Ok(execution.assertion_results)
    }

    fn planned_directory_assertion_names(test_case: &FileSystemTestCase) -> Vec<String> {
        let mut known_paths: BTreeSet<PathBuf> = Self::tracked_paths(&test_case.input_files);
        known_paths.extend(Self::tracked_paths(&test_case.expected_files));
        known_paths
            .into_iter()
            .map(|relative_path| {
                Self::format_directory_assertion_name(&test_case.name, &relative_path)
            })
            .collect()
    }

    fn directory_layout(test_case: &FileSystemTestCase) -> Result<(&Path, &Path)> {
        match &test_case.layout {
            FileSystemTestCaseLayout::DirectorySnapshot {
                input_dir,
                expected_dir,
            } => Ok((input_dir.as_path(), expected_dir.as_path())),
            FileSystemTestCaseLayout::SingleFile => {
                Err(anyhow::anyhow!("expected directory snapshot test case"))
            }
        }
    }

    fn create_directory_workspace(input_dir: &Path) -> Result<TempDir> {
        let workspace = TempDir::new().context("Failed to create temp test workspace")?;
        Self::copy_directory_contents(input_dir, workspace.path())
            .context("Failed to copy fixture input into temp workspace")?;
        Ok(workspace)
    }

    async fn execute_directory_entrypoints<'a>(
        test_case: &FileSystemTestCase,
        input_dir: &Path,
        workspace_root: &Path,
        execution_fn: &ExecutionFn<'a>,
        options: &TestOptions,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<DirectoryExecution> {
        let mut execution = DirectoryExecution {
            assertion_results: Vec::new(),
            fixed_results: HashMap::new(),
            stopped_early: false,
        };

        for relative_path in &test_case.entrypoint_files {
            let Some(result) = Self::execute_directory_entrypoint(
                test_case,
                input_dir,
                workspace_root,
                relative_path,
                execution_fn,
                options,
                capabilities.clone(),
            )
            .await?
            else {
                continue;
            };

            if Self::store_directory_entrypoint_result(
                &mut execution,
                &test_case.name,
                relative_path,
                result,
                options,
            ) {
                execution.stopped_early = true;
                return Ok(execution);
            }
        }

        Ok(execution)
    }

    async fn execute_directory_entrypoint<'a>(
        test_case: &FileSystemTestCase,
        input_dir: &Path,
        workspace_root: &Path,
        relative_path: &Path,
        execution_fn: &ExecutionFn<'a>,
        options: &TestOptions,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<Option<Result<()>>> {
        let actual_path = workspace_root.join(relative_path);
        if !actual_path.exists() {
            return Ok(None);
        }

        let input_code = fs::read_to_string(&actual_path)
            .with_context(|| format!("Failed to read test input '{}'", actual_path.display()))?;
        let assertion_name = Self::format_directory_assertion_name(&test_case.name, relative_path);
        let should_expect_error = Self::should_expect_error_name(
            &assertion_name,
            test_case.should_error,
            &options.expect_errors,
        );

        let execution_result = timeout(
            options.timeout,
            execution_fn(
                ExecutionRequest {
                    input_code,
                    input_path: actual_path.clone(),
                    logical_input_path: Some(input_dir.join(relative_path)),
                    workspace_root: Some(workspace_root.to_path_buf()),
                },
                capabilities,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "Test '{}' timed out after {:?}",
                assertion_name,
                options.timeout
            ))
        });

        let result = match execution_result {
            Err(error) => Err(error),
            Ok(TransformationResult::Error(_)) if should_expect_error => Ok(()),
            Ok(TransformationResult::Error(error)) => Err(anyhow::anyhow!(
                "Transformation execution failed:\n{}",
                error
            )),
            Ok(TransformationResult::Success(_)) if should_expect_error => Err(anyhow::anyhow!(
                "Test '{}' was expected to fail but succeeded",
                assertion_name
            )),
            Ok(TransformationResult::Success(output)) => {
                Self::apply_transformation_output(&actual_path, output)?;
                return Ok(None);
            }
        };

        Ok(Some(result))
    }

    fn directory_assertion_paths(
        test_case: &FileSystemTestCase,
        actual_files: &HashMap<PathBuf, String>,
    ) -> BTreeSet<PathBuf> {
        let input_paths = Self::tracked_paths(&test_case.input_files);
        let expected_paths = Self::tracked_paths(&test_case.expected_files);
        let mut known_paths: BTreeSet<PathBuf> =
            input_paths.union(&expected_paths).cloned().collect();
        known_paths.extend(
            actual_files
                .keys()
                .filter(|path| !expected_paths.contains(*path) && !input_paths.contains(*path))
                .cloned(),
        );
        known_paths
    }

    fn evaluate_directory_assertion(
        test_case: &FileSystemTestCase,
        expected_dir: &Path,
        workspace_root: &Path,
        actual_files: &HashMap<PathBuf, String>,
        relative_path: &Path,
        options: &TestOptions,
    ) -> Result<Result<()>> {
        let assertion_name = Self::format_directory_assertion_name(&test_case.name, relative_path);
        let actual_path = workspace_root.join(relative_path);

        Ok(
            match (
                test_case.input_files.get(relative_path),
                test_case.expected_files.get(relative_path),
                actual_files.get(relative_path),
            ) {
                (Some(_), Some(expected_file), Some(actual_content))
                | (None, Some(expected_file), Some(actual_content)) => {
                    Self::assert_expected_content(
                        expected_file,
                        actual_content,
                        Some(actual_path.as_path()),
                        options,
                        assertion_name.as_str(),
                    )
                }
                (Some(_), Some(expected_file), None) => Self::remove_expected_or_fail(
                    assertion_name.as_str(),
                    options,
                    format!(
                        "Expected file '{}' is missing from actual output",
                        relative_path.display()
                    ),
                    || Self::remove_expected_snapshot(&expected_file.path, expected_dir),
                )?,
                (None, Some(expected_file), None) => Self::remove_expected_or_fail(
                    assertion_name.as_str(),
                    options,
                    format!(
                        "Expected created file '{}' was not produced",
                        relative_path.display()
                    ),
                    || Self::remove_expected_snapshot(&expected_file.path, expected_dir),
                )?,
                (Some(_), None, None) => Ok(()),
                (Some(_), None, Some(actual_content)) => Self::write_expected_or_fail(
                    assertion_name.as_str(),
                    options,
                    format!(
                        "Expected file '{}' to be deleted but it still exists",
                        relative_path.display()
                    ),
                    || Self::write_expected_snapshot(expected_dir, relative_path, actual_content),
                )?,
                (None, None, Some(actual_content)) => Self::write_expected_or_fail(
                    assertion_name.as_str(),
                    options,
                    format!("Unexpected output file '{}'", relative_path.display()),
                    || Self::write_expected_snapshot(expected_dir, relative_path, actual_content),
                )?,
                (None, None, None) => Ok(()),
            },
        )
    }

    fn remove_expected_or_fail<F>(
        assertion_name: &str,
        options: &TestOptions,
        error_message: String,
        update_snapshot: F,
    ) -> Result<Result<()>>
    where
        F: FnOnce() -> Result<()>,
    {
        Self::update_snapshot_or_fail(
            options,
            assertion_name,
            error_message,
            "remove snapshot",
            update_snapshot,
        )
    }

    fn write_expected_or_fail<F>(
        assertion_name: &str,
        options: &TestOptions,
        error_message: String,
        update_snapshot: F,
    ) -> Result<Result<()>>
    where
        F: FnOnce() -> Result<()>,
    {
        Self::update_snapshot_or_fail(
            options,
            assertion_name,
            error_message,
            "create snapshot",
            update_snapshot,
        )
    }

    fn update_snapshot_or_fail<F>(
        options: &TestOptions,
        assertion_name: &str,
        error_message: String,
        action: &str,
        update_snapshot: F,
    ) -> Result<Result<()>>
    where
        F: FnOnce() -> Result<()>,
    {
        if options.update_snapshots {
            update_snapshot()
                .with_context(|| format!("Failed to {action} for test '{assertion_name}'"))?;
            return Ok(Ok(()));
        }

        Ok(Err(anyhow::anyhow!(error_message)))
    }

    fn push_directory_assertion(
        assertion_results: &mut Vec<AssertionResult>,
        test_name: &str,
        relative_path: PathBuf,
        result: Result<()>,
        options: &TestOptions,
    ) -> bool {
        let failed = result.is_err();
        assertion_results.push(AssertionResult {
            name: Self::format_directory_assertion_name(test_name, &relative_path),
            result,
        });
        options.should_fail_fast() && failed
    }

    fn store_directory_entrypoint_result(
        execution: &mut DirectoryExecution,
        test_name: &str,
        relative_path: &Path,
        result: Result<()>,
        options: &TestOptions,
    ) -> bool {
        if options.should_fail_fast() && result.is_err() {
            execution.assertion_results.push(AssertionResult {
                name: Self::format_directory_assertion_name(test_name, relative_path),
                result,
            });
            return true;
        }

        execution
            .fixed_results
            .insert(relative_path.to_path_buf(), result);
        false
    }

    fn contents_match(
        expected: &str,
        actual: &str,
        options: &TestOptions,
        input_path: Option<&Path>,
    ) -> bool {
        // Warn if ignore_whitespace is used with non-strict mode (it only applies to strict)
        if options.ignore_whitespace && options.strictness != Strictness::Strict {
            eprintln!(
                "Warning: --ignore-whitespace only applies to strict mode. \
                 It has no effect with {} mode, which already handles whitespace differences.",
                options.strictness
            );
        }

        // For non-strict modes, we need language detection
        let language = options
            .language
            .as_deref()
            .or_else(|| input_path.and_then(detect_language));

        match (options.strictness, language) {
            (Strictness::Strict, _) => {
                if options.ignore_whitespace {
                    let normalize = |s: &str| {
                        s.lines()
                            .map(|line| line.trim())
                            .filter(|line| !line.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    normalize(expected) == normalize(actual)
                } else {
                    expected == actual
                }
            }
            (Strictness::Cst, Some(lang)) => cst_compare(expected, actual, lang),
            (Strictness::Ast, Some(lang)) => ast_compare(expected, actual, lang),
            (Strictness::Loose, Some(lang)) => loose_compare(expected, actual, lang),
            (strictness, None) => {
                eprintln!(
                    "Warning: Language could not be detected for {} comparison mode. \
                     Tree-based comparison (loose/ast/cst) requires a language to select the parser. \
                     Falling back to strict (exact string) comparison. \
                     Use --language to specify the language explicitly.",
                    strictness
                );
                expected == actual
            }
        }
    }

    fn generate_diff(expected: &str, actual: &str, options: &TestOptions) -> String {
        use similar::ChangeTag;
        use std::fmt::Write;

        let diff = TextDiff::from_lines(expected, actual);
        let mut result = String::new();

        let format_change = |result: &mut String, change: &similar::Change<&str>| {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            let _ = write!(result, "{sign}{change}");
        };

        let grouped_ops = diff.grouped_ops(options.context_lines);
        for group in &grouped_ops {
            for op in group {
                for change in diff.iter_changes(op) {
                    format_change(&mut result, &change);
                }
            }
        }

        if result.is_empty() {
            for change in diff.iter_all_changes() {
                format_change(&mut result, &change);
            }
        }

        result
    }

    async fn run_with_watch<'a>(
        &mut self,
        extensions: &[&str],
        execution_fn: ExecutionFn<'a>,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
    ) -> Result<TestSummary> {
        println!("Running in watch mode. Press Ctrl+C to exit.");
        let initial_summary = self
            .run_tests_once(extensions, execution_fn, capabilities)
            .await?;

        println!("Watch mode not fully implemented yet. Use --no-watch for now.");

        Ok(initial_summary)
    }

    fn should_expect_error_name(
        name: &str,
        should_error: bool,
        expect_error_patterns: &[String],
    ) -> bool {
        should_error
            || expect_error_patterns
                .iter()
                .any(|pattern| name.contains(pattern))
    }

    fn format_directory_assertion_name(test_name: &str, relative_path: &Path) -> String {
        format!("{}_{}", test_name, relative_path.display())
    }

    fn copy_directory_contents(source: &Path, destination: &Path) -> Result<()> {
        for entry in WalkDir::new(source) {
            let entry = entry?;
            let path = entry.path();
            let relative_path = path
                .strip_prefix(source)
                .with_context(|| format!("Failed to strip prefix '{}'", source.display()))?;
            if relative_path.as_os_str().is_empty() {
                continue;
            }

            let destination_path = destination.join(relative_path);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&destination_path)?;
            } else {
                if let Some(parent) = destination_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(path, &destination_path).with_context(|| {
                    format!(
                        "Failed to copy '{}' to '{}'",
                        path.display(),
                        destination_path.display()
                    )
                })?;
            }
        }

        Ok(())
    }

    fn collect_workspace_files(root: &Path) -> Result<HashMap<PathBuf, String>> {
        let mut files = HashMap::new();
        for entry in WalkDir::new(root) {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let relative_path = path
                .strip_prefix(root)
                .with_context(|| format!("Failed to strip prefix '{}'", root.display()))?
                .to_path_buf();
            if Self::should_ignore_snapshot_path(&relative_path) {
                continue;
            }

            if let Ok(file) = crate::fixtures::TestFile::from_path_with_base(path, root) {
                files.insert(relative_path, file.content);
            }
        }

        Ok(files)
    }

    fn tracked_paths(files: &HashMap<PathBuf, crate::fixtures::TestFile>) -> BTreeSet<PathBuf> {
        files
            .keys()
            .filter(|path| !Self::should_ignore_snapshot_path(path))
            .cloned()
            .collect()
    }

    fn should_ignore_snapshot_path(relative_path: &Path) -> bool {
        relative_path == Path::new("metrics.json")
    }

    fn apply_transformation_output(path: &Path, output: TransformOutput) -> Result<()> {
        let write_path = output.rename_to.as_deref().unwrap_or(path);
        if let Some(parent) = write_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(write_path, output.content)?;

        if output.rename_to.is_some() && write_path != path && path.exists() {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    fn assert_expected_content(
        expected_file: &crate::fixtures::TestFile,
        actual_content: &str,
        input_path: Option<&Path>,
        options: &TestOptions,
        assertion_name: &str,
    ) -> Result<()> {
        if Self::contents_match(&expected_file.content, actual_content, options, input_path) {
            return Ok(());
        }

        if options.update_snapshots {
            if let Some(parent) = expected_file.path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&expected_file.path, actual_content)?;
            return Ok(());
        }

        let diff = Self::generate_diff(&expected_file.content, actual_content, options);
        Err(anyhow::anyhow!(
            "Output mismatch for test '{}':\n{}",
            assertion_name,
            diff
        ))
    }

    fn write_expected_snapshot(
        expected_root: &Path,
        relative_path: &Path,
        content: &str,
    ) -> Result<()> {
        let expected_path = expected_root.join(relative_path);
        if let Some(parent) = expected_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(expected_path, content)?;
        Ok(())
    }

    fn remove_expected_snapshot(expected_path: &Path, expected_root: &Path) -> Result<()> {
        if expected_path.exists() {
            fs::remove_file(expected_path)?;
        }

        let mut current = expected_path.parent();
        while let Some(dir) = current {
            if dir == expected_root {
                break;
            }

            if fs::read_dir(dir)?.next().is_none() {
                fs::remove_dir(dir)?;
                current = dir.parent();
            } else {
                break;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionFn, ExecutionRequest, TestRunner, TransformOutput, TransformationResult};
    use crate::{
        config::{ReporterType, Strictness, TestOptions},
        fixtures::TestSource,
    };
    use anyhow::Result;
    use std::fs;
    use std::future::Future;
    use std::path::Path;
    use std::pin::Pin;
    use std::time::Duration;
    use tempfile::TempDir;

    fn test_options(update_snapshots: bool) -> TestOptions {
        TestOptions {
            filter: None,
            update_snapshots,
            verbose: false,
            parallel: false,
            max_threads: Some(1),
            fail_fast: false,
            watch: false,
            reporter: ReporterType::Terse,
            timeout: Duration::from_secs(5),
            ignore_whitespace: false,
            context_lines: 3,
            expect_errors: vec![],
            strictness: Strictness::Strict,
            language: Some("javascript".to_string()),
            expected_extension: None,
        }
    }

    fn write_fixture_file(path: impl AsRef<Path>, content: &str) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, content).expect("write fixture file");
    }

    fn read_fixture_file(path: impl AsRef<Path>) -> String {
        fs::read_to_string(path).expect("read fixture file")
    }

    fn boxed_execution_fn<F, Fut>(func: F) -> ExecutionFn<'static>
    where
        F: Fn(ExecutionRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<TransformationResult>> + 'static,
    {
        Box::new(move |request, _capabilities| {
            Box::pin(func(request)) as Pin<Box<dyn Future<Output = Result<TransformationResult>>>>
        })
    }

    #[tokio::test]
    async fn directory_fixture_reports_created_and_deleted_files() {
        let temp = TempDir::new().expect("temp dir");
        let tests_dir = temp.path().join("tests");
        let fixture_dir = tests_dir.join("multi");
        write_fixture_file(fixture_dir.join("input/main.js"), "before-main");
        write_fixture_file(fixture_dir.join("input/old.js"), "old-file");
        write_fixture_file(fixture_dir.join("expected/main.js"), "after-main");
        write_fixture_file(fixture_dir.join("expected/new.js"), "created-file");

        let execution_fn = boxed_execution_fn(|request| async move {
            let file_name = request
                .input_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            if file_name == "main.js" {
                let root = request.input_path.parent().expect("parent").to_path_buf();
                fs::remove_file(root.join("old.js")).expect("remove old file");
                fs::write(root.join("new.js"), "created-file").expect("write new file");
                Ok(TransformationResult::Success(TransformOutput {
                    content: "after-main".to_string(),
                    rename_to: None,
                }))
            } else {
                Ok(TransformationResult::Success(TransformOutput {
                    content: request.input_code,
                    rename_to: None,
                }))
            }
        });

        let mut runner = TestRunner::new(test_options(false), TestSource::Directory(tests_dir));
        let summary = runner
            .run_tests(&[".js"], execution_fn, None)
            .await
            .expect("run tests");

        assert!(summary.is_success());
        assert_eq!(summary.total, 3);
        assert!(summary
            .details
            .iter()
            .any(|detail| detail.name == "multi_main.js" && detail.passed));
        assert!(summary
            .details
            .iter()
            .any(|detail| detail.name == "multi_new.js" && detail.passed));
        assert!(summary
            .details
            .iter()
            .any(|detail| detail.name == "multi_old.js" && detail.passed));
    }

    #[tokio::test]
    async fn directory_fixture_reports_unexpected_extra_files() {
        let temp = TempDir::new().expect("temp dir");
        let tests_dir = temp.path().join("tests");
        let fixture_dir = tests_dir.join("unexpected");
        write_fixture_file(fixture_dir.join("input/main.js"), "before-main");
        write_fixture_file(fixture_dir.join("expected/main.js"), "before-main");

        let execution_fn = boxed_execution_fn(|request| async move {
            let root = request.input_path.parent().expect("parent");
            fs::write(root.join("extra.js"), "extra").expect("write extra file");
            Ok(TransformationResult::Success(TransformOutput {
                content: request.input_code,
                rename_to: None,
            }))
        });

        let mut runner = TestRunner::new(test_options(false), TestSource::Directory(tests_dir));
        let summary = runner
            .run_tests(&[".js"], execution_fn, None)
            .await
            .expect("run tests");

        assert!(!summary.is_success());
        assert!(summary
            .details
            .iter()
            .any(|detail| detail.name == "unexpected_extra.js"
                && detail
                    .error_message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Unexpected output file")));
    }

    #[tokio::test]
    async fn directory_fixture_update_snapshots_syncs_expected_tree() {
        let temp = TempDir::new().expect("temp dir");
        let tests_dir = temp.path().join("tests");
        let fixture_dir = tests_dir.join("update");
        write_fixture_file(fixture_dir.join("input/main.js"), "before-main");
        write_fixture_file(fixture_dir.join("input/legacy.js"), "legacy-file");
        write_fixture_file(fixture_dir.join("expected/main.js"), "stale-main");
        write_fixture_file(fixture_dir.join("expected/legacy.js"), "legacy-file");

        let execution_fn = boxed_execution_fn(|request| async move {
            let file_name = request
                .input_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            if file_name == "main.js" {
                let root = request.input_path.parent().expect("parent").to_path_buf();
                fs::remove_file(root.join("legacy.js")).expect("remove legacy file");
                fs::write(root.join("new.js"), "created-file").expect("write new file");
                Ok(TransformationResult::Success(TransformOutput {
                    content: "after-main".to_string(),
                    rename_to: None,
                }))
            } else {
                Ok(TransformationResult::Success(TransformOutput {
                    content: request.input_code,
                    rename_to: None,
                }))
            }
        });

        let mut runner =
            TestRunner::new(test_options(true), TestSource::Directory(tests_dir.clone()));
        let summary = runner
            .run_tests(&[".js"], execution_fn, None)
            .await
            .expect("run tests");

        assert!(summary.is_success());
        assert_eq!(
            read_fixture_file(fixture_dir.join("expected/main.js")),
            "after-main"
        );
        assert_eq!(
            read_fixture_file(fixture_dir.join("expected/new.js")),
            "created-file"
        );
        assert!(!fixture_dir.join("expected/legacy.js").exists());
    }

    #[tokio::test]
    async fn directory_fixture_ignores_non_utf8_workspace_files() {
        let temp = TempDir::new().expect("temp dir");
        let tests_dir = temp.path().join("tests");
        let fixture_dir = tests_dir.join("non-utf8");
        write_fixture_file(fixture_dir.join("input/main.js"), "before-main");
        write_fixture_file(fixture_dir.join("expected/main.js"), "after-main");

        let execution_fn = boxed_execution_fn(|request| async move {
            let root = request.input_path.parent().expect("parent");
            fs::write(root.join("binary.bin"), [0xff, 0xfe, 0xfd]).expect("write binary file");
            Ok(TransformationResult::Success(TransformOutput {
                content: "after-main".to_string(),
                rename_to: None,
            }))
        });

        let mut runner = TestRunner::new(test_options(false), TestSource::Directory(tests_dir));
        let summary = runner
            .run_tests(&[".js"], execution_fn, None)
            .await
            .expect("run tests");

        assert!(summary.is_success());
        assert_eq!(summary.total, 1);
        assert_eq!(summary.details[0].name, "non-utf8_main.js");
    }

    #[tokio::test]
    async fn single_file_fixture_behavior_is_unchanged() {
        let temp = TempDir::new().expect("temp dir");
        let tests_dir = temp.path().join("tests");
        let fixture_dir = tests_dir.join("single");
        write_fixture_file(fixture_dir.join("input.js"), "before");
        write_fixture_file(fixture_dir.join("expected.js"), "after");

        let execution_fn = boxed_execution_fn(|request| async move {
            Ok(TransformationResult::Success(TransformOutput {
                content: request.input_code.replace("before", "after"),
                rename_to: None,
            }))
        });

        let mut runner = TestRunner::new(test_options(false), TestSource::Directory(tests_dir));
        let summary = runner
            .run_tests(&[".js"], execution_fn, None)
            .await
            .expect("run tests");

        assert!(summary.is_success());
        assert_eq!(summary.total, 1);
        assert_eq!(summary.details[0].name, "single");
    }

    #[tokio::test]
    async fn filter_skips_unmatched_directory_fixtures() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let temp = TempDir::new().expect("temp dir");
        let tests_dir = temp.path().join("tests");

        write_fixture_file(tests_dir.join("matched/input/keep.js"), "keep");
        write_fixture_file(tests_dir.join("matched/expected/keep.js"), "keep");
        write_fixture_file(tests_dir.join("skipped/input/skip.js"), "skip");
        write_fixture_file(tests_dir.join("skipped/expected/skip.js"), "skip");

        let skipped_executions = Arc::new(AtomicUsize::new(0));
        let skipped_executions_clone = skipped_executions.clone();
        let execution_fn = boxed_execution_fn(move |request| {
            let skipped_executions = skipped_executions_clone.clone();
            async move {
                if request
                    .logical_input_path
                    .as_deref()
                    .unwrap_or(request.input_path.as_path())
                    .ends_with("skip.js")
                {
                    skipped_executions.fetch_add(1, Ordering::SeqCst);
                }

                Ok(TransformationResult::Success(TransformOutput {
                    content: request.input_code,
                    rename_to: None,
                }))
            }
        });

        let mut options = test_options(false);
        options.filter = Some("matched_keep.js".to_string());
        let mut runner = TestRunner::new(options, TestSource::Directory(tests_dir));
        let summary = runner
            .run_tests(&[".js"], execution_fn, None)
            .await
            .expect("run tests");

        assert!(summary.is_success());
        assert_eq!(summary.total, 1);
        assert_eq!(summary.details[0].name, "matched_keep.js");
        assert_eq!(skipped_executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fail_fast_stops_directory_fixture_after_first_failure() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let temp = TempDir::new().expect("temp dir");
        let tests_dir = temp.path().join("tests");
        let fixture_dir = tests_dir.join("fail-fast");
        write_fixture_file(fixture_dir.join("input/a.js"), "a");
        write_fixture_file(fixture_dir.join("input/b.js"), "b");
        write_fixture_file(fixture_dir.join("expected/a.js"), "a");
        write_fixture_file(fixture_dir.join("expected/b.js"), "b");

        let later_executions = Arc::new(AtomicUsize::new(0));
        let later_executions_clone = later_executions.clone();
        let execution_fn = boxed_execution_fn(move |request| {
            let later_executions = later_executions_clone.clone();
            async move {
                let file_name = request
                    .input_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if file_name == "a.js" {
                    Ok(TransformationResult::Error("boom".to_string()))
                } else {
                    later_executions.fetch_add(1, Ordering::SeqCst);
                    Ok(TransformationResult::Success(TransformOutput {
                        content: request.input_code,
                        rename_to: None,
                    }))
                }
            }
        });

        let mut options = test_options(false);
        options.fail_fast = true;
        let mut runner = TestRunner::new(options, TestSource::Directory(tests_dir));
        let summary = runner
            .run_tests(&[".js"], execution_fn, None)
            .await
            .expect("run tests");

        assert!(!summary.is_success());
        assert_eq!(summary.total, 1);
        assert_eq!(summary.details[0].name, "fail-fast_a.js");
        assert_eq!(later_executions.load(Ordering::SeqCst), 0);
    }
}
