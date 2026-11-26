use anyhow::Result;
use clap::Args;
use codemod_sandbox::sandbox::engine::{CodemodOutput, JssgExecutionOptions};
use codemod_telemetry::send_event::BaseEvent;
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::CodemodLang;
use codemod_sandbox::{
    sandbox::{
        engine::{execute_codemod_with_quickjs, language_data::get_extensions_for_language},
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use testing_utils::{
    map_execution_result, CaseEnd, CaseHook, ExecutionRequest, ReporterType, TestOptions,
    TestRunner, TestSource, TransformationResult,
};

use crate::commands::TelemetrySenderExt;
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::{TelemetrySenderMutex, CLI_VERSION};

use super::config::{ResolvedTestConfig, TestConfig};

/// What a test case's transform returned as `data`, by input file.
type DataEntries = Vec<(String, serde_json::Value)>;

#[derive(Clone)]
struct PendingDataSnapshot {
    entries: DataEntries,
    /// The case's `data.json`.
    path: PathBuf,
}

/// The `data` of a test case's files, checked against its `data.json` when
/// the case ends.
struct SharedDataState {
    entries: DataEntries,
    /// The case's `data.json`.
    path: PathBuf,
    /// A file stopped early (an error, a timeout): the data is incomplete.
    skip_snapshot: bool,
}

/// What each case's files returned as `data`, by the case's root.
type SharedData = Arc<Mutex<HashMap<PathBuf, SharedDataState>>>;

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
    /// For directory snapshot tests, the workspace root is automatically set
    /// to the test fixture's temporary directory.
    #[arg(long)]
    pub semantic_workspace: bool,

    /// Allow fs access
    #[arg(long)]
    pub allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    pub allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    pub allow_child_process: bool,

    /// The file is a forEachFile callback (its default export is
    /// `(file) => ...`) rather than a jssg transform. It runs on each input
    /// as a forEachFile step runs it; what it returns is checked against the
    /// case's data.json.
    #[arg(long)]
    pub callback: bool,
}

async fn send_failure_event(
    telemetry: &TelemetrySenderMutex,
    codemod_file: &str,
    error_category: &str,
    error_message: &str,
) {
    telemetry
        .send_event_logged(
            BaseEvent {
                kind: "failedToExecuteCommand".to_string(),
                properties: HashMap::from([
                    ("codemodName".to_string(), codemod_file.to_string()),
                    ("cliVersion".to_string(), CLI_VERSION.to_string()),
                    ("commandName".to_string(), "codemod.jssgTest".to_string()),
                    ("os".to_string(), std::env::consts::OS.to_string()),
                    ("arch".to_string(), std::env::consts::ARCH.to_string()),
                    ("errorCategory".to_string(), error_category.to_string()),
                    ("errorMessage".to_string(), error_message.to_string()),
                ]),
            },
            None,
        )
        .await;
}

pub async fn handler(args: &Command, telemetry: TelemetrySenderMutex) -> Result<()> {
    let result = handler_impl(args).await;
    if let Err(error) = &result {
        let (error_category, error_message) = classify_and_sanitize_error(&error.to_string());
        send_failure_event(
            &telemetry,
            &args.codemod_file,
            error_category,
            &error_message,
        )
        .await;
    }
    result
}

async fn handler_impl(args: &Command) -> Result<()> {
    let codemod_path = Path::new(&args.codemod_file);

    if !codemod_path.exists() {
        anyhow::bail!("Codemod file '{}' does not exist", codemod_path.display());
    }

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
        reporter: global_config.reporter.clone(),
        timeout: std::time::Duration::from_secs(global_config.timeout),
        ignore_whitespace: global_config.ignore_whitespace,
        context_lines: global_config.context_lines,
        expect_errors: global_config.expect_errors,
        strictness,
        language: global_config.language.clone(),
        expected_extension: global_config.expected_extension.clone(),
    };
    let runtime_event_output = if matches!(global_config.reporter, ReporterType::Json) {
        super::RuntimeEventOutput::stderr()
    } else {
        super::RuntimeEventOutput::stdout()
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
    let shared_data: SharedData = Arc::default();
    // data.json belongs to the whole case: it is checked when the case ends,
    // as the case's own result.
    let case_hook = data_case_hook(shared_data.clone(), args.update_snapshots);
    let semantic_provider: Option<Arc<dyn SemanticProvider>> =
        Some(Arc::new(LazySemanticProvider::file_scope()));
    let recovered = Arc::new(AtomicUsize::new(0));
    let recovered_for_test = Arc::clone(&recovered);
    let execution_fn = Box::new(
        move |request: ExecutionRequest, capabilities: Option<HashSet<LlrtSupportedModules>>| {
            let codemod_path = codemod_path_clone.clone();
            let resolver = resolver.clone();
            let input_code = request.input_code;
            let input_path = request.input_path;
            let logical_input_path = request.logical_input_path;
            let workspace_root = request.workspace_root;
            let test_case_root = request.test_case_root;
            let base_config = base_config_clone.clone();
            let args = args_clone.clone();
            let current_dir = current_dir_clone.clone();
            let semantic_provider = semantic_provider.clone();
            let shared_data = shared_data.clone();
            let runtime_event_output = runtime_event_output.clone();
            let recovered = Arc::clone(&recovered_for_test);

            Box::pin(async move {
                let logical_input_path = logical_input_path.unwrap_or_else(|| input_path.clone());
                let test_case_dir = logical_input_path
                    .parent()
                    .unwrap_or(logical_input_path.as_path());
                let target_directory = workspace_root
                    .clone()
                    .unwrap_or_else(|| test_case_dir.to_path_buf());
                // One data.json per case: a directory case's at its root, next to
                // input/ and expected/, so it is never among the transform's output
                // files; a single-file case's next to its input file.
                let data_path = if workspace_root.is_some() {
                    test_case_root.join("data.json")
                } else {
                    test_case_dir.join("data.json")
                };
                // This file's part of the case's data.json; when it stops
                // early (an error, a timeout), the case's data is not checked.
                let collection =
                    DataCollection::start(shared_data.clone(), test_case_root, data_path)?;
                let per_test_config =
                    match TestConfig::load_hierarchical(test_case_dir, Some(current_dir.as_path()))
                    {
                        Ok(config) => config,
                        Err(error) => return Err(error),
                    };

                let test_config = match ResolvedTestConfig::resolve(
                    &args,
                    &base_config,
                    Some(&per_test_config),
                ) {
                    Ok(config) => config,
                    Err(error) => return Err(error),
                };

                let language_str = test_config
                    .language
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Language must be specified for test case"))?;
                let language_enum: CodemodLang = language_str
                    .parse()
                    .map_err(|e: String| anyhow::anyhow!("{}", e))?;

                // For directory snapshot tests with --semantic-workspace,
                // create a workspace-scoped provider using the temp dir
                // and pre-index all files (matching jssg run behavior).
                let semantic_provider = if test_config.semantic_workspace {
                    if let Some(ref ws_root) = workspace_root {
                        let provider: Arc<dyn SemanticProvider> =
                            Arc::new(LazySemanticProvider::workspace_scope(ws_root.clone()));
                        for entry in walkdir::WalkDir::new(ws_root)
                            .into_iter()
                            .filter_map(|e| e.ok())
                            .filter(|e| e.file_type().is_file())
                        {
                            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                                let _ = provider.notify_file_processed(entry.path(), &content);
                            }
                        }
                        Some(provider)
                    } else {
                        semantic_provider
                    }
                } else {
                    semantic_provider
                };
                let runtime_event_buffer = super::RuntimeEventBuffer::new();
                let runtime_event_callback = runtime_event_buffer.callback_for_title(
                    super::display_path_title(&logical_input_path, Some(&current_dir)),
                );

                let file_name = data_file_name(&input_path, &target_directory);
                let mut data = None;
                let execution_result = if args.callback {
                    run_callback(CallbackCase {
                        callback: &codemod_path,
                        resolver,
                        target: &target_directory,
                        path: &file_name,
                        text: input_code,
                        language: language_str,
                        params: test_config.params.as_ref(),
                        semantic_provider,
                        runtime_event_callback,
                    })
                    .await
                    .map(|(result, output)| {
                        data = output;
                        result
                    })
                } else {
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
                        llm_request_handler: None,
                        runtime_event_callback: Some(runtime_event_callback),
                        cancellation_flag: None,
                        test_mode: true,
                        dry_run: false,
                        target_directory: &target_directory,
                        pending_files: None,
                    };
                    execute_codemod_with_quickjs(options)
                        .await
                        .map(|output: CodemodOutput| {
                            if output.recovered_syntax {
                                recovered.fetch_add(1, Ordering::Relaxed);
                            }
                            data = output.data;
                            map_execution_result(output.primary, input_code)
                        })
                        .map_err(|error| with_callback_hint(anyhow::Error::from(error)))
                };

                collection.finish(
                    data.map(|data| (file_name, data)),
                    execution_result.is_err(),
                )?;

                runtime_event_output.flush(&runtime_event_buffer);

                execution_result
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
        .run_tests_with_case_hook(
            &extensions,
            execution_fn,
            Some(case_hook),
            Some(capabilities),
        )
        .await?;

    let recovered = recovered.load(Ordering::Relaxed);
    if recovered > 0 {
        eprintln!("{recovered} files had syntax errors; transformed with error recovery");
    }
    if !summary.is_success() {
        std::process::exit(1);
    }

    Ok(())
}

/// Checks a case's `data.json` once the case ends, against the data its
/// files returned; reported as `<case>: data.json`. Every file of a case runs
/// before it ends, so the data is complete whichever files `--filter`
/// reports, and a file an earlier one deleted is simply not in it. A case
/// with a file that stopped early (an error, a timeout, `--fail-fast`) is
/// not checked, and one without data or a data.json reports nothing.
fn data_case_hook(shared_data: SharedData, update_snapshots: bool) -> CaseHook<'static> {
    Box::new(move |case: CaseEnd| {
        let state = shared_data.lock().ok()?.remove(&case.root)?;
        if case.stopped_early || state.skip_snapshot {
            return None;
        }
        if state.entries.is_empty() && !state.path.exists() {
            return None;
        }
        let snapshot = PendingDataSnapshot {
            entries: state.entries,
            path: state.path,
        };
        Some((
            format!("{}: data.json", case.name),
            handle_data_snapshot(&snapshot, update_snapshots),
        ))
    })
}

/// One input of a `--callback` test case.
struct CallbackCase<'a> {
    callback: &'a Path,
    resolver: Arc<OxcResolver>,
    target: &'a Path,
    /// From the target, with `/`: the callback's `file.path`.
    path: &'a str,
    text: String,
    language: &'a str,
    params: Option<&'a HashMap<String, serde_json::Value>>,
    semantic_provider: Option<Arc<dyn SemanticProvider>>,
    runtime_event_callback: codemod_sandbox::sandbox::runtime_module::RuntimeEventCallback,
}

/// Run a forEachFile callback file on one input, as a `forEachFile` step
/// runs it: the file's new text (and where it was renamed to), and what the
/// callback returned.
async fn run_callback(
    case: CallbackCase<'_>,
) -> Result<(TransformationResult, Option<serde_json::Value>)> {
    if case.params.is_some_and(|params| !params.is_empty()) {
        anyhow::bail!(
            "params are not given to a forEachFile callback: a callback file has no options; \
             remove `params` from the test config, or read the value in the workflow and pass it \
             to an inline callback"
        );
    }
    let run = codemod_workflow::run_callback_file(codemod_workflow::CallbackFileRequest {
        callback: case.callback.to_path_buf(),
        resolver: case.resolver,
        target: case.target.to_path_buf(),
        path: case.path.to_string(),
        text: case.text.clone(),
        language: Some(case.language.to_string()),
        semantic_provider: case.semantic_provider,
        runtime_event_callback: Some(case.runtime_event_callback),
    })
    .await
    .map_err(|error| anyhow::anyhow!(error))?;
    if run.deleted {
        anyhow::bail!("the callback deleted {}: jssg test cannot check deleted files; test it with codemod/testing", case.path);
    }
    let rename_to = run.renamed.map(|to| {
        to.split('/')
            .filter(|part| !part.is_empty())
            .fold(case.target.to_path_buf(), |path, part| path.join(part))
    });
    let result = TransformationResult::Success(testing_utils::TransformOutput {
        content: run.written.unwrap_or(case.text),
        rename_to,
    });
    Ok((result, run.output))
}

/// A forEachFile callback run as a jssg transform fails in `parse(root)`:
/// say to pass `--callback`.
fn with_callback_hint(error: anyhow::Error) -> anyhow::Error {
    if format!("{error:#}").contains("parse(file) expects an object with a string `text`") {
        anyhow::anyhow!(
            "{error}\n\nIf this file is a forEachFile callback (`export default (file) => ...`), run it with --callback"
        )
    } else {
        error
    }
}

/// A file's name in `data.json`: its path from the target, with `/`, as a
/// `jssg` step's `data` names it.
fn data_file_name(input_path: &Path, target_directory: &Path) -> String {
    input_path
        .strip_prefix(target_directory)
        .unwrap_or(input_path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// One file's part in its case's data. Dropped before
/// [`DataCollection::finish`] (the file stopped early), it marks the case's
/// data incomplete, so it is not checked.
struct DataCollection {
    shared: SharedData,
    /// The case's root, which keys its data.
    case: PathBuf,
    finished: bool,
}

impl DataCollection {
    fn start(shared: SharedData, case: PathBuf, data_path: PathBuf) -> Result<Self> {
        shared
            .lock()
            .map_err(|_| anyhow::anyhow!("shared data lock poisoned"))?
            .entry(case.clone())
            .or_insert_with(|| SharedDataState {
                entries: Vec::new(),
                path: data_path,
                skip_snapshot: false,
            });
        Ok(Self {
            shared,
            case,
            finished: false,
        })
    }

    fn finish(
        mut self,
        entry: Option<(String, serde_json::Value)>,
        stopped_early: bool,
    ) -> Result<()> {
        self.finished = true;
        self.add(entry, stopped_early)
    }

    fn add(&self, entry: Option<(String, serde_json::Value)>, stopped_early: bool) -> Result<()> {
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| anyhow::anyhow!("shared data lock poisoned"))?;
        if let Some(state) = shared.get_mut(&self.case) {
            state.entries.extend(entry);
            state.skip_snapshot |= stopped_early;
        }
        Ok(())
    }
}

impl Drop for DataCollection {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.add(None, true);
        }
    }
}

