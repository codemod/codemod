use codemod_sandbox::sandbox::engine::{ExecutionResult, JssgExecutionOptions};
use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool, ErrorData as McpError};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ast_grep_language::SupportLang;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::{
    sandbox::{
        engine::{execute_codemod_with_quickjs, language_data::get_extensions_for_language},
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use testing_utils::{
    ReporterType, TestOptions, TestRunner, TestSource, TransformationResult, TransformationTestCase,
};

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum TestCase {
    #[serde(rename = "adhoc")]
    Adhoc {
        input_code: String,
        expected_output_code: String,
    },
    #[serde(rename = "file-system")]
    FileSystem {
        input_file: String,
        expected_output_file: String,
    },
}

impl From<TestCase> for TransformationTestCase {
    fn from(test_case: TestCase) -> Self {
        match test_case {
            TestCase::Adhoc {
                input_code,
                expected_output_code,
            } => TransformationTestCase {
                name: "adhoc_test".to_string(),
                input_code,
                expected_output_code,
            },
            TestCase::FileSystem {
                input_file,
                expected_output_file,
            } => {
                // For file system test cases, we'll read the files
                let input_code = std::fs::read_to_string(&input_file).unwrap_or_default();
                let expected_output_code =
                    std::fs::read_to_string(&expected_output_file).unwrap_or_default();

                TransformationTestCase {
                    name: format!(
                        "fs_test_{}",
                        Path::new(&input_file)
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                    ),
                    input_code,
                    expected_output_code,
                }
            }
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RunJssgTestRequest {
    /// The programming language for the codemod
    pub language: String,
    /// Path to the JSSG codemod file
    pub codemod_file: String,
    /// Test cases to run
    pub tests: Vec<TestCase>,
    /// Timeout for each test in seconds (default: 30)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct TestResult {
    pub success: bool,
    pub message: String,
    pub test_index: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(tag = "success")]
pub enum RunJssgTestResponse {
    #[serde(rename = "true")]
    Success { message: String },
    #[serde(rename = "false")]
    Failure {
        message: String,
        test_results: Vec<TestResult>,
    },
}

#[derive(Clone)]
pub struct JssgTestHandler;

impl JssgTestHandler {
    pub fn new() -> Self {
        Self
    }

    #[tool(
        description = "Run tests for a JSSG (JavaScript AST-grep) codemod with given test cases"
    )]
    pub async fn run_jssg_tests(
        &self,
        Parameters(request): Parameters<RunJssgTestRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Spawn a blocking task to handle the QuickJS execution
        let response = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current()
                .block_on(async move { Self::execute_tests_blocking(request).await })
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))?
        .map_err(|e| McpError::internal_error(format!("Failed to run JSSG tests: {e}"), None))?;

        let content = match serde_json::to_string_pretty(&response) {
            Ok(json) => json,
            Err(e) => {
                return Err(McpError::internal_error(
                    format!("Failed to serialize response: {e}"),
                    None,
                ));
            }
        };

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    async fn execute_tests_blocking(
        request: RunJssgTestRequest,
    ) -> Result<RunJssgTestResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Parse language
        let language: SupportLang = request
            .language
            .parse()
            .map_err(|_| format!("Unsupported language: {}", request.language))?;

        // Set up execution components
        let codemod_path = PathBuf::from(&request.codemod_file);

        let script_base_dir = codemod_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let tsconfig_path = find_tsconfig(&script_base_dir);
        let resolver = Arc::new(OxcResolver::new(script_base_dir, tsconfig_path)?);

        // Check if codemod file exists
        if !codemod_path.exists() {
            return Ok(RunJssgTestResponse::Failure {
                message: format!("Codemod file not found: {}", request.codemod_file),
                test_results: vec![],
            });
        }

        // Convert test cases to TransformationTestCase
        let transformation_test_cases: Vec<TransformationTestCase> = request
            .tests
            .into_iter()
            .enumerate()
            .map(|(index, test_case)| {
                let mut transformed = TransformationTestCase::from(test_case);
                transformed.name = format!("test_{index}");
                transformed
            })
            .collect();

        // Create test options
        let test_options = TestOptions {
            filter: None,
            update_snapshots: false,
            verbose: false,
            parallel: false,
            max_threads: Some(1),
            fail_fast: false,
            watch: false,
            reporter: ReporterType::Console,
            timeout: Duration::from_secs(request.timeout_seconds),
            ignore_whitespace: false,
            context_lines: 3,
            expect_errors: vec![],
        };

        // Create execution function
        let execution_fn = Box::new(
            move |input_code: &str,
                  input_path: &Path,
                  capabilities: Option<HashSet<LlrtSupportedModules>>| {
                let codemod_path = codemod_path.clone();
                let resolver = resolver.clone();
                let input_code = input_code.to_string();
                let input_path = input_path.to_path_buf();

                Box::pin(async move {
                    let options = JssgExecutionOptions {
                        script_path: &codemod_path,
                        resolver,
                        language,
                        file_path: &input_path,
                        content: &input_code,
                        selector_config: None,
                        params: None,
                        matrix_values: None,
                        capabilities: capabilities.clone(),
                        semantic_provider: None,
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

        // Create test source from cases
        let test_source = TestSource::Cases(transformation_test_cases);

        // Get file extensions for the language
        let extensions = get_extensions_for_language(language);

        // Create and run test runner in a blocking task
        let summary = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(async move {
                let mut runner = TestRunner::new(test_options, test_source);
                runner.run_tests(&extensions, execution_fn, None).await
            })
        })
        .await
        .map_err(|e| format!("Task join error: {e}"))?
        .map_err(|e| format!("Test execution error: {e}"))?;

        // Convert summary to response
        if summary.is_success() {
            Ok(RunJssgTestResponse::Success {
                message: format!("All {} tests passed! 🎉", summary.total),
            })
        } else {
            // Create test results from summary (simplified for now)
            let test_results = (0..summary.total)
                .map(|index| TestResult {
                    success: index < summary.passed,
                    message: if index < summary.passed {
                        format!("Test {index} passed")
                    } else {
                        format!("Test {index} failed")
                    },
                    test_index: index,
                })
                .collect();

            Ok(RunJssgTestResponse::Failure {
                message: format!(
                    "{} of {} tests failed. {} tests passed.",
                    summary.failed, summary.total, summary.passed
                ),
                test_results,
            })
        }
    }
}

impl Default for JssgTestHandler {
    fn default() -> Self {
        Self::new()
    }
}
