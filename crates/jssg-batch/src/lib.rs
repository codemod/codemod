//! The JSSG batch rules every TypeScript orchestration runtime shares, in one
//! place:
//!
//! - the batch's protocol data ([`ArtifactRef`], [`Selector`],
//!   [`SemanticAnalysis`], [`BatchFile`]) and its results ([`Edit`],
//!   [`FileOutcome`]);
//! - [`BatchPlan`]: everything a batch needs before its first transform runs
//!   (hash-verified artifact, canonical target root, language, compiled
//!   selector, params, semantic scope and its provider, every requested path
//!   contained in the root), the workspace index, the static selector skip,
//!   parsing a file into a [`CoreRoot`] (answering semantic queries), and
//!   turning a transform's [`CodemodOutput`] into a validated
//!   [`FileOutcome`] (`/`-separated, contained secondary paths and rename
//!   targets).
//!
//! Users: the orchestration QuickJS host
//! (`crates/orchestration-quickjs-host`, one context for the whole run) and
//! the napi addon of the `node` runtime (`crates/jssg-napi`). Each binds the
//! transform call itself; none re-implements these rules.
//!
//! Binding-agnostic: no JavaScript engine types here. Turning a transform's
//! JavaScript return value into a [`CodemodOutput`] is the binding's job, via
//! the shared [`classify_transform_result`] of `codemod-sandbox`.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use ast_grep_config::RuleConfig;
use ast_grep_core::{matcher::MatcherExt, AstGrep};
use codemod_sandbox::sandbox::engine::{
    codemod_lang::CodemodLang, selector_from_value, selector_matches, CodemodOutput,
    ExecutionResult,
};
use codemod_sandbox::sg_core::{CoreRoot, RootOptions, SemanticSeam};
use language_core::{ProviderMode, SemanticProvider};
use semantic_factory::LazySemanticProvider;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use codemod_sandbox::sandbox::engine::{
    classify_transform_result, ModificationCheck, ReturnedContent, MISSING_OUTPUT_MESSAGE,
};

// ---------------------------------------------------------------------------
// Protocol data (mirrors `packages/orchestration/src/core/protocol.ts`)
// ---------------------------------------------------------------------------

/// Build-time identity of one transform artifact: the definition's `name`
/// and the lowercase hex SHA-256 of the bundled source. No path, so the
/// recorded command is the same on every checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub name: String,
    pub hash: String,
}

/// The ast-grep rule data a definition declares as its selector. Only the
/// matching fields are accepted; `id` and `language` are set when compiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    pub rule: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utils: Option<Value>,
}

/// `"file"`, `"workspace"`, or `{ mode, root? }` where `root` is a safe
/// relative path beneath the target root and requires workspace mode.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchFile {
    /// Safe relative path beneath the target root.
    pub path: String,
    pub content: String,
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

impl FileOutcome {
    /// A file the selector skipped: no edits, no output.
    pub fn skipped(path: &str) -> Self {
        FileOutcome {
            path: path.to_string(),
            edits: vec![],
            output: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Batch planning
// ---------------------------------------------------------------------------

/// One `jssg` operation's batch as the request carries it.
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

/// The JSON form of a [`Batch`] without its files, for runners whose file
/// contents stay on the JavaScript side (`jssgBegin` of the QuickJS host,
/// `JssgBatch` of the napi addon); the paths travel separately.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchConfig {
    pub transform: ArtifactRef,
    #[serde(default)]
    pub source: Option<String>,
    pub language: String,
    #[serde(default)]
    pub selector: Option<Selector>,
    #[serde(default)]
    pub semantic_analysis: Option<SemanticAnalysis>,
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub target_root: Option<String>,
}

impl BatchConfig {
    pub fn batch(&self) -> Batch<'_> {
        Batch {
            artifact: &self.transform,
            source: self.source.as_deref(),
            language: &self.language,
            selector: self.selector.as_ref(),
            target_root: self.target_root.as_deref(),
            semantic_analysis: self.semantic_analysis.as_ref(),
            input: self.input.as_ref(),
            files: &[],
        }
    }
}

/// The semantic analysis a batch asked for, validated against the root;
/// [`BatchPlan`] builds its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticScope {
    File,
    /// The contained, existing workspace directory to index.
    Workspace(PathBuf),
}

