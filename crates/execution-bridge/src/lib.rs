//! Execution bridge for the TypeScript orchestration prototype.
//!
//! Turns a JSON `OperationRequest` into a JSON `OperationCompletion`. Exec uses
//! the existing `butterflow_runners::Runner`; JSSG uses the existing QuickJS
//! sandbox. No plans, replay, or history live here. See
//! `packages/orchestration/RUST_BRIDGE.md` for the TypeScript equivalent.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use butterflow_models::Error;
use butterflow_runners::Runner;
use codemod_sandbox::{
    sandbox::{
        engine::{
            codemod_lang::CodemodLang, execution_engine::execute_codemod_with_quickjs,
            extract_selector_with_quickjs, language_data::get_extensions_for_language,
            CodemodOutput, ExecutionResult, JssgExecutionOptions, SelectorEngineOptions,
        },
        filesystem::codemod_walk_builder,
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use ignore::overrides::{Override, OverrideBuilder};
use language_core::SemanticProvider;
use semantic_factory::LazySemanticProvider;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Must match `PROTOCOL_VERSION` in `packages/orchestration/src/protocol.ts`.
pub const PROTOCOL_VERSION: u32 = 2;

/// Every variant rejects fields it does not declare, so a `target` on `exec`
/// or `ai` is a parse error rather than a silently dropped field. Only `jssg`
/// carries a target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    Exec {
        command: String,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        env: HashMap<String, String>,
    },
    Jssg {
        /// Safe relative path, resolved against `RequestContext::script_root`
        /// (or the working directory). Never absolute, so command identity is
        /// stable across checkouts.
        script: String,
        language: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclude: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        semantic_analysis: Option<SemanticAnalysis>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Target>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
    /// Decoded for protocol parity only; no executor adapter exists yet.
    Ai {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SemanticAnalysis {
    Mode(SemanticMode),
    Detailed(SemanticAnalysisDetails),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticAnalysisDetails {
    pub mode: SemanticMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SemanticMode {
    File,
    Workspace,
}

/// Repository area one JSSG invocation applies to. Mirrors `Target` in
/// `protocol.ts`: `root` is relative to the working directory, `include` and
/// `exclude` are globs relative to `root`. Validation of author input happens
/// in TypeScript; this is the decoded wire shape, and unknown fields are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

impl Operation {
    pub fn kind(&self) -> &'static str {
        match self {
            Operation::Exec { .. } => "exec",
            Operation::Jssg { .. } => "jssg",
            Operation::Ai { .. } => "ai",
        }
    }
}

/// Executor-side context that is not part of command identity. It is set by
/// the host that spawns the bridge (for example the local workflow CLI) and is
/// never recorded in history, so it may hold machine-specific paths.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestContext {
    /// Directory that relative JSSG `script` paths are resolved against.
    /// Defaults to the working directory when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRequest {
    pub protocol_version: u32,
    pub command_id: String,
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<RequestContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionStatus {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationCompletion {
    pub protocol_version: u32,
    pub command_id: String,
    pub status: CompletionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CompletionError>,
}

impl OperationCompletion {
    fn succeeded(command_id: &str, output: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            command_id: command_id.to_string(),
            status: CompletionStatus::Succeeded,
            output: Some(output),
            error: None,
        }
    }

    fn not_succeeded(command_id: &str, status: CompletionStatus, error: CompletionError) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            command_id: command_id.to_string(),
            status,
            output: None,
            error: Some(error),
        }
    }
}

/// Parse a request and reject protocol versions this bridge does not speak.
pub fn parse_request(text: &str) -> Result<OperationRequest, String> {
    let request: OperationRequest =
        serde_json::from_str(text).map_err(|error| format!("invalid request JSON: {error}"))?;
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported protocolVersion {} (expected {PROTOCOL_VERSION})",
            request.protocol_version
        ));
    }
    if let Operation::Jssg {
        script,
        language,
        include,
        exclude,
        semantic_analysis,
        target,
        ..
    } = &request.operation
    {
        validate_relative_path(script, "JSSG script")?;
        if language.trim().is_empty() {
            return Err("JSSG language must not be empty".to_string());
        }
        for (name, patterns) in [("include", include), ("exclude", exclude)] {
            if patterns.as_ref().is_some_and(|values| {
                values.is_empty() || values.iter().any(|value| value.trim().is_empty())
            }) {
                return Err(format!("JSSG {name} must contain non-empty glob patterns"));
            }
        }
        if let Some(root) = target.as_ref().and_then(|target| target.root.as_deref()) {
            validate_relative_path(root, "JSSG target root")?;
        }
        if let Some(SemanticAnalysis::Detailed(details)) = semantic_analysis {
            if details.mode == SemanticMode::File && details.root.is_some() {
                return Err("semanticAnalysis.root requires workspace mode".to_string());
            }
            if let Some(root) = details.root.as_deref() {
                validate_relative_path(root, "semanticAnalysis.root")?;
            }
        }
    }
    if let Some(root) = request
        .context
        .as_ref()
        .and_then(|context| context.script_root.as_deref())
    {
        if root.trim().is_empty() {
            return Err("context.scriptRoot must not be empty".to_string());
        }
    }
    Ok(request)
}

