//! Test fixtures and helpers for semantic analysis integration tests.
//!
//! A fixture is a small source tree; its codemod registers named cases with
//! `codemods/spec.js` (one rule each), and `jssg_test!` runs them on the
//! fixture's targets with one semantic provider. Transform tests compare a
//! codemod's output with expected files instead.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, CodemodOutput, ExecutionResult, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use codemod_sandbox::CodemodLang;
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

    // Copy all files (recursively) from fixture directory to temp directory.
    // Keys are `/`-separated paths relative to the fixture directory.
    if fixture_path.exists() {
        copy_fixture_dir(&fixture_path, temp_dir.path(), "", &mut file_paths);
    }

    (temp_dir, file_paths)
}

fn copy_fixture_dir(
    src: &Path,
    dest: &Path,
    prefix: &str,
    file_paths: &mut HashMap<String, PathBuf>,
) {
    for entry in fs::read_dir(src).expect("Failed to read fixture directory") {
        let entry = entry.expect("Failed to read directory entry");
        let file_name = entry.file_name();
        let key = format!("{prefix}{}", file_name.to_string_lossy());
        let file_type = entry.file_type().expect("Failed to read file type");
        let dest_path = dest.join(&file_name);

        if file_type.is_dir() {
            fs::create_dir_all(&dest_path).expect("Failed to create fixture subdirectory");
            copy_fixture_dir(&entry.path(), &dest_path, &format!("{key}/"), file_paths);
        } else if file_type.is_file() {
            let content = fs::read_to_string(entry.path()).unwrap_or_else(|e| {
                panic!("Failed to read fixture file {:?}: {}", entry.path(), e)
            });
            fs::write(&dest_path, &content)
                .unwrap_or_else(|e| panic!("Failed to write fixture file {:?}: {}", dest_path, e));
            file_paths.insert(key, dest_path);
        }
    }
}

/// Provider scope type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderScope {
    /// File-scoped analysis (single file)
    File,
    /// Workspace-scoped analysis (cross-file)
    Workspace,
    /// Workspace scope with types from the TypeScript 7 checker
    /// (`types: "ts7"`).
    WorkspaceWithTypes,
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
        ProviderScope::WorkspaceWithTypes => {
            let path = workspace_path.expect("Workspace path required for workspace scope");
            Arc::new(OxcSemanticProvider::workspace_scope(path.to_path_buf()).with_types(true))
        }
    }
}

/// Whether a TypeScript 7 checker is around for tests that need one:
/// `CODEMOD_TS_API` names it (CI installs the pinned version).
pub fn ts7_checker_available() -> bool {
    std::env::var("CODEMOD_TS_API").is_ok_and(|value| !value.is_empty() && value != "off")
}

/// Create a semantic provider for Python
pub fn create_python_provider(
    scope: ProviderScope,
    workspace_path: Option<&Path>,
) -> Arc<dyn SemanticProvider> {
    match scope {
        ProviderScope::File => Arc::new(RuffSemanticProvider::file_scope()),
        ProviderScope::Workspace | ProviderScope::WorkspaceWithTypes => {
            let path = workspace_path.expect("Workspace path required for workspace scope");
            Arc::new(RuffSemanticProvider::workspace_scope(path.to_path_buf()))
        }
    }
}

/// The provider for `scope` and `language`, or none without a scope.
fn provider_for(
    scope: Option<ProviderScope>,
    language: CodemodLang,
    workspace: &Path,
) -> Option<Arc<dyn SemanticProvider>> {
    let scope = scope?;
    Some(match language {
        CodemodLang::Static(SupportLang::JavaScript)
        | CodemodLang::Static(SupportLang::TypeScript)
        | CodemodLang::Static(SupportLang::Tsx) => create_js_provider(scope, Some(workspace)),
        CodemodLang::Static(SupportLang::Python) => create_python_provider(scope, Some(workspace)),
        _ => panic!("Unsupported language: {language:?}"),
    })
}

/// The language a target is parsed as, from its extension.
pub fn language_of(target: &str) -> CodemodLang {
    let lang = match Path::new(target).extension().and_then(|e| e.to_str()) {
        Some("ts" | "mts" | "cts") => SupportLang::TypeScript,
        Some("tsx") => SupportLang::Tsx,
        Some("js" | "jsx" | "mjs" | "cjs") => SupportLang::JavaScript,
        Some("py") => SupportLang::Python,
        _ => panic!("no language for target {target}"),
    };
    CodemodLang::Static(lang)
}

