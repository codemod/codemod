//! One JSSG batch: the transform bundle (verified against its hash and
//! loaded from memory under a virtual module name), the language, the
//! optional static selector, the invocation input, and one optional semantic
//! provider are set up once; every supplied file is then transformed serially
//! from the content the host sent (snapshot semantics: no transform sees
//! another's edits, and the workspace index is built once from the whole
//! supplied set before the first transform). Files the selector does not
//! match are skipped before any JavaScript runtime starts. Edits, renames,
//! and JSON outputs come back as data with every path validated against the
//! canonical target root. Nothing here reads an author file or a repository
//! listing, and nothing writes repository files.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use codemod_sandbox::sandbox::{
    engine::{
        codemod_lang::CodemodLang, execute_codemod_with_loader, selector_from_value,
        selector_matches, CodemodOutput, ExecutionResult, JssgExecutionOptions,
    },
    resolvers::{InMemoryLoader, InMemoryResolver},
};
use language_core::{ProviderMode, SemanticProvider};
use semantic_factory::LazySemanticProvider;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ArtifactRef, BatchFile, Selector, SemanticAnalysis, SemanticMode};

pub struct Batch<'a> {
    pub artifact: &'a ArtifactRef,
    /// The bundled transform source from the request context.
    pub source: Option<&'a str>,
    pub language: &'a str,
    pub selector: Option<&'a Selector>,
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
/// `jssgTransform` and staged `write()` edits, and the structured output. A
/// file the selector skipped has no edits and no output.
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
    let (module, source) = verified_module(batch.artifact, batch.source)?;
    let language: CodemodLang = batch
        .language
        .parse()
        .map_err(|error| format!("invalid JSSG language '{}': {error}", batch.language))?;
    let selector = batch
        .selector
        .map(|selector| {
            serde_json::to_value(selector)
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    selector_from_value(language, value).map_err(|error| error.to_string())
                })
        })
        .transpose()
        .map_err(|error| format!("invalid JSSG selector: {error}"))?;
    let provider = semantic_provider(batch.semantic_analysis, &root)?;
    let params = batch
        .input
        .map(|value| HashMap::from([("input".to_string(), value.clone())]));
    let mut resolver = InMemoryResolver::new();
    resolver.add_module_with_source(format!("./{module}"), module.clone(), source.to_string());
    let resolver = Arc::new(resolver);

    // Every requested path is checked before anything runs.
    let sources = batch
        .files
        .iter()
        .map(|file| resolve_source(&root, &file.path, "file path"))
        .collect::<Result<Vec<_>, _>>()?;
    // The whole selected set is indexed, including files the selector will
    // skip, so a matching transform can resolve definitions in them.
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
        if selector
            .as_ref()
            .is_some_and(|selector| !selector_matches(selector, language, &file.content))
        {
            outcomes.push(FileOutcome {
                path: file.path.clone(),
                edits: vec![],
                output: None,
            });
            continue;
        }
        let output = execute_codemod_with_loader(
            JssgExecutionOptions {
                script_path: Path::new(&module),
                resolver: Arc::clone(&resolver),
                language,
                file_path: source,
                content: &file.content,
                selector_config: None,
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
            },
            InMemoryLoader::new(Arc::clone(&resolver)),
        )
        .await
        .map_err(|error| format!("JSSG failed for '{}': {error}", file.path))?;
        outcomes.push(convert(&root, &file.path, output)?);
    }
    Ok(outcomes)
}

/// The artifact's virtual module name and source, once the source is present
/// and its SHA-256 matches the hash the operation recorded. The name is
/// derived from the definition name so sandbox errors point at the transform.
fn verified_module<'a>(
    artifact: &ArtifactRef,
    source: Option<&'a str>,
) -> Result<(String, &'a str), String> {
    if artifact.name.trim().is_empty() {
        return Err("JSSG transform name must not be empty".to_string());
    }
    let hash = &artifact.hash;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(format!(
            "JSSG transform '{}' hash must be lowercase hex SHA-256",
            artifact.name
        ));
    }
    let source = source.ok_or_else(|| {
        format!(
            "JSSG transform '{}' has no artifact source in the request context",
            artifact.name
        )
    })?;
    let actual = format!("{:x}", Sha256::digest(source.as_bytes()));
    if actual != *hash {
        return Err(format!(
            "JSSG transform '{}' source hashes to {actual}, not the recorded {hash}",
            artifact.name
        ));
    }
    let stem: String = artifact
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok((format!("{stem}.jssg.js"), source))
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

fn canonical_dir(value: Option<&str>, name: &str) -> Result<PathBuf, String> {
    let path = match value.map(Path::new) {
        Some(path) if path.is_absolute() => path,
        _ => return Err(format!("{name} must be an absolute path")),
    };
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