/// Same rules as `isSafeRelativePath` in `packages/orchestration/src/paths.ts`:
/// non-empty, not absolute on any platform (`/x`, `\x`, `C:\x`), and no `..`
/// segment. A `..` inside a name such as `foo..bar` is allowed.
pub fn validate_relative_path(value: &str, name: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if is_absolute_path(trimmed) || escapes_root(trimmed) {
        return Err(format!("{name} must be a safe relative path"));
    }
    Ok(())
}

fn is_absolute_path(value: &str) -> bool {
    let path = Path::new(value);
    let bytes = value.as_bytes();
    path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
        || value.starts_with('/')
        || value.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn escapes_root(value: &str) -> bool {
    value.split(['/', '\\']).any(|segment| segment == "..")
}

/// Execute one request through the given runner, using the process working
/// directory as the repository root.
pub async fn execute(runner: &dyn Runner, request: &OperationRequest) -> OperationCompletion {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            return OperationCompletion::not_succeeded(
                &request.command_id,
                CompletionStatus::Failed,
                CompletionError {
                    message: format!("failed to read working directory: {error}"),
                    exit_code: None,
                    output: None,
                },
            )
        }
    };
    execute_in(runner, request, &cwd).await
}

/// Execute one request with an explicit repository root. `exec` still runs in
/// the process working directory (the runner owns that); JSSG resolves its
/// target and, absent `context.scriptRoot`, its script beneath `cwd`.
pub async fn execute_in(
    runner: &dyn Runner,
    request: &OperationRequest,
    cwd: &Path,
) -> OperationCompletion {
    match &request.operation {
        Operation::Exec { command, env } => {
            let mut merged: HashMap<String, String> = std::env::vars().collect();
            merged.extend(env.clone());
            let result = runner.run_command(command, &merged, None).await;
            completion_from_result(&request.command_id, result)
        }
        Operation::Jssg {
            script,
            language,
            include,
            exclude,
            semantic_analysis,
            target,
            input,
        } => {
            let args = JssgArgs {
                cwd,
                script,
                language,
                include: include.as_deref(),
                exclude: exclude.as_deref(),
                semantic_analysis: semantic_analysis.as_ref(),
                target: target.as_ref(),
                input: input.as_ref(),
                context: request.context.as_ref(),
            };
            match execute_jssg(args).await {
                Ok(output) => OperationCompletion::succeeded(&request.command_id, output),
                Err(failure) => OperationCompletion::not_succeeded(
                    &request.command_id,
                    // Before the bridge's first write nothing has changed, so the
                    // command is `failed`. Afterwards earlier files may already be
                    // modified and the outcome is `unknown`.
                    if failure.after_write {
                        CompletionStatus::Unknown
                    } else {
                        CompletionStatus::Failed
                    },
                    CompletionError {
                        message: failure.message,
                        exit_code: None,
                        output: None,
                    },
                ),
            }
        }
        Operation::Ai { .. } => OperationCompletion::not_succeeded(
            &request.command_id,
            CompletionStatus::Failed,
            CompletionError {
                message: "operation kind 'ai' has no executor adapter in the execution bridge"
                    .to_string(),
                exit_code: None,
                output: None,
            },
        ),
    }
}

struct JssgArgs<'a> {
    cwd: &'a Path,
    script: &'a str,
    language: &'a str,
    include: Option<&'a [String]>,
    exclude: Option<&'a [String]>,
    semantic_analysis: Option<&'a SemanticAnalysis>,
    target: Option<&'a Target>,
    input: Option<&'a Value>,
    context: Option<&'a RequestContext>,
}