/// The codemod and `spec.js` it imports, in a directory of their own, so
/// the workspace under analysis holds only the fixture's files.
fn codemod_dir(codemod_name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("Failed to create codemod directory");
    let path = dir.path().join("codemod.js");
    fs::write(&path, load_codemod(codemod_name)).expect("Failed to write codemod");
    fs::write(dir.path().join("spec.js"), load_codemod("spec.js"))
        .expect("Failed to write spec.js");
    (dir, path)
}

async fn execute(
    codemod_path: &Path,
    target_path: &Path,
    language: CodemodLang,
    provider: Option<Arc<dyn SemanticProvider>>,
) -> Result<CodemodOutput, String> {
    let content = fs::read_to_string(target_path)
        .unwrap_or_else(|e| panic!("Failed to read target {target_path:?}: {e}"));
    let resolver =
        Arc::new(OxcResolver::new(codemod_path.parent().unwrap().to_path_buf(), None).unwrap());
    let options = JssgExecutionOptions {
        script_path: codemod_path,
        resolver,
        language,
        file_path: target_path,
        content: &content,
        selector_config: None,
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: provider,
        llm_request_handler: None,
        runtime_event_callback: None,
        cancellation_flag: None,
        test_mode: false,
        dry_run: false,
        target_directory: target_path.parent().unwrap_or(target_path),
        pending_files: None,
    };
    execute_codemod_with_quickjs(options)
        .await
        .map_err(|e| format!("Execution failed: {e:?}"))
}

/// A fixture whose codemod registers named cases with `spec.js`.
pub struct Spec<'a> {
    pub fixture_dir: &'a str,
    pub codemod: &'a str,
    pub targets: &'a [&'a str],
    /// `None` runs without a semantic provider.
    pub scope: Option<ProviderScope>,
    /// Files the provider is told about before the codemod runs.
    pub preprocess: &'a [&'a str],
}

/// Runs the spec's codemod on each target, with one provider for the
/// fixture, and fails with every case that failed, by name. Each case must
/// run on exactly one target.
pub async fn run_spec(spec: Spec<'_>) {
    let (workspace, _) = setup_test_workspace(spec.fixture_dir);
    let (_codemod_dir, codemod_path) = codemod_dir(spec.codemod);
    let language = language_of(spec.targets[0]);
    let provider = provider_for(spec.scope, language, workspace.path());
    if let Some(provider) = &provider {
        for file in spec.preprocess {
            let path = workspace.path().join(file);
            let content = fs::read_to_string(&path).expect("Failed to read preprocessed file");
            provider
                .notify_file_processed(&path, &content)
                .expect("Failed to preprocess file");
        }
    }

    let mut ran: Vec<Case> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for target in spec.targets {
        let path = workspace.path().join(target);
        let output = execute(&codemod_path, &path, language_of(target), provider.clone()).await;
        let cases = match output {
            Ok(CodemodOutput {
                data: Some(serde_json::Value::Array(cases)),
                ..
            }) => cases,
            Ok(_) => {
                problems.push(format!(
                    "{target}: the codemod did not answer cases (export run as default)"
                ));
                continue;
            }
            Err(e) => {
                problems.push(format!("{target}: {e}"));
                continue;
            }
        };
        for case in cases {
            let name = case["name"].as_str().unwrap_or_default().to_string();
            match case["status"].as_str() {
                Some("skip") => skipped.push(name),
                Some(status) => {
                    if let Some(other) = ran.iter().find(|c| c.name == name) {
                        problems.push(format!(
                            "\"{name}\" ran on {} and {target}: put it in onFile()",
                            other.target
                        ));
                        continue;
                    }
                    ran.push(Case {
                        name,
                        target,
                        failure: (status == "fail")
                            .then(|| case["message"].as_str().unwrap_or_default().to_string()),
                        ms: case["ms"].as_u64().unwrap_or_default(),
                    });
                }
                None => problems.push(format!("{target}: a case without a status: {case}")),
            }
        }
    }
    skipped.sort();
    skipped.dedup();
    for name in skipped {
        if !ran.iter().any(|c| c.name == name) {
            problems.push(format!("\"{name}\" matched no target"));
        }
    }

    for case in &ran {
        match &case.failure {
            None => println!("  ok    {} ({} ms)", case.name, case.ms),
            Some(message) => println!("  FAIL  {} ({}): {message}", case.name, case.target),
        }
    }
    let failed = ran.iter().filter_map(|case| {
        let message = case.failure.as_ref()?;
        Some(format!("\"{}\" ({}): {message}", case.name, case.target))
    });
    let mut report = problems;
    report.extend(failed);
    assert!(
        report.is_empty(),
        "{} of {} cases failed in {}:\n{}",
        report.len(),
        ran.len(),
        spec.codemod,
        report.join("\n")
    );
}

