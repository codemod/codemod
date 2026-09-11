//! One JSSG batch: the script, its module resolver and selector, the
//! language, the invocation input, and one optional semantic provider are set
//! up once; every supplied file is then transformed serially from the content
//! the host sent (snapshot semantics: no transform sees another's edits, and
//! the workspace index is built once from the supplied set before the first
//! transform). Edits, renames, and JSON outputs come back as data with every
//! path validated against the canonical target root. Nothing here reads a
//! repository listing or writes repository files.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use codemod_sandbox::{
    sandbox::{
        engine::{
            codemod_lang::CodemodLang, execute_codemod_with_quickjs, extract_selector_with_quickjs,
            CodemodOutput, ExecutionResult, JssgExecutionOptions, SelectorEngineOptions,
        },
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use language_core::{ProviderMode, SemanticProvider};
use semantic_factory::LazySemanticProvider;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BatchFile, SemanticAnalysis, SemanticMode};

pub struct Batch<'a> {
    pub script: &'a str,
    pub script_root: Option<&'a str>,
    pub language: &'a str,
    pub target_root: Option<&'a str>,
    pub semantic_analysis: Option<&'a SemanticAnalysis>,
    /// Exposed to the transform as `options.params.input`.
    pub input: Option<&'a Value>,
    pub files: &'a [BatchFile],
}

/// One write the sandbox asked for. `path` is the file whose content this is;
/// with `rename_to` the content belongs at that path and `path` goes away.
/// Both are target-root-relative `/`-separated paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edit {
    pub path: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_to: Option<String>,
}

/// What one supplied file's transform produced: its own edit (if modified),
/// `jssgTransform` and staged `write()` edits, and the structured output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOutcome {
    pub path: String,
    pub edits: Vec<Edit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

pub async fn transform_batch(batch: Batch<'_>) -> Result<Vec<FileOutcome>, String> {
    let root = canonical_dir(batch.target_root, "targetRoot")?;
    let script_root = absolute(batch.script_root, "scriptRoot")?;
    validate_relative_path(batch.script, "JSSG script")?;
    let script_path = script_root.join(batch.script);
    let script_path = script_path.canonicalize().map_err(|error| {
        format!(
            "failed to resolve JSSG script '{}': {error}",
            script_path.display()
        )
    })?;
    let language: CodemodLang = batch
        .language
        .parse()
        .map_err(|error| format!("invalid JSSG language '{}': {error}", batch.language))?;
    let script_dir = script_path.parent().unwrap_or(Path::new("."));
    let resolver = Arc::new(
        OxcResolver::new(script_dir.to_path_buf(), find_tsconfig(script_dir))
            .map_err(|error| format!("failed to create JSSG resolver: {error}"))?,
    );
    // The shipped selector engine passes no params, so `getSelector` sees `{}`.
    let selector = extract_selector_with_quickjs(SelectorEngineOptions {
        script_path: &script_path,
        language,
        resolver: Arc::clone(&resolver),
        capabilities: None,
        target_directory: Some(&root),
    })
    .await
    .map_err(|error| format!("failed to load JSSG selector: {error}"))?
    .map(Arc::from);
    let provider = semantic_provider(batch.semantic_analysis, &root)?;
    let params = batch
        .input
        .map(|value| HashMap::from([("input".to_string(), value.clone())]));

    // Every requested path is checked before anything runs.
    let sources = batch
        .files
        .iter()
        .map(|file| resolve_source(&root, &file.path, "file path"))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(provider) = provider
        .as_ref()
        .filter(|provider| provider.mode() == ProviderMode::WorkspaceScope)
    {
        for (file, source) in batch.files.iter().zip(&sources) {
            provider
                .notify_file_processed(source, &file.content)
                .map_err(|error| format!("failed to index '{}': {error}", file.path))?;
        }
    }

    let mut outcomes = Vec::with_capacity(batch.files.len());
    for (file, source) in batch.files.iter().zip(&sources) {
        let output = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &script_path,
            resolver: Arc::clone(&resolver),
            language,
            file_path: source,
            content: &file.content,
            selector_config: selector.clone(),
            params: params.clone(),
            matrix_values: None,
            capabilities: None,
            semantic_provider: provider.clone(),
            metrics_context: None,
            llm_request_handler: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            stage_writes: true,
            target_directory: &root,
        })
        .await
        .map_err(|error| format!("JSSG failed for '{}': {error}", file.path))?;
        outcomes.push(convert(&root, &file.path, output)?);
    }
    Ok(outcomes)
}

fn convert(root: &Path, path: &str, output: CodemodOutput) -> Result<FileOutcome, String> {
    let mut edits = Vec::new();
    push_edit(&mut edits, root, path.to_string(), output.primary)?;
    for change in output.secondary {
        let path = normalize_output(root, &change.path, "secondary path")?;
        push_edit(&mut edits, root, path, change.result)?;
    }
    Ok(FileOutcome {
        path: path.to_string(),
        edits,
        output: output.output,
    })
}