/// A JSSG failure plus whether the bridge had already written to disk. Errors
/// produced by `String` conversion are always pre-write.
struct JssgFailure {
    message: String,
    after_write: bool,
}

impl From<String> for JssgFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            after_write: false,
        }
    }
}

/// A file the bridge wrote, with the content it now holds.
struct Written {
    path: PathBuf,
    content: String,
    /// Original path of a renamed file; removed after every file has run so a
    /// rename cannot delete a source that is still waiting to be processed.
    deferred_deletion: Option<PathBuf>,
}

async fn execute_jssg(args: JssgArgs<'_>) -> Result<Value, JssgFailure> {
    let JssgArgs {
        cwd,
        script,
        language,
        include,
        exclude,
        semantic_analysis,
        target,
        input,
        context,
    } = args;
    // Canonical so definition globs and walked paths share one root form
    // (macOS temp dirs, for example, live under a symlinked `/var`).
    let cwd = cwd
        .canonicalize()
        .map_err(|error| format!("failed to resolve working directory: {error}"))?;
    let cwd = cwd.as_path();
    let script_path = resolve_script(cwd, context, script)?;
    let target_root = resolve_target_root(cwd, target.and_then(|value| value.root.as_deref()))?;
    let language: CodemodLang = language
        .parse()
        .map_err(|error| format!("invalid JSSG language '{language}': {error}"))?;
    let intrinsic_include = intrinsic_include(language, include);
    let intrinsic = build_overrides(cwd, intrinsic_include.as_deref(), exclude)?;
    let invocation = build_overrides(
        &target_root,
        target.and_then(|value| value.include.as_deref()),
        target.and_then(|value| value.exclude.as_deref()),
    )?;
    let files = collect_files(&target_root, intrinsic.as_ref(), invocation.as_ref());
    let script_dir = script_path.parent().unwrap_or(Path::new("."));
    let resolver = Arc::new(
        OxcResolver::new(script_dir.to_path_buf(), find_tsconfig(script_dir))
            .map_err(|error| format!("failed to create JSSG resolver: {error}"))?,
    );
    // Invocation input reaches the transform as `options.params.input`. The
    // shipped selector engine passes no params, so `getSelector` sees `{}`.
    let params = input.map(|value| HashMap::from([("input".to_string(), value.clone())]));
    let selector = extract_selector_with_quickjs(SelectorEngineOptions {
        script_path: &script_path,
        language,
        resolver: Arc::clone(&resolver),
        capabilities: None,
        target_directory: Some(&target_root),
    })
    .await
    .map_err(|error| format!("failed to load JSSG selector: {error}"))?
    .map(Arc::from);
    let semantic_provider = build_semantic_provider(semantic_analysis, &target_root)?;
    if semantic_provider
        .as_ref()
        .is_some_and(|provider| provider.mode() == language_core::ProviderMode::WorkspaceScope)
    {
        let provider = semantic_provider.as_ref().expect("provider checked above");
        for file in &files {
            let Some(content) = read_source(file)? else {
                continue;
            };
            provider
                .notify_file_processed(file, &content)
                .map_err(|error| {
                    format!(
                        "failed to index '{}' for semantic analysis: {error}",
                        file.display()
                    )
                })?;
        }
    }

    let mut wrote = false;
    let mut deferred_deletions: Vec<PathBuf> = Vec::new();
    let mut outputs = Vec::new();
    for file in files {
        let Some(content) = read_source(&file).map_err(|message| JssgFailure {
            message,
            after_write: wrote,
        })?
        else {
            continue;
        };
        let result = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &script_path,
            resolver: Arc::clone(&resolver),
            language,
            file_path: &file,
            content: &content,
            selector_config: selector.clone(),
            params: params.clone(),
            matrix_values: None,
            capabilities: None,
            semantic_provider: semantic_provider.clone(),
            metrics_context: None,
            llm_request_handler: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            target_directory: &target_root,
        })
        .await
        .map_err(|error| JssgFailure {
            message: format!("JSSG failed for '{}': {error}", file.display()),
            after_write: wrote,
        })?;
        let written = apply_output(&target_root, &file, &result, &mut wrote)?;
        for entry in written {
            if let Some(provider) = &semantic_provider {
                provider
                    .notify_file_processed(&entry.path, &entry.content)
                    .map_err(|error| JssgFailure {
                        message: format!(
                            "failed to refresh '{}' for semantic analysis: {error}",
                            entry.path.display()
                        ),
                        after_write: true,
                    })?;
            }
            deferred_deletions.extend(entry.deferred_deletion);
        }
        if let Some(output) = result.output {
            outputs.push(output);
        }
    }
    for source in deferred_deletions {
        std::fs::remove_file(&source).map_err(|error| JssgFailure {
            message: format!("failed to remove renamed '{}': {error}", source.display()),
            after_write: true,
        })?;
    }
    Ok(Value::Array(outputs))
}