/// When a batch's file paths are resolved inside the root (a `stat` and a
/// `realpath` each). Every path is checked syntactically (a safe relative
/// path) while planning either way, and nothing is ever written unless every
/// file of the batch passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    /// All of them while planning, before anything runs ([`BatchPlan::new`],
    /// for a runner that has every file's content up front).
    Upfront,
    /// Each in [`BatchPlan::parse`], before that file can reach a transform:
    /// the in-process runtimes parse on worker threads a chunk ahead, so the
    /// filesystem calls overlap the transforms instead of preceding them.
    PerFile,
}

/// Everything a batch needs before its first transform runs, shared by every
/// batch runner: the canonical target root, the hash-verified artifact, the
/// language, the compiled selector, the transform params, the semantic
/// scope, and every file path checked (resolved inside the root up front or
/// per file, see [`Containment`]). Building it fails before anything runs.
pub struct BatchPlan {
    pub root: PathBuf,
    /// Virtual module name derived from the transform name (`<name>.jssg.js`).
    pub module: String,
    pub language: CodemodLang,
    selector: Option<RuleConfig<CodemodLang>>,
    /// `{ input }` when the operation has an input.
    pub params: Option<HashMap<String, Value>>,
    pub semantic: Option<SemanticScope>,
    /// The batch's one semantic provider (one per batch, shared by every
    /// file's root), built lazily on first use; `None` without
    /// `semanticAnalysis`.
    pub provider: Option<Arc<dyn SemanticProvider>>,
    /// The requested (root-relative) path of every file, in request order.
    pub paths: Vec<String>,
    /// Absolute, contained path of every requested file, in request order
    /// ([`Containment::Upfront`]; empty with [`Containment::PerFile`], see
    /// [`BatchPlan::source`]).
    pub sources: Vec<PathBuf>,
}

impl BatchPlan {
    /// Plan `batch` for its own `files`, every path resolved up front.
    pub fn new(batch: &Batch<'_>) -> Result<Self, String> {
        Self::for_paths(
            batch,
            batch.files.iter().map(|file| file.path.as_str()),
            Containment::Upfront,
        )
    }

