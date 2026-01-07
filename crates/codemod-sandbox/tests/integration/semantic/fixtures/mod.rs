//! Test fixtures and helpers for semantic analysis integration tests.
//!
//! This module provides:
//! - Helper functions for loading fixtures and setting up test workspaces
//! - The `jssg_test!` macro for declarative test definitions

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, ExecutionResult, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use language_core::SemanticProvider;
use language_javascript::OxcSemanticProvider;
use language_python::RuffSemanticProvider;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

/// Base path for test fixtures relative to the crate root
const FIXTURES_BASE: &str = "tests/integration/semantic/fixtures";

/// Load a codemod file from the codemods directory
pub fn load_codemod(name: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir)
        .join(FIXTURES_BASE)
        .join("codemods")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to load codemod {}: {}", name, e))
}

/// Load a fixture file from the specified fixture directory
pub fn load_fixture(fixture_dir: &str, filename: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir)
        .join(FIXTURES_BASE)
        .join(fixture_dir)
        .join(filename);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to load fixture {}/{}: {}", fixture_dir, filename, e))
}

/// Get the path to a fixture directory
pub fn get_fixture_dir_path(fixture_dir: &str) -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join(FIXTURES_BASE)
        .join(fixture_dir)
}

/// Setup a test workspace by copying fixtures to a temp directory
pub fn setup_test_workspace(fixture_dir: &str) -> (TempDir, HashMap<String, PathBuf>) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let fixture_path = get_fixture_dir_path(fixture_dir);

    let mut file_paths = HashMap::new();

    // Copy all files from fixture directory to temp directory
    if fixture_path.exists() {
        for entry in fs::read_dir(&fixture_path).expect("Failed to read fixture directory") {
            let entry = entry.expect("Failed to read directory entry");
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy().to_string();

            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let content = fs::read_to_string(entry.path()).unwrap_or_else(|e| {
                    panic!("Failed to read fixture file {:?}: {}", entry.path(), e)
                });
                let dest_path = temp_dir.path().join(&file_name);
                fs::write(&dest_path, &content).unwrap_or_else(|e| {
                    panic!("Failed to write fixture file {:?}: {}", dest_path, e)
                });
                file_paths.insert(file_name_str, dest_path);
            }
        }
    }

    (temp_dir, file_paths)
}

/// Provider scope type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderScope {
    /// File-scoped analysis (single file)
    File,
    /// Workspace-scoped analysis (cross-file)
    Workspace,
}

/// Create a semantic provider for JavaScript/TypeScript
pub fn create_js_provider(
    scope: ProviderScope,
    workspace_path: Option<&Path>,
) -> Arc<dyn SemanticProvider> {
    match scope {
        ProviderScope::File => Arc::new(OxcSemanticProvider::file_scope()),
        ProviderScope::Workspace => {
            let path = workspace_path.expect("Workspace path required for workspace scope");
            Arc::new(OxcSemanticProvider::workspace_scope(path.to_path_buf()))
        }
    }
}

/// Create a semantic provider for Python
pub fn create_python_provider(
    scope: ProviderScope,
    workspace_path: Option<&Path>,
) -> Arc<dyn SemanticProvider> {
    match scope {
        ProviderScope::File => Arc::new(RuffSemanticProvider::file_scope()),
        ProviderScope::Workspace => {
            let path = workspace_path.expect("Workspace path required for workspace scope");
            Arc::new(RuffSemanticProvider::workspace_scope(path.to_path_buf()))
        }
    }
}

/// Configuration for a test execution
pub struct TestConfig<'a> {
    pub codemod_name: &'a str,
    pub fixture_dir: &'a str,
    pub target_file: &'a str,
    pub language: SupportLang,
    pub scope: ProviderScope,
    pub preprocess_files: Vec<&'a str>,
    pub expected_file: Option<&'a str>,
    pub no_provider: bool,
}

impl<'a> TestConfig<'a> {
    pub fn new(
        codemod_name: &'a str,
        fixture_dir: &'a str,
        target_file: &'a str,
        language: SupportLang,
    ) -> Self {
        Self {
            codemod_name,
            fixture_dir,
            target_file,
            language,
            scope: ProviderScope::File,
            preprocess_files: Vec::new(),
            expected_file: None,
            no_provider: false,
        }
    }

