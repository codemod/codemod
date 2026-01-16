use anyhow::{Context, Result};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use libtest_mimic::{run, Trial};
use similar::TextDiff;
use std::pin::Pin;
use std::{collections::HashSet, future::Future};
use tokio::time::timeout;

use crate::{
    config::{Strictness, TestOptions},
    fixtures::{TestSource, UnifiedTestCase},
    strictness::{ast_compare, cst_compare, detect_language, loose_compare},
};

/// Result of executing a transformation on input code
#[derive(Debug, Clone)]
pub enum TransformationResult {
    Success(String),
    Error(String),
}

/// Execution function type - takes input code and file path, returns transformation result
pub type ExecutionFn<'a> = Box<
    dyn Fn(
            &str,
            &std::path::Path,
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
        let test_cases = self
            .test_source
            .to_unified_test_cases(extensions)
            .map_err(|e| anyhow::anyhow!("Failed to load test cases: {}", e))?;

        if test_cases.is_empty() {
            return Err(anyhow::anyhow!("No test cases found"));
        }

        let filtered_test_cases: Vec<&UnifiedTestCase> = if let Some(filter) = &self.options.filter
        {
            test_cases
                .iter()
                .filter(|test_case| test_case.name.contains(filter))
                .collect()
        } else {
            test_cases.iter().collect()
        };

        if filtered_test_cases.is_empty() {
            return Err(anyhow::anyhow!(
                "No test cases match the filter '{}'",
                self.options.filter.as_ref().unwrap()
            ));
        }

        let mut test_results = Vec::new();
        for test_case in filtered_test_cases {
            let result = timeout(
                self.options.timeout,
                Self::execute_test_case(
                    test_case,
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

            let failed = final_result.is_err();
            test_results.push((test_case.name.clone(), final_result));

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
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("test_input"));
        let execution_result =
            execution_fn(&test_case.input_code, input_path, capabilities.clone()).await?;

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
            TransformationResult::Success(content) => content,
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
            test_case.input_path.as_deref(),
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

    fn contents_match(
        expected: &str,
        actual: &str,
        options: &TestOptions,
        input_path: Option<&std::path::Path>,
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
}
