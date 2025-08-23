use anyhow::Result;
use libtest_mimic::{run, Trial};
use similar::TextDiff;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::time::timeout;

use codemod_ast_grep_dynamic_lang::DynamicLang;
use codemod_sandbox::tree_sitter::SupportedLanguage;
use codemod_sandbox::utils::project_discovery::find_tsconfig;

use crate::commands::jssg::testing::{
    config::TestOptions,
    fixtures::{TestCase, TestError, TestFile},
};
use codemod_sandbox::sandbox::{
    engine::execute_codemod_with_quickjs, errors::ExecutionError, filesystem::RealFileSystem,
    resolvers::OxcResolver,
};
use std::str::FromStr;
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
            errors: 0, // libtest-mimic doesn't distinguish between failed and errors
            ignored,
        }
    }

    /// Check if all tests passed
    pub fn is_success(&self) -> bool {
        self.failed == 0 && self.errors == 0
    }

    /// Display a summary of the test results
    pub fn display_summary(&self) {
        println!(
            "Test result: {}. {} passed; {} failed; {} ignored; {} total",
            if self.is_success() { "ok" } else { "FAILED" },
            self.passed,
            self.failed,
            self.ignored,
            self.total
        );
    }
}

pub struct TestRunner {
    options: TestOptions,
    test_directory: PathBuf,
}

impl TestRunner {
    pub fn new(options: TestOptions, test_directory: PathBuf) -> Self {
        Self {
            options,
            test_directory,
        }
    }

    /// Format execution errors in a more readable way
    fn format_execution_error(error: &ExecutionError) -> String {
        match error {
            ExecutionError::Runtime { source } => {
                match source {
                    codemod_sandbox::sandbox::errors::RuntimeError::InitializationFailed {
                        message,
                    } => {
                        // Check if this looks like a JavaScript error with stack trace
                        if message.contains("\\n") {
                            // Unescape newlines and format nicely
                            let formatted = message.replace("\\n", "\n");
                            format!("Runtime Error:\n{formatted}")
                        } else {
                            format!("Runtime Error: {message}")
                        }
                    }
                    codemod_sandbox::sandbox::errors::RuntimeError::ExecutionFailed { message } => {
                        if message.contains("\\n") {
                            let formatted = message.replace("\\n", "\n");
                            format!("Execution Error:\n{formatted}")
                        } else {
                            format!("Execution Error: {message}")
                        }
                    }
                    other => format!("Runtime Error: {other}"),
                }
            }
            ExecutionError::Configuration { message } => {
                format!("Configuration Error: {message}")
            }
            ExecutionError::FileSystem { source } => {
                format!("File System Error: {source}")
            }
            ExecutionError::ThreadExecution { message } => {
                format!("Thread Execution Error: {message}")
            }
            ExecutionError::NoFilesFound => "No files found to process".to_string(),
        }
    }

    pub async fn run_tests(&mut self, codemod_path: &Path, language: &str) -> Result<TestSummary> {
        if self.options.watch {
            return self.run_with_watch(codemod_path, language).await;
        }

        self.run_tests_once(codemod_path, language).await
    }