    pub fn with_scope(mut self, scope: ProviderScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn with_preprocess(mut self, files: Vec<&'a str>) -> Self {
        self.preprocess_files = files;
        self
    }

    pub fn with_expected(mut self, expected: &'a str) -> Self {
        self.expected_file = Some(expected);
        self
    }

    pub fn without_provider(mut self) -> Self {
        self.no_provider = true;
        self
    }
}

/// Run a test with the given configuration
pub async fn run_test(config: TestConfig<'_>) -> Result<Option<String>, String> {
    let (temp_dir, file_paths) = setup_test_workspace(config.fixture_dir);

    // Write codemod to temp directory
    let codemod_content = load_codemod(config.codemod_name);
    let codemod_path = temp_dir.path().join("codemod.js");
    fs::write(&codemod_path, &codemod_content).expect("Failed to write codemod");

    // Get target file path and content
    let target_path = file_paths
        .get(config.target_file)
        .cloned()
        .unwrap_or_else(|| temp_dir.path().join(config.target_file));

    let content = if target_path.exists() {
        fs::read_to_string(&target_path).expect("Failed to read target file")
    } else {
        load_fixture(config.fixture_dir, config.target_file)
    };

    let resolver = Arc::new(OxcResolver::new(temp_dir.path().to_path_buf(), None).unwrap());

    // Create provider based on language and scope
    let provider: Option<Arc<dyn SemanticProvider>> = if config.no_provider {
        None
    } else {
        let provider = match config.language {
            SupportLang::JavaScript | SupportLang::TypeScript | SupportLang::Tsx => {
                create_js_provider(config.scope, Some(temp_dir.path()))
            }
            SupportLang::Python => create_python_provider(config.scope, Some(temp_dir.path())),
            _ => panic!("Unsupported language: {:?}", config.language),
        };

        // Preprocess files if needed
        for preprocess_file in &config.preprocess_files {
            let preprocess_path = file_paths
                .get(*preprocess_file)
                .cloned()
                .unwrap_or_else(|| temp_dir.path().join(preprocess_file));
            let preprocess_content = fs::read_to_string(&preprocess_path)
                .unwrap_or_else(|_| load_fixture(config.fixture_dir, preprocess_file));
            provider
                .notify_file_processed(&preprocess_path, &preprocess_content)
                .expect("Failed to preprocess file");
        }

        Some(provider)
    };

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver,
        language: config.language,
        file_path: &target_path,
        content: &content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: provider,
        console_log_collector: None,
    };

    let result = execute_codemod_with_quickjs(options).await;

    match result {
        Ok(ExecutionResult::Modified(new_content)) => {
            // If expected file is specified, verify the output
            if let Some(expected_file) = config.expected_file {
                let expected = load_fixture(config.fixture_dir, expected_file);
                if new_content.trim() != expected.trim() {
                    return Err(format!(
                        "Output mismatch.\nExpected:\n{}\n\nGot:\n{}",
                        expected, new_content
                    ));
                }
            }
            Ok(Some(new_content))
        }
        Ok(ExecutionResult::Unmodified) | Ok(ExecutionResult::Skipped) => Ok(None),
        Err(e) => Err(format!("Execution failed: {:?}", e)),
    }
}

/// Macro for declarative test definitions
#[macro_export]
macro_rules! jssg_test {
    // Basic test: single file, file-scope provider
    (
        name: $name:ident,
        language: $lang:expr,
        codemod: $codemod:literal,
        fixture_dir: $fixture_dir:literal,
        target: $target:literal
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{TestConfig, run_test};

            let config = TestConfig::new($codemod, $fixture_dir, $target, $lang);
            let result = run_test(config).await;
            assert!(result.is_ok(), "Test failed: {:?}", result.err());
        }
    };

    // With expected output
    (
        name: $name:ident,
        language: $lang:expr,
        codemod: $codemod:literal,
        fixture_dir: $fixture_dir:literal,
        target: $target:literal,
        expected: $expected:literal
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{TestConfig, run_test};

            let config = TestConfig::new($codemod, $fixture_dir, $target, $lang)
                .with_expected($expected);
            let result = run_test(config).await;
            assert!(result.is_ok(), "Test failed: {:?}", result.err());
        }
    };

    // Workspace scope
    (
        name: $name:ident,
        language: $lang:expr,
        codemod: $codemod:literal,
        fixture_dir: $fixture_dir:literal,
        target: $target:literal,
        scope: workspace
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{TestConfig, run_test, ProviderScope};

            let config = TestConfig::new($codemod, $fixture_dir, $target, $lang)
                .with_scope(ProviderScope::Workspace);
            let result = run_test(config).await;
            assert!(result.is_ok(), "Test failed: {:?}", result.err());
        }
    };

    // Workspace scope with preprocess
    (
        name: $name:ident,
        language: $lang:expr,
        codemod: $codemod:literal,
        fixture_dir: $fixture_dir:literal,
        target: $target:literal,
        preprocess: [$($preprocess:literal),* $(,)?],
        scope: workspace
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{TestConfig, run_test, ProviderScope};

            let config = TestConfig::new($codemod, $fixture_dir, $target, $lang)
                .with_scope(ProviderScope::Workspace)
                .with_preprocess(vec![$($preprocess),*]);
            let result = run_test(config).await;
            assert!(result.is_ok(), "Test failed: {:?}", result.err());
        }
    };

    // No provider (for common_tests)
    (
        name: $name:ident,
        language: $lang:expr,
        codemod: $codemod:literal,
        fixture_dir: $fixture_dir:literal,
        target: $target:literal,
        no_provider: true
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{TestConfig, run_test};

            let config = TestConfig::new($codemod, $fixture_dir, $target, $lang)
                .without_provider();
            let result = run_test(config).await;
            assert!(result.is_ok(), "Test failed: {:?}", result.err());
        }
    };

    // With preprocess (file scope)
    (
        name: $name:ident,
        language: $lang:expr,
        codemod: $codemod:literal,
        fixture_dir: $fixture_dir:literal,
        target: $target:literal,
        preprocess: [$($preprocess:literal),* $(,)?]
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{TestConfig, run_test};

            let config = TestConfig::new($codemod, $fixture_dir, $target, $lang)
                .with_preprocess(vec![$($preprocess),*]);
            let result = run_test(config).await;
            assert!(result.is_ok(), "Test failed: {:?}", result.err());
        }
    };
}

pub use jssg_test;