fn handle_data_snapshot(snapshot: &PendingDataSnapshot, update_snapshots: bool) -> Result<()> {
    let data_path = &snapshot.path;

    if !snapshot.entries.is_empty() {
        let actual_json = data_to_canonical_json(&snapshot.entries)?;

        if data_path.exists() {
            let expected_json = std::fs::read_to_string(data_path)?;
            let normalized_expected_json = normalize_line_endings(&expected_json);
            let normalized_actual_json = normalize_line_endings(&actual_json);

            if normalized_actual_json != normalized_expected_json {
                if update_snapshots {
                    std::fs::write(data_path, &actual_json)?;
                } else {
                    anyhow::bail!(
                        "Data mismatch:\n--- expected\n+++ actual\n{}",
                        generate_data_diff(&normalized_expected_json, &normalized_actual_json)
                    );
                }
            }
        } else {
            std::fs::write(data_path, &actual_json)?;
        }
    } else if data_path.exists() {
        if update_snapshots {
            std::fs::remove_file(data_path)?;
        } else {
            anyhow::bail!(
                "Data snapshot exists at {} but the codemod returned no data. \
                 Run with --update-snapshots to remove the stale snapshot.",
                data_path.display()
            );
        }
    }

    Ok(())
}

fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

fn classify_and_sanitize_error(error_message: &str) -> (&'static str, String) {
    if error_message.contains("Data mismatch:") {
        return ("data_mismatch", "Data snapshot mismatch".to_string());
    }

    if error_message.contains("Data snapshot exists at") {
        return ("stale_data_snapshot", "Stale data snapshot".to_string());
    }

    if error_message.contains("Codemod file '") && error_message.contains(" does not exist") {
        return (
            "codemod_not_found",
            "Codemod file does not exist".to_string(),
        );
    }

    let first_line = error_message
        .split('\n')
        .next()
        .unwrap_or("")
        .trim()
        .replace('\r', "");
    let sanitized = truncate_with_ellipsis(&first_line, 200);
    ("command_error", sanitized)
}