/// Read a file the walker enumerated. `None` means the file disappeared since
/// enumeration or is not valid UTF-8; both are skipped, as the workflow engine
/// does, so an unrelated binary cannot abort a run.
fn read_source(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("failed to read '{}': {error}", path.display())),
    }
}

fn resolve_script(
    cwd: &Path,
    context: Option<&RequestContext>,
    script: &str,
) -> Result<PathBuf, String> {
    validate_relative_path(script, "JSSG script")?;
    let root = match context.and_then(|context| context.script_root.as_deref()) {
        Some(root) => {
            let root = Path::new(root);
            if root.is_absolute() {
                root.to_path_buf()
            } else {
                cwd.join(root)
            }
        }
        None => cwd.to_path_buf(),
    };
    let candidate = root.join(script);
    candidate.canonicalize().map_err(|error| {
        format!(
            "failed to resolve JSSG script '{}': {error}",
            candidate.display()
        )
    })
}

fn resolve_target_root(cwd: &Path, root: Option<&str>) -> Result<PathBuf, String> {
    let root = root.unwrap_or(".");
    if root != "." {
        validate_relative_path(root, "JSSG target root")?;
    }
    cwd.join(root)
        .canonicalize()
        .map_err(|error| format!("failed to resolve JSSG target root '{root}': {error}"))
}

/// Definition include patterns, or the language's file extensions when the
/// definition has none. Mirrors `CodemodExecutionConfig::build_globs` in the
/// workflow engine so an untargeted definition selects the same files there.
fn intrinsic_include(language: CodemodLang, include: Option<&[String]>) -> Option<Vec<String>> {
    match include {
        Some(patterns) => Some(patterns.to_vec()),
        None => {
            let derived: Vec<String> = get_extensions_for_language(language)
                .into_iter()
                .map(|extension| format!("**/*{extension}"))
                .collect();
            (!derived.is_empty()).then_some(derived)
        }
    }
}

fn build_overrides(
    root: &Path,
    include: Option<&[String]>,
    exclude: Option<&[String]>,
) -> Result<Option<(Override, bool)>, String> {
    if include.is_none() && exclude.is_none() {
        return Ok(None);
    }
    let mut builder = OverrideBuilder::new(root);
    for pattern in include.unwrap_or_default() {
        builder
            .add(pattern)
            .map_err(|error| format!("invalid include glob '{pattern}': {error}"))?;
    }
    for pattern in exclude.unwrap_or_default() {
        builder
            .add(&format!("!{pattern}"))
            .map_err(|error| format!("invalid exclude glob '{pattern}': {error}"))?;
    }
    builder
        .build()
        .map(|matcher| Some((matcher, include.is_some())))
        .map_err(|error| format!("invalid JSSG globs: {error}"))
}

/// Enumerate files under `root` accepted by both the definition and the
/// invocation filters, in sorted order. Uses the walker settings shared with
/// the workflow engine; entries that cannot be read are skipped as there.
fn collect_files(
    root: &Path,
    intrinsic: Option<&(Override, bool)>,
    invocation: Option<&(Override, bool)>,
) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = codemod_walk_builder(root)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .filter(|path| matches_overrides(path, intrinsic) && matches_overrides(path, invocation))
        .collect();
    files.sort();
    files
}

fn matches_overrides(path: &Path, overrides: Option<&(Override, bool)>) -> bool {
    let Some((matcher, has_include)) = overrides else {
        return true;
    };
    let matched = matcher.matched(path, false);
    !matched.is_ignore() && (!has_include || matched.is_whitelist())
}

