use anyhow::Result;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::tree_sitter::load_tree_sitter;
use libtest_mimic::{run, Trial};
use similar::TextDiff;
use std::pin::Pin;
use std::{collections::HashSet, future::Future};
use tokio::time::timeout;

use crate::{
    config::TestOptions,
    fixtures::{TestSource, UnifiedTestCase},
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

#[derive(Debug, Clone)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub ignored: usize,
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
    pub async fn new(options: TestOptions, test_source: TestSource) -> Self {
        let language = options.language.unwrap();
        let _ = load_tree_sitter(
            &[language],
            options
                .download_progress_callback
                .as_ref()
                .map(|c| c.callback.clone()),
        )
        .await
        .map_err(|e| {
            Box::new(std::io::Error::other(format!(
                "Failed to load tree-sitter language: {e:?}"
            )))
        });

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

            let final_result = match result {
                Ok(test_result) => test_result,
                Err(_) => Err(anyhow::anyhow!(
                    "Test '{}' timed out after {:?}",
                    test_case.name,
                    self.options.timeout
                )),
            };

            test_results.push((test_case.name.clone(), final_result));

            if self.options.should_fail_fast() && test_results.last().unwrap().1.is_err() {
                println!("Stopping test execution due to --fail-fast and test failure");
                break;
            }
        }

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

        Ok(TestSummary::from_libtest_result(result))
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
                _ => {
                    return Err(anyhow::anyhow!(
                        "Test '{}' was expected to fail but succeeded",
                        test_case.name
                    ));
                }
            }
        }

        if let TransformationResult::Error(error) = execution_result {
            return Err(anyhow::anyhow!(
                "Transformation execution failed:\n{}",
                error
            ));
        }

        let actual_content = match execution_result {
            TransformationResult::Success(content) => Ok(content),
            TransformationResult::Error(error) => Err(anyhow::anyhow!(
                "Transformation execution failed:\n{}",
                error
            )),
        }?;

        if !Self::contents_match(&test_case.expected_output_code, &actual_content, options) {
            if options.update_snapshots {
                match test_case.update_expected_output(&actual_content) {
                    Ok(()) => {
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!(
                            "Failed to update snapshot for test '{}': {}",
                            test_case.name,
                            e
                        ));
                    }
                }
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

    fn contents_match(expected: &str, actual: &str, options: &TestOptions) -> bool {
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

    fn generate_diff(expected: &str, actual: &str, options: &TestOptions) -> String {
        let diff = TextDiff::from_lines(expected, actual);

        let mut result = String::new();
        let grouped_ops = diff.grouped_ops(options.context_lines);

        for group in &grouped_ops {
            for op in group {
                for change in diff.iter_changes(op) {
                    let sign = match change.tag() {
                        similar::ChangeTag::Delete => "-",
                        similar::ChangeTag::Insert => "+",
                        similar::ChangeTag::Equal => " ",
                    };
                    result.push_str(&format!("{sign}{change}"));
                }
            }
        }

        if result.is_empty() {
            for change in diff.iter_all_changes() {
                let sign = match change.tag() {
                    similar::ChangeTag::Delete => "-",
                    similar::ChangeTag::Insert => "+",
                    similar::ChangeTag::Equal => " ",
                };
                result.push_str(&format!("{sign}{change}"));
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