    async fn run_tests_once(&mut self, codemod_path: &Path, language: &str) -> Result<TestSummary> {
        // Parse language
        let language_enum: DynamicLang = language.parse().unwrap();

        // Discover test cases
        let test_cases = TestCase::discover_in_directory(&self.test_directory, language_enum)
            .map_err(|e| anyhow::anyhow!("Failed to discover test cases: {}", e))?;

        if test_cases.is_empty() {
            return Err(anyhow::anyhow!(
                "No test cases found in {}",
                self.test_directory.display()
            ));
        }

        // Filter test cases before execution if filter is specified
        let filtered_test_cases: Vec<&TestCase> = if let Some(filter) = &self.options.filter {
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

        // Set up execution components following the pattern from run.rs
        let filesystem = Arc::new(RealFileSystem::new());
        let script_base_dir = codemod_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let tsconfig_path = find_tsconfig(&script_base_dir);

        let resolver = Arc::new(OxcResolver::new(script_base_dir.clone(), tsconfig_path)?);

        // Execute only the filtered tests
        let mut test_results = Vec::new();
        for test_case in filtered_test_cases {
            let result = timeout(
                self.options.timeout,
                Self::execute_test_case(
                    codemod_path,
                    language_enum,
                    &filesystem,
                    &resolver,
                    test_case,
                    &self.options,
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

            // Implement fail_fast: stop on first failure
            if self.options.should_fail_fast() && test_results.last().unwrap().1.is_err() {
                println!("Stopping test execution due to --fail-fast and test failure");
                break;
            }
        }

        // Convert results to libtest-mimic trials
        let trials: Vec<Trial> = test_results
            .into_iter()
            .map(|(name, result)| {
                Trial::test(name, move || {
                    result.map_err(|e| libtest_mimic::Failed::from(format!("{e}")))
                })
            })
            .collect();

        // Convert our options to libtest-mimic arguments (without filter since we already filtered)
        let mut args = self.options.to_libtest_args();
        // Clear the filter since we already applied it
        args.filter = None;

        // Run tests using libtest-mimic
        let result = run(&args, trials);

        // Convert libtest-mimic result to our TestSummary
        Ok(TestSummary::from_libtest_result(result))
    }

    async fn execute_test_case(
        codemod_path: &Path,
        language: DynamicLang,
        filesystem: &Arc<RealFileSystem>,
        resolver: &Arc<OxcResolver>,
        test_case: &TestCase,
        options: &TestOptions,
    ) -> Result<()> {
        let should_expect_error = test_case.should_expect_error(&options.expect_errors);

        // Check for missing expected files
        if let Err(TestError::NoExpectedFile {
            test_dir,
            input_file,
        }) = test_case.validate_expected_files()
        {
            if options.update_snapshots {
                // Create expected files by running the codemod
                return Self::create_expected_files(
                    codemod_path,
                    language,
                    filesystem,
                    resolver,
                    test_case,
                )
                .await;
            } else {
                return Err(anyhow::anyhow!(
                    "No expected file found for {} in {}. Run with --update-snapshots to create it.",
                    input_file.display(),
                    test_dir.display()
                ));
            }
        }

        // Execute codemod on each input file and compare with expected output
        for (input_file, expected_file) in test_case
            .input_files
            .iter()
            .zip(test_case.expected_files.iter())
        {
            // Execute the codemod using the new pattern
            let execution_output = execute_codemod_with_quickjs(
                codemod_path,
                filesystem.clone(),
                resolver.clone(),
                SupportedLanguage::from_str(language.name()).unwrap(),
                &input_file.path,
                &input_file.content,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", Self::format_execution_error(&e)))?;

            // Handle expected errors
            if should_expect_error {
                if execution_output.error.is_none() {
                    return Err(anyhow::anyhow!(
                        "Test '{}' was expected to fail but succeeded",
                        test_case.name
                    ));
                } else {
                    // Test failed as expected, this is success
                    println!("Test '{}' failed as expected", test_case.name);
                    return Ok(());
                }
            }

            if let Some(error) = &execution_output.error {
                return Err(anyhow::anyhow!("Codemod execution failed:\n{}", error));
            }

            let actual_content = execution_output
                .content
                .unwrap_or_else(|| input_file.content.clone());

            if !Self::contents_match(&expected_file.content, &actual_content, options) {
                if options.update_snapshots {
                    // Update the expected file
                    let updated_file = TestFile::from_content(
                        expected_file.relative_path.clone(),
                        actual_content,
                        &test_case.path,
                    );
                    updated_file
                        .write_to_disk()
                        .map_err(|e| anyhow::anyhow!("Failed to update snapshot: {}", e))?;
                    println!("Updated snapshot for {}", test_case.name);
                } else {
                    let diff =
                        Self::generate_diff(&expected_file.content, &actual_content, options);
                    return Err(anyhow::anyhow!(
                        "Output mismatch for file {}:\n{}",
                        expected_file.relative_path.display(),
                        diff
                    ));
                }
            }
        }

        Ok(())
    }

    async fn create_expected_files(
        codemod_path: &Path,
        language: DynamicLang,
        filesystem: &Arc<RealFileSystem>,
        resolver: &Arc<OxcResolver>,
        test_case: &TestCase,
    ) -> Result<()> {
        for input_file in &test_case.input_files {
            let execution_output = execute_codemod_with_quickjs(
                codemod_path,
                filesystem.clone(),
                resolver.clone(),
                SupportedLanguage::from_str(language.name()).unwrap(),
                &input_file.path,
                &input_file.content,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", Self::format_execution_error(&e)))?;

            if let Some(error) = &execution_output.error {
                return Err(anyhow::anyhow!("Codemod execution failed:\n{}", error));
            }

            let output_content = execution_output
                .content
                .unwrap_or_else(|| input_file.content.clone());

            // Create expected file name by replacing "input" with "expected"
            let input_name = input_file.relative_path.to_string_lossy();
            let expected_name = input_name.replace("input", "expected");
            let expected_path = PathBuf::from(expected_name);

            let expected_file =
                TestFile::from_content(expected_path, output_content, &test_case.path);

            expected_file
                .write_to_disk()
                .map_err(|e| anyhow::anyhow!("Failed to create expected file: {}", e))?;

            println!(
                "Created expected file for {}/{}",
                test_case.name,
                input_file.relative_path.display()
            );
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

        // Use the simpler grouped diff approach
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

        // If grouped ops is empty, fall back to simple diff
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

    async fn run_with_watch(&mut self, codemod_path: &Path, language: &str) -> Result<TestSummary> {
        println!("Running in watch mode. Press Ctrl+C to exit.");

        // Run tests initially
        let initial_summary = self.run_tests_once(codemod_path, language).await?;

        // For now, just return the initial summary
        // TODO: Implement file watching with notify crate
        println!("Watch mode not fully implemented yet. Use --no-watch for now.");

        Ok(initial_summary)
    }
}