    /// Plan `batch` for `paths` (a runner that keeps file contents on its
    /// own side passes `files: &[]` and the paths here).
    pub fn for_paths<'p>(
        batch: &Batch<'_>,
        paths: impl IntoIterator<Item = &'p str>,
        containment: Containment,
    ) -> Result<Self, String> {
        let root = canonical_dir(batch.target_root, "targetRoot")?;
        let module = verified_module(batch.artifact, batch.source)?;
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
        let semantic = semantic_scope(batch.semantic_analysis, &root)?;
        let provider = semantic.as_ref().map(semantic_provider);
        let params = batch
            .input
            .map(|value| HashMap::from([("input".to_string(), value.clone())]));
        // Every requested path is checked before anything runs.
        let paths: Vec<String> = paths.into_iter().map(str::to_string).collect();
        let sources = match containment {
            Containment::Upfront => paths
                .iter()
                .map(|path| resolve_source(&root, path, "file path"))
                .collect::<Result<Vec<_>, _>>()?,
            Containment::PerFile => {
                for path in &paths {
                    validate_relative_path(path, "file path")?;
                }
                Vec::new()
            }
        };
        Ok(Self {
            root,
            module,
            language,
            selector,
            params,
            semantic,
            provider,
            paths,
            sources,
        })
    }

    /// The display name of the language (`options.language`).
    pub fn language_name(&self) -> String {
        self.language.to_string()
    }

    /// The requested path of file `index`.
    pub fn path(&self, index: usize) -> Result<&str, String> {
        self.paths
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("no file {index} in the batch"))
    }

    /// The absolute, contained path of file `index` (resolved now with
    /// [`Containment::PerFile`]).
    pub fn source(&self, index: usize) -> Result<PathBuf, String> {
        match self.sources.get(index) {
            Some(source) => Ok(source.clone()),
            None if self.sources.is_empty() => {
                resolve_source(&self.root, self.path(index)?, "file path")
            }
            None => Err(format!("no file {index} in the batch")),
        }
    }

    /// Whether the static selector rules `content` out (the transform is not
    /// called and the file's outcome is [`FileOutcome::skipped`]).
    pub fn skips(&self, content: &str) -> bool {
        self.selector
            .as_ref()
            .is_some_and(|selector| !selector_matches(selector, self.language, content))
    }

    /// File `index` parsed from `content` (the snapshot the host read), with
    /// its canonical path as `filename()` and the root as the target
    /// directory, or `None` when the static selector matches nothing in it.
    /// Contains the path first ([`BatchPlan::source`]). Parses once; safe to
    /// call from any thread. The root answers semantic queries through the
    /// batch's provider, if any.
    pub fn parse(&self, index: usize, content: &str) -> Result<Option<CoreRoot>, String> {
        let source = self.source(index)?;
        let filename = source.to_string_lossy().into_owned();
        let root = CoreRoot::from_ast_grep(
            AstGrep::new(content, self.language),
            RootOptions {
                filename: Some(filename.clone()),
                target_directory: Some(&self.root),
                current_file_path: Some(filename),
                semantic: SemanticSeam::from_provider(self.provider.clone()),
            },
        );
        let skipped = self.selector.as_ref().is_some_and(|selector| {
            !root
                .ast_grep()
                .root()
                .dfs()
                .any(|node| selector.matcher.match_node(node).is_some())
        });
        Ok((!skipped).then_some(root))
    }

    /// Whether the batch indexes a workspace: every file must then go
    /// through [`BatchPlan::index`] before the first transform runs.
    pub fn indexes_workspace(&self) -> bool {
        self.provider
            .as_ref()
            .is_some_and(|provider| provider.mode() == ProviderMode::WorkspaceScope)
    }

    /// Add file `index` with `content` (the snapshot the host read) to the
    /// workspace index; a no-op unless [`BatchPlan::indexes_workspace`].
    /// Every file of the batch is indexed, including ones the selector will
    /// skip, so a matching transform can resolve definitions in them. Safe to
    /// call from any thread.
    pub fn index(&self, index: usize, content: &str) -> Result<(), String> {
        if !self.indexes_workspace() {
            return Ok(());
        }
        let provider = self.provider.as_ref().expect("workspace provider");
        let source = self.source(index)?;
        // A file the semantic parser cannot read (JSX in a `.js` file, for
        // one) is left out of the index, as the YAML engine's pre-index does;
        // the transform still runs on it.
        let _ = provider.notify_file_processed(&source, content);
        Ok(())
    }

    /// File `index`'s transform output as a validated [`FileOutcome`]; an
    /// error (the binding's, or a path that fails validation) names the file:
    /// `JSSG failed for '<path>': <error>`.
    pub fn finish(
        &self,
        index: usize,
        output: Result<CodemodOutput, String>,
    ) -> Result<FileOutcome, String> {
        let path = self.path(index)?;
        output
            .and_then(|output| convert(&self.root, path, output))
            .map_err(|error| format!("JSSG failed for '{path}': {error}"))
    }
}

/// The artifact's virtual module name, once the source is present and its
/// SHA-256 matches the hash the operation recorded. The name is derived from
/// the definition name so errors point at the transform.
fn verified_module(artifact: &ArtifactRef, source: Option<&str>) -> Result<String, String> {
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
    Ok(format!("{stem}.jssg.js"))
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

fn semantic_provider(scope: &SemanticScope) -> Arc<dyn SemanticProvider> {
    match scope {
        SemanticScope::File => Arc::new(LazySemanticProvider::file_scope()),
        SemanticScope::Workspace(root) => {
            Arc::new(LazySemanticProvider::workspace_scope(root.clone()))
        }
    }
}

fn semantic_scope(
    config: Option<&SemanticAnalysis>,
    root: &Path,
) -> Result<Option<SemanticScope>, String> {
    let (mode, sub_root) = match config {
        None => return Ok(None),
        Some(SemanticAnalysis::Mode(mode)) => (*mode, None),
        Some(SemanticAnalysis::Detailed(details)) => (details.mode, details.root.as_deref()),
    };
    Ok(Some(match (mode, sub_root) {
        (SemanticMode::File, None) => SemanticScope::File,
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
            SemanticScope::Workspace(workspace)
        }
    }))
}

// ---------------------------------------------------------------------------
// Path containment
// ---------------------------------------------------------------------------
//
// Paths the host sends must be safe relative paths whose nearest existing
// ancestor resolves inside the canonical root, so a symlinked file or
// directory cannot point outside it. Paths the sandbox produces may be
// absolute or relative and are normalized to root-relative `/`-separated
// form after the same check. TypeScript repeats its own checks before
// writing; neither side trusts the other's validation.

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