fn build_semantic_provider(
    config: Option<&SemanticAnalysis>,
    target_root: &Path,
) -> Result<Option<Arc<dyn SemanticProvider>>, String> {
    let Some(config) = config else {
        return Ok(None);
    };
    let (mode, root) = match config {
        SemanticAnalysis::Mode(mode) => (*mode, None),
        SemanticAnalysis::Detailed(details) => (details.mode, details.root.as_deref()),
    };
    match mode {
        SemanticMode::File => {
            if root.is_some() {
                return Err("semanticAnalysis.root requires workspace mode".to_string());
            }
            Ok(Some(Arc::new(LazySemanticProvider::file_scope())))
        }
        SemanticMode::Workspace => {
            let root = match root {
                Some(root) => {
                    validate_relative_path(root, "semanticAnalysis.root")?;
                    target_root.join(root).canonicalize().map_err(|error| {
                        format!("failed to resolve semanticAnalysis.root '{root}': {error}")
                    })?
                }
                None => target_root.to_path_buf(),
            };
            Ok(Some(Arc::new(LazySemanticProvider::workspace_scope(root))))
        }
    }
}

fn apply_output(
    target_root: &Path,
    source: &Path,
    output: &CodemodOutput,
    wrote: &mut bool,
) -> Result<Vec<Written>, JssgFailure> {
    let mut written = Vec::new();
    if let Some(entry) = apply_file_result(target_root, source, &output.primary, wrote)? {
        written.push(entry);
    }
    for secondary in &output.secondary {
        if let Some(entry) =
            apply_file_result(target_root, &secondary.path, &secondary.result, wrote)?
        {
            written.push(entry);
        }
    }
    Ok(written)
}

fn apply_file_result(
    target_root: &Path,
    source: &Path,
    result: &ExecutionResult,
    wrote: &mut bool,
) -> Result<Option<Written>, JssgFailure> {
    let ExecutionResult::Modified(modified) = result else {
        return Ok(None);
    };
    let fail = |message: String| JssgFailure {
        message,
        after_write: *wrote,
    };
    let requested = modified.rename_to.as_deref().unwrap_or(source);
    if requested
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(fail(format!(
            "JSSG output '{}' contains parent traversal",
            requested.display()
        )));
    }
    let destination = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        target_root.join(requested)
    };
    if !destination.starts_with(target_root) {
        return Err(fail(format!(
            "JSSG output '{}' escapes the target root",
            destination.display()
        )));
    }
    let parent = destination.parent().unwrap_or(target_root);
    let existing_ancestor = parent
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| {
            fail(format!(
                "no existing ancestor for '{}'",
                destination.display()
            ))
        })?;
    let canonical_ancestor = existing_ancestor.canonicalize().map_err(|error| {
        fail(format!(
            "failed to resolve '{}': {error}",
            existing_ancestor.display()
        ))
    })?;
    if !canonical_ancestor.starts_with(target_root) {
        return Err(fail(format!(
            "JSSG output '{}' escapes the target root",
            destination.display()
        )));
    }
    // From here on the filesystem may change, so later errors are `unknown`.
    *wrote = true;
    std::fs::create_dir_all(parent).map_err(|error| JssgFailure {
        message: format!("failed to create '{}': {error}", parent.display()),
        after_write: true,
    })?;
    std::fs::write(&destination, &modified.content).map_err(|error| JssgFailure {
        message: format!("failed to write '{}': {error}", destination.display()),
        after_write: true,
    })?;
    let deferred_deletion =
        (modified.rename_to.is_some() && source != destination).then(|| source.to_path_buf());
    Ok(Some(Written {
        path: destination,
        content: modified.content.clone(),
        deferred_deletion,
    }))
}

/// Convert the runner's success or failure into a structured completion.
pub fn completion_from_result(
    command_id: &str,
    result: butterflow_models::Result<String>,
) -> OperationCompletion {
    match result {
        Ok(stdout) => {
            let mut output = Map::new();
            output.insert("stdout".to_string(), Value::String(stdout));
            OperationCompletion::succeeded(command_id, Value::Object(output))
        }
        Err(Error::ShellCommandFailed { exit_code, output }) => OperationCompletion::not_succeeded(
            command_id,
            CompletionStatus::Failed,
            CompletionError {
                message: format!("Command failed with exit code {exit_code}: {output}"),
                exit_code: Some(exit_code),
                output: Some(output),
            },
        ),
        // Spawn/wait failures: the bridge cannot tell whether side effects happened.
        Err(error) => OperationCompletion::not_succeeded(
            command_id,
            CompletionStatus::Unknown,
            CompletionError {
                message: error.to_string(),
                exit_code: None,
                output: None,
            },
        ),
    }
}