fn truncate_with_ellipsis(input: &str, max_chars: usize) -> String {
    let char_count = input.chars().count();
    if char_count <= max_chars {
        return input.to_string();
    }

    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}...")
}

/// The step result's shape, `[{ file, data }]` in file order, as canonical
/// JSON (RFC 8785) printed readably, so object keys never reorder.
fn data_to_canonical_json(entries: &DataEntries) -> Result<String> {
    let mut entries = entries.clone();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let json_value = serde_json::Value::Array(
        entries
            .into_iter()
            .map(|(file, data)| serde_json::json!({ "file": file, "data": data }))
            .collect(),
    );
    let canonical = String::from_utf8(serde_json_canonicalizer::to_vec(&json_value)?)?;
    let reparsed: serde_json::Value = serde_json::from_str(&canonical)?;
    Ok(serde_json::to_string_pretty(&reparsed)?)
}

fn generate_data_diff(expected: &str, actual: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::{
        classify_and_sanitize_error, data_case_hook, data_file_name, data_to_canonical_json,
        generate_data_diff, handle_data_snapshot, normalize_line_endings, DataCollection,
        PendingDataSnapshot, SharedData,
    };
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tempfile::tempdir;
    use testing_utils::CaseEnd;

    #[test]
    fn normalize_line_endings_converts_crlf_to_lf() {
        assert_eq!(normalize_line_endings("[\r\n]\r\n"), "[\n]\n");
    }

    #[test]
    fn data_diff_reports_changed_lines() {
        let diff = generate_data_diff("{\n  \"count\": 1\n}\n", "{\n  \"count\": 2\n}\n");
        assert!(
            diff.contains("-  \"count\": 1") && diff.contains("+  \"count\": 2"),
            "{diff}"
        );
    }

    #[test]
    fn the_data_snapshot_lists_files_in_path_order() {
        let entries = vec![
            ("b.ts".to_string(), json!({ "z": 1, "a": 2 })),
            ("a.ts".to_string(), json!([1])),
        ];
        let written: serde_json::Value =
            serde_json::from_str(&data_to_canonical_json(&entries).unwrap()).unwrap();
        assert_eq!(
            written,
            json!([{ "file": "a.ts", "data": [1] }, { "file": "b.ts", "data": { "a": 2, "z": 1 } }])
        );
    }

    #[test]
    fn a_nested_files_data_is_named_by_its_path_from_the_target() {
        let target = PathBuf::from("/tmp/workspace");
        assert_eq!(
            data_file_name(&target.join("src/a.ts"), &target),
            "src/a.ts"
        );
    }

    #[test]
    fn error_classification_redacts_the_data_diff() {
        let input = "Data mismatch:\n--- expected\n+++ actual\n-{\"code\":\"secret\"}";
        assert_eq!(
            classify_and_sanitize_error(input),
            ("data_mismatch", "Data snapshot mismatch".to_string())
        );
    }

    #[test]
    fn error_classification_truncates_generic_errors() {
        let (category, message) = classify_and_sanitize_error(&"x".repeat(260));
        assert_eq!(category, "command_error");
        assert_eq!(message.len(), 203);
    }

    fn case_end(root: &Path, stopped_early: bool) -> CaseEnd {
        CaseEnd {
            name: "case".into(),
            root: root.to_path_buf(),
            stopped_early,
        }
    }

    #[test]
    fn a_case_snapshots_its_files_data_once_it_ends() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.json");
        let shared: SharedData = Arc::default();
        let hook = data_case_hook(shared.clone(), false);
        for (file, value) in [("b.ts", 2), ("a.ts", 1)] {
            DataCollection::start(shared.clone(), dir.path().into(), path.clone())
                .unwrap()
                .finish(Some((file.into(), json!(value))), false)
                .unwrap();
        }
        assert!(!path.exists());

        let (name, result) = hook(case_end(dir.path(), false)).expect("a data check");
        assert_eq!(name, "case: data.json");
        result.unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written,
            json!([{ "file": "a.ts", "data": 1 }, { "file": "b.ts", "data": 2 }])
        );
        assert!(shared.lock().unwrap().is_empty());

        // Different data is a failure of the case.
        DataCollection::start(shared.clone(), dir.path().into(), path.clone())
            .unwrap()
            .finish(Some(("a.ts".into(), json!(3))), false)
            .unwrap();
        let (_, result) = hook(case_end(dir.path(), false)).expect("a data check");
        assert!(format!("{:#}", result.unwrap_err()).contains("Data mismatch"));
    }

    #[test]
    fn a_case_cut_short_is_not_checked() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.json");
        let shared: SharedData = Arc::default();
        let hook = data_case_hook(shared.clone(), false);

        // A file that stopped early (dropped unfinished).
        drop(DataCollection::start(shared.clone(), dir.path().into(), path.clone()).unwrap());
        DataCollection::start(shared.clone(), dir.path().into(), path.clone())
            .unwrap()
            .finish(Some(("a.ts".into(), json!(1))), false)
            .unwrap();
        assert!(hook(case_end(dir.path(), false)).is_none());

        // A case --fail-fast stopped.
        DataCollection::start(shared.clone(), dir.path().into(), path.clone())
            .unwrap()
            .finish(Some(("a.ts".into(), json!(1))), false)
            .unwrap();
        assert!(hook(case_end(dir.path(), true)).is_none());
        assert!(!path.exists());
        assert!(shared.lock().unwrap().is_empty());
    }

    #[test]
    fn a_case_without_data_reports_nothing() {
        let dir = tempdir().unwrap();
        let shared: SharedData = Arc::default();
        let hook = data_case_hook(shared.clone(), false);
        DataCollection::start(
            shared.clone(),
            dir.path().into(),
            dir.path().join("data.json"),
        )
        .unwrap()
        .finish(None, false)
        .unwrap();
        assert!(hook(case_end(dir.path(), false)).is_none());
    }

    #[test]
    fn a_missing_data_json_is_written() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.json");
        handle_data_snapshot(
            &PendingDataSnapshot {
                entries: vec![("input.ts".into(), json!({ "calls": 2 }))],
                path: path.clone(),
            },
            false,
        )
        .unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            written,
            json!([{ "file": "input.ts", "data": { "calls": 2 } }])
        );
    }
}