/// One case that ran, on its target, with its failure if it failed.
struct Case<'a> {
    name: String,
    target: &'a str,
    failure: Option<String>,
    ms: u64,
}

/// A transform test: the codemod's output must match an expected file, or
/// the workspace an expected directory.
pub struct Transform<'a> {
    pub fixture_dir: &'a str,
    pub codemod: &'a str,
    pub target: &'a str,
    pub scope: ProviderScope,
    pub expected_file: Option<&'a str>,
    pub expected_dir: Option<&'a str>,
}

pub async fn run_transform(t: Transform<'_>) -> Result<(), String> {
    let (workspace, _) = setup_test_workspace(t.fixture_dir);
    let (_codemod_dir, codemod_path) = codemod_dir(t.codemod);
    let language = language_of(t.target);
    let provider = provider_for(Some(t.scope), language, workspace.path());
    let target_path = workspace.path().join(t.target);
    let output = execute(&codemod_path, &target_path, language, provider).await?;
    let modified = match output.primary {
        ExecutionResult::Modified(modified) => Some(modified.content),
        _ => None,
    };

    if let Some(expected_file) = t.expected_file {
        let expected = load_fixture(t.fixture_dir, expected_file);
        let actual = modified.clone().unwrap_or_default();
        if actual.trim() != expected.trim() {
            return Err(format!(
                "Output mismatch.\nExpected:\n{expected}\n\nGot:\n{actual}"
            ));
        }
    }
    if let Some(expected_dir) = t.expected_dir {
        // The target's own result is returned, not written: apply it first.
        if let Some(content) = &modified {
            fs::write(&target_path, content).expect("Failed to write target");
        }
        let (_expected_temp, expected_files) = setup_test_workspace(expected_dir);
        let mut names: Vec<_> = expected_files.keys().collect();
        names.sort();
        for name in names {
            let expected = fs::read_to_string(&expected_files[name]).unwrap();
            let actual = fs::read_to_string(workspace.path().join(name))
                .unwrap_or_else(|_| format!("<{name} is missing>"));
            if actual.trim() != expected.trim() {
                return Err(format!(
                    "{name} does not match.\nExpected:\n{expected}\n\nGot:\n{actual}"
                ));
            }
        }
    }
    Ok(())
}

/// One test per fixture.
///
/// A spec: the codemod's named cases run on each target, and the test fails
/// with every failed case. `scope` is `none`, `file`, `workspace` or `ts7`
/// (workspace with the TypeScript 7 checker; skipped without one).
///
/// A transform: `expected` names the file the target must become, or
/// `expected_dir` the files the workspace must hold afterwards.
#[macro_export]
macro_rules! jssg_test {
    (
        name: $name:ident,
        fixture: $fixture:literal,
        codemod: $codemod:literal,
        targets: [$($target:literal),+ $(,)?],
        scope: $scope:ident
        $(, preprocess: [$($preprocess:literal),* $(,)?])?
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{run_spec, ProviderScope, Spec};
            let scope = match stringify!($scope) {
                "none" => None,
                "file" => Some(ProviderScope::File),
                "workspace" => Some(ProviderScope::Workspace),
                "ts7" => {
                    if !super::fixtures::ts7_checker_available() {
                        eprintln!("skipped: set CODEMOD_TS_API to a TypeScript 7.0 tsc");
                        return;
                    }
                    Some(ProviderScope::WorkspaceWithTypes)
                }
                other => panic!("unknown scope {other}"),
            };
            run_spec(Spec {
                fixture_dir: $fixture,
                codemod: $codemod,
                targets: &[$($target),+],
                scope,
                preprocess: &[$($($preprocess),*)?],
            })
            .await;
        }
    };

    (
        name: $name:ident,
        fixture: $fixture:literal,
        codemod: $codemod:literal,
        target: $target:literal,
        scope: $scope:ident,
        $kind:ident: $expected:literal
        $(,)?
    ) => {
        #[tokio::test]
        async fn $name() {
            use super::fixtures::{run_transform, ProviderScope, Transform};
            let scope = match stringify!($scope) {
                "file" => ProviderScope::File,
                "workspace" => ProviderScope::Workspace,
                other => panic!("unknown scope {other}"),
            };
            let expected = $expected;
            let (expected_file, expected_dir) = match stringify!($kind) {
                "expected" => (Some(expected), None),
                "expected_dir" => (None, Some(expected)),
                other => panic!("unknown expectation {other}"),
            };
            let result = run_transform(Transform {
                fixture_dir: $fixture,
                codemod: $codemod,
                target: $target,
                scope,
                expected_file,
                expected_dir,
            })
            .await;
            assert!(result.is_ok(), "Test failed: {}", result.err().unwrap_or_default());
        }
    };
}