/// Same rules as `isSafeRelativePath` in `packages/orchestration/src/core/paths.ts`:
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
    // The common case, an existing path, costs one `realpath`; otherwise the
    // nearest ancestor that exists (`Path::exists`: any error walks on).
    let (existing, canonical) = match candidate.canonicalize() {
        Ok(canonical) => (candidate, canonical),
        Err(_) => {
            let existing = candidate
                .ancestors()
                .skip(1)
                .find(|ancestor| ancestor.exists())
                .ok_or_else(escape)?;
            let canonical = existing.canonicalize().map_err(|error| {
                format!("failed to resolve {name} '{}': {error}", existing.display())
            })?;
            (existing, canonical)
        }
    };
    let rest = candidate.strip_prefix(existing).map_err(|_| escape())?;
    // `join("")` would append a trailing separator to an existing path.
    let real = if rest.as_os_str().is_empty() {
        canonical
    } else {
        canonical.join(rest)
    };
    if real.starts_with(root) {
        Ok(real)
    } else {
        Err(escape())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemod_sandbox::sandbox::engine::{FileChange, ModifiedResult};

    fn plan(dir: &Path, paths: &[&str], selector: Option<Selector>) -> BatchPlan {
        let source = "export default () => null;";
        let artifact = ArtifactRef {
            name: "t x".to_string(),
            hash: format!("{:x}", Sha256::digest(source.as_bytes())),
        };
        let root = dir.to_string_lossy().into_owned();
        let batch = Batch {
            artifact: &artifact,
            source: Some(source),
            language: "typescript",
            selector: selector.as_ref(),
            target_root: Some(&root),
            semantic_analysis: None,
            input: None,
            files: &[],
        };
        BatchPlan::for_paths(&batch, paths.iter().copied(), Containment::Upfront).expect("plan")
    }

    fn modified(content: &str, rename_to: Option<&str>) -> ExecutionResult {
        ExecutionResult::Modified(ModifiedResult {
            content: content.to_string(),
            rename_to: rename_to.map(PathBuf::from),
        })
    }

    #[test]
    fn plans_parses_and_skips_with_the_selector() {
        let dir = tempfile::tempdir().unwrap();
        let selector = Selector {
            rule: serde_json::json!({ "pattern": "oldApi($A)" }),
            constraints: None,
            utils: None,
        };
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.ts"), "oldApi(1);").unwrap();
        let plan = plan(dir.path(), &["a.ts", "sub/b.ts"], Some(selector));
        assert_eq!(plan.module, "t_x.jssg.js");
        assert_eq!(plan.language_name(), "TypeScript");
        let root = plan.parse(1, "oldApi(1);").unwrap().expect("matches");
        // Existing and missing files alike: the canonical path, no trailing separator.
        let canonical = dir.path().canonicalize().unwrap();
        assert_eq!(
            root.filename(),
            Some(canonical.join("sub/b.ts").to_string_lossy().as_ref())
        );
        assert_eq!(root.relative_filename(), Some("sub/b.ts"));
        assert_eq!(plan.sources[0], canonical.join("a.ts"));
        assert!(root.rename_to().is_none());
        assert_eq!(root.source(), "oldApi(1);");
        assert!(plan.parse(0, "plain();").unwrap().is_none());
        assert!(plan.skips("plain();"));
        assert!(!plan.skips("oldApi(2);"));
        assert!(plan.parse(2, "").is_err());
    }

    #[test]
    fn refuses_paths_outside_the_root_before_anything_runs() {
        let dir = tempfile::tempdir().unwrap();
        let source = "x";
        let artifact = ArtifactRef {
            name: "t".to_string(),
            hash: format!("{:x}", Sha256::digest(source.as_bytes())),
        };
        let root = dir.path().to_string_lossy().into_owned();
        let batch = Batch {
            artifact: &artifact,
            source: Some(source),
            language: "typescript",
            selector: None,
            target_root: Some(&root),
            semantic_analysis: Some(&SemanticAnalysis::Mode(SemanticMode::File)),
            input: None,
            files: &[],
        };
        for path in ["../x.ts", "/etc/passwd", "a/../../b.ts", ""] {
            let error = BatchPlan::for_paths(&batch, [path], Containment::Upfront)
                .err()
                .expect(path);
            assert!(error.contains("file path"), "{path}: {error}");
        }
    }

    #[test]
    fn semantic_analysis_resolves_through_the_batch_provider() {
        use codemod_sandbox::sg_core::DefinitionOptions;
        let dir = tempfile::tempdir().unwrap();
        let main = "import { add } from \"./utils\";\nconst result = add(1, 2);\n";
        let utils = "export function add(a: number, b: number): number {\n  return a + b;\n}\n";
        std::fs::write(dir.path().join("main.ts"), main).unwrap();
        std::fs::write(dir.path().join("utils.ts"), utils).unwrap();
        let source = "x";
        let artifact = ArtifactRef {
            name: "t".to_string(),
            hash: format!("{:x}", Sha256::digest(source.as_bytes())),
        };
        let root = dir.path().to_string_lossy().into_owned();
        let definition_of_add = |mode: SemanticMode, index: bool| {
            let analysis = SemanticAnalysis::Mode(mode);
            let batch = Batch {
                artifact: &artifact,
                source: Some(source),
                language: "typescript",
                selector: None,
                target_root: Some(&root),
                semantic_analysis: Some(&analysis),
                input: None,
                files: &[],
            };
            let plan = BatchPlan::for_paths(&batch, ["main.ts", "utils.ts"], Containment::PerFile)
                .unwrap();
            assert!(plan.provider.is_some());
            assert_eq!(plan.indexes_workspace(), mode == SemanticMode::Workspace);
            if index {
                plan.index(0, main).unwrap();
                plan.index(1, utils).unwrap();
            }
            let parsed = plan.parse(0, main).unwrap().unwrap();
            let pattern = ast_grep_core::Pattern::new("add($A, $B)", plan.language);
            let call = parsed.root().find_all(&pattern).remove(0);
            let callee = call.children().remove(0);
            assert_eq!(callee.text(), "add");
            // Follow the local import binding to the export, as the TS
            // fixture does: at most three hops.
            let mut definition = callee
                .semantic()
                .unwrap()
                .expect("provider attached")
                .definition(DefinitionOptions::default())
                .unwrap();
            for _ in 0..3 {
                match &definition {
                    Some(found) if found.root.relative_filename() == Some("main.ts") => {
                        let next = found
                            .node
                            .semantic()
                            .unwrap()
                            .expect("provider inherited")
                            .definition(DefinitionOptions::default())
                            .unwrap();
                        match next {
                            Some(next)
                                if next.node.text() != found.node.text()
                                    || next.root.relative_filename() != Some("main.ts") =>
                            {
                                definition = Some(next)
                            }
                            _ => break,
                        }
                    }
                    _ => break,
                }
            }
            definition.map(|definition| definition.root.relative_filename().map(str::to_string))
        };
        assert_eq!(
            definition_of_add(SemanticMode::Workspace, true),
            Some(Some("utils.ts".to_string()))
        );
        assert_eq!(
            definition_of_add(SemanticMode::File, false),
            Some(Some("main.ts".to_string()))
        );
    }

    #[test]
    fn workspace_indexing_skips_a_file_the_semantic_parser_rejects() {
        let dir = tempfile::tempdir().unwrap();
        let jsx_in_js = "export const A = () => <div>{x}</div>;\n";
        std::fs::write(dir.path().join("a.js"), jsx_in_js).unwrap();
        let source = "x";
        let artifact = ArtifactRef {
            name: "t".to_string(),
            hash: format!("{:x}", Sha256::digest(source.as_bytes())),
        };
        let root = dir.path().to_string_lossy().into_owned();
        let analysis = SemanticAnalysis::Mode(SemanticMode::Workspace);
        let batch = Batch {
            artifact: &artifact,
            source: Some(source),
            language: "typescript",
            selector: None,
            target_root: Some(&root),
            semantic_analysis: Some(&analysis),
            input: None,
            files: &[],
        };
        let plan = BatchPlan::for_paths(&batch, ["a.js"], Containment::PerFile).unwrap();
        assert_eq!(plan.index(0, jsx_in_js), Ok(()));
    }

    #[test]
    fn contains_per_file_when_asked_after_the_syntactic_check() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let source = "x";
        let artifact = ArtifactRef {
            name: "t".to_string(),
            hash: format!("{:x}", Sha256::digest(source.as_bytes())),
        };
        let root = dir.path().to_string_lossy().into_owned();
        let batch = Batch {
            artifact: &artifact,
            source: Some(source),
            language: "typescript",
            selector: None,
            target_root: Some(&root),
            semantic_analysis: None,
            input: None,
            files: &[],
        };
        let plan_for = |paths: &[&str]| {
            BatchPlan::for_paths(&batch, paths.iter().copied(), Containment::PerFile)
        };
        // Syntax is still checked while planning.
        let error = plan_for(&["a.ts", "../x.ts"]).err().unwrap();
        assert_eq!(error, "file path must be a safe relative path");
        let plan = plan_for(&["a.ts", "link/x.ts"]).unwrap();
        assert!(plan.sources.is_empty());
        assert_eq!(plan.source(0).unwrap(), canonical.join("a.ts"));
        assert!(plan.parse(0, "a;").unwrap().is_some());
        assert!(plan.source(2).is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
            let error = plan.parse(1, "x;").err().unwrap();
            assert!(error.contains("escapes the target root"), "{error}");
        }
    }

    #[test]
    fn resolves_large_batches_in_order_with_the_earliest_error() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let mut paths: Vec<String> = (0..1000).map(|i| format!("d{}/f{i}.ts", i % 7)).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let plan = plan(dir.path(), &refs, None);
        assert_eq!(plan.sources.len(), 1000);
        for (i, source) in plan.sources.iter().enumerate() {
            assert_eq!(*source, canonical.join(&paths[i]));
        }
        paths[900] = String::new();
        paths[400] = "/early.ts".to_string();
        let source = "x";
        let artifact = ArtifactRef {
            name: "t".to_string(),
            hash: format!("{:x}", Sha256::digest(source.as_bytes())),
        };
        let root = dir.path().to_string_lossy().into_owned();
        let batch = Batch {
            artifact: &artifact,
            source: Some(source),
            language: "typescript",
            selector: None,
            target_root: Some(&root),
            semantic_analysis: None,
            input: None,
            files: &[],
        };
        let error = BatchPlan::for_paths(
            &batch,
            paths.iter().map(String::as_str),
            Containment::Upfront,
        )
        .err()
        .unwrap();
        assert_eq!(error, "file path must be a safe relative path");
        paths[400] = "d0/ok.ts".to_string();
        let error = BatchPlan::for_paths(
            &batch,
            paths.iter().map(String::as_str),
            Containment::Upfront,
        )
        .err()
        .unwrap();
        assert_eq!(error, "file path must not be empty");
    }

    #[test]
    fn converts_outputs_and_rejects_escaping_paths() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan(dir.path(), &["a.ts"], None);
        let root = plan.root.clone();
        let outcome = plan
            .finish(
                0,
                Ok(CodemodOutput {
                    primary: modified("new", Some(&root.join("b.ts").to_string_lossy())),
                    secondary: vec![
                        FileChange {
                            path: root.join("sub/c.ts"),
                            result: modified("c", None),
                        },
                        FileChange {
                            path: root.join("same.ts"),
                            result: ExecutionResult::Unmodified,
                        },
                    ],
                    output: Some(serde_json::json!({ "n": 1 })),
                }),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&outcome).unwrap(),
            serde_json::json!({
                "path": "a.ts",
                "edits": [
                    { "path": "a.ts", "content": "new", "renameTo": "b.ts" },
                    { "path": "sub/c.ts", "content": "c" },
                ],
                "output": { "n": 1 },
            })
        );
        let escaping = |result: ExecutionResult, secondary: Vec<FileChange>| {
            plan.finish(
                0,
                Ok(CodemodOutput {
                    primary: result,
                    secondary,
                    output: None,
                }),
            )
            .unwrap_err()
        };
        let error = escaping(modified("x", Some("/elsewhere/x.ts")), vec![]);
        assert!(
            error.starts_with("JSSG failed for 'a.ts': rename target")
                && error.ends_with("escapes the target root"),
            "{error}"
        );
        let error = escaping(
            ExecutionResult::Unmodified,
            vec![FileChange {
                path: root.join("../outside.ts"),
                result: ExecutionResult::Unmodified,
            }],
        );
        assert!(error.contains("secondary path"), "{error}");
        let error = plan.finish(0, Err("boom".to_string())).unwrap_err();
        assert_eq!(error, "JSSG failed for 'a.ts': boom");
    }
}