fn push_edit(
    edits: &mut Vec<Edit>,
    root: &Path,
    path: String,
    result: ExecutionResult,
) -> Result<(), String> {
    if let ExecutionResult::Modified(modified) = result {
        let rename_to = modified
            .rename_to
            .as_deref()
            .map(|target| normalize_output(root, target, "rename target"))
            .transpose()?;
        edits.push(Edit {
            path,
            content: modified.content,
            rename_to,
        });
    }
    Ok(())
}

fn semantic_provider(
    config: Option<&SemanticAnalysis>,
    root: &Path,
) -> Result<Option<Arc<dyn SemanticProvider>>, String> {
    let (mode, sub_root) = match config {
        None => return Ok(None),
        Some(SemanticAnalysis::Mode(mode)) => (*mode, None),
        Some(SemanticAnalysis::Detailed(details)) => (details.mode, details.root.as_deref()),
    };
    Ok(Some(match (mode, sub_root) {
        (SemanticMode::File, None) => Arc::new(LazySemanticProvider::file_scope()),
        (SemanticMode::File, Some(_)) => {
            return Err("semanticAnalysis.root requires workspace mode".to_string())
        }
        (SemanticMode::Workspace, sub_root) => {
            let workspace = match sub_root {
                Some(sub_root) => resolve_source(root, sub_root, "semanticAnalysis.root")?,
                None => root.to_path_buf(),
            };
            if !workspace.is_dir() {
                return Err(format!(
                    "failed to resolve semanticAnalysis.root '{}': not a directory",
                    workspace.display()
                ));
            }
            Arc::new(LazySemanticProvider::workspace_scope(workspace))
        }
    }))
}

// Path containment. Paths the host sends must be safe relative paths whose
// nearest existing ancestor resolves inside the canonical root, so a
// symlinked file or directory cannot point outside it. Paths the sandbox
// produces may be absolute or relative and are normalized to root-relative
// `/`-separated form after the same check. TypeScript repeats its own checks
// before writing; neither side trusts the other's validation.

fn absolute(value: Option<&str>, name: &str) -> Result<PathBuf, String> {
    match value.map(Path::new) {
        Some(path) if path.is_absolute() => Ok(path.to_path_buf()),
        _ => Err(format!("{name} must be an absolute path")),
    }
}

fn canonical_dir(value: Option<&str>, name: &str) -> Result<PathBuf, String> {
    let path = absolute(value, name)?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve {name} '{}': {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{name} '{}' is not a directory", path.display()));
    }
    Ok(canonical)
}

/// Same rules as `isSafeRelativePath` in `packages/orchestration/src/paths.ts`:
/// non-empty, not absolute on any platform (`/x`, `\x`, `C:\x`), and no `..`
/// segment. A `..` inside a name such as `foo..bar` is allowed.
pub fn validate_relative_path(value: &str, name: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    let bytes = trimmed.as_bytes();
    let absolute = Path::new(trimmed).has_root()
        || trimmed.starts_with(['/', '\\'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');
    if absolute || trimmed.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(format!("{name} must be a safe relative path"));
    }
    Ok(())
}

fn resolve_source(root: &Path, relative: &str, name: &str) -> Result<PathBuf, String> {
    validate_relative_path(relative, name)?;
    contain(root, &root.join(relative), name)
}

fn normalize_output(root: &Path, requested: &Path, name: &str) -> Result<String, String> {
    if requested.as_os_str().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    let real = contain(root, &root.join(requested), name)?;
    let mut parts = Vec::new();
    for component in real.strip_prefix(root).unwrap_or(&real).components() {
        match component {
            Component::Normal(part) => match part.to_str() {
                Some(text) if !text.contains('\\') => parts.push(text.to_string()),
                _ => {
                    return Err(format!(
                        "{name} '{}' is not a plain path",
                        requested.display()
                    ))
                }
            },
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "{name} '{}' is not a plain path",
                    requested.display()
                ))
            }
        }
    }
    if parts.is_empty() {
        return Err(format!("{name} resolves to the target root itself"));
    }
    Ok(parts.join("/"))
}

/// The real path of `candidate` once symlinks in its nearest existing
/// ancestor are resolved, provided it stays inside `root`.
fn contain(root: &Path, candidate: &Path, name: &str) -> Result<PathBuf, String> {
    let escape = || format!("{name} '{}' escapes the target root", candidate.display());
    if candidate
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(escape());
    }
    let existing = candidate
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(escape)?;
    let real = existing
        .canonicalize()
        .map_err(|error| format!("failed to resolve {name} '{}': {error}", existing.display()))?
        .join(candidate.strip_prefix(existing).map_err(|_| escape())?);
    if real.starts_with(root) {
        Ok(real)
    } else {
        Err(escape())
    }
}
