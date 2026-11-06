//! napi-rs facade over the binding-agnostic JSSG AST core
//! ([`codemod_sandbox::sg_core`]).
//!
//! Two surfaces live in this addon:
//!
//! * The transform-facing `codemod:ast-grep` API: the `SgRoot` / `SgNode`
//!   classes, `parse`, `parseAsync`, `parseFile`, and `kind`, with the same
//!   behaviour and error strings as the rquickjs facade
//!   (`crates/codemod-sandbox/src/ast_grep/sg_node.rs`). `jssgTransform` is
//!   implemented in JavaScript by `packages/jssg-napi` on top of the engine
//!   helpers below.
//! * Engine helpers used by the orchestration `node` runtime's in-process
//!   JSSG batches: the `JssgBatch` class (see [`batch`]: the bridge's own
//!   batch rules from `crates/jssg-batch`, parsing off the JS thread, result
//!   conversion), and the per-call execution context that stages
//!   `write()` / `jssgTransform()` edits instead of touching the disk
//!   (`engine*` exports).
//!
//! Every method is a thin adapter: convert arguments, call the core, convert
//! the result, map [`CoreError`] to a JS `Error` / `TypeError`. Nodes are not
//! `Send` and are only created and used on the JS thread; roots are
//! `Send + Sync` and are parsed on worker threads by [`JssgBatch::parse`].
//!
//! Semantic analysis (`definition()` / `references()` / `typeInfo()`) goes
//! through the core's semantic seam. Roots made here have no provider, so
//! those calls behave as in a JSSG run without semantic analysis (`null` /
//! `[]`); the orchestration runtime refuses operations that request it before
//! anything runs.

#![deny(clippy::all)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use codemod_sandbox::sandbox::engine::{ExecutionResult, FileChange, ModifiedResult};
use codemod_sandbox::sg_core::{
    check_within_target, compile_matcher, CoreError, CoreErrorKind, CoreLang, CoreMatcher,
    CoreNode, CoreRoot, DefinitionOptions, Edit, MatcherInput,
};
use codemod_sandbox::CodemodLang;
use napi::bindgen_prelude::*;
use napi_derive::napi;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Throw a JS `TypeError` and return the error that tells napi-rs an
/// exception is already pending.
fn type_error(env: &Env, message: &str) -> Error {
    let _ = env.throw_type_error(message, None);
    Error::new(Status::PendingException, message.to_string())
}

fn core_error(env: &Env, error: CoreError) -> Error {
    match error.kind {
        CoreErrorKind::TypeError => type_error(env, &error.message),
        CoreErrorKind::Error => Error::from_reason(error.message),
    }
}

// ---------------------------------------------------------------------------
// Per-file execution context (engine)
// ---------------------------------------------------------------------------

mod batch;

pub use batch::{JssgBatch, StagedChanges};

struct ExecutionContext {
    target_dir: PathBuf,
    changes: Vec<FileChange>,
}

thread_local! {
    /// The transform currently running on this (the JS) thread, if any.
    /// Mirrors the QuickJS context userdata `TargetDirectory`,
    /// `StageFileWrites(true)`, and `JssgFileChanges`.
    static CONTEXT: RefCell<Option<ExecutionContext>> = const { RefCell::new(None) };
}

fn with_target<T>(f: impl FnOnce(Option<&Path>) -> T) -> T {
    CONTEXT.with(|context| f(context.borrow().as_ref().map(|c| c.target_dir.as_path())))
}

/// Start staging for one transform call against `targetDir`.
#[napi(js_name = "engineBegin")]
pub fn engine_begin(target_dir: String) {
    CONTEXT.with(|context| {
        *context.borrow_mut() = Some(ExecutionContext {
            target_dir: PathBuf::from(target_dir),
            changes: Vec::new(),
        })
    });
}

/// End the current transform call and return what it staged, in order
/// (opaque; `JssgBatch.finish` consumes it), or `null` when it staged
/// nothing (no object per call in the common case).
#[napi(js_name = "engineEnd")]
pub fn engine_end() -> Option<StagedChanges> {
    let changes = CONTEXT.with(|context| {
        context
            .borrow_mut()
            .take()
            .map(|c| c.changes)
            .unwrap_or_default()
    });
    (!changes.is_empty()).then_some(StagedChanges { changes })
}

/// `jssgTransform()`: stage the inner transform's awaited `result` for the
/// file at `path` parsed as `root`, with the sandbox's result rules.
/// Returns the new content, or `null` when the file is unmodified.
#[napi(js_name = "engineStageResult")]
pub fn engine_stage_result(
    path: String,
    root: &SgRoot,
    result: Unknown<'_>,
) -> Result<Option<String>> {
    let (modified, change) =
        batch::stage_secondary(path, &root.core, result).map_err(Error::from_reason)?;
    CONTEXT.with(|context| match context.borrow_mut().as_mut() {
        Some(context) => {
            context.changes.push(change);
            Ok(modified)
        }
        None => Err(Error::from_reason(
            "jssgTransform() can only be called while a transform runs",
        )),
    })
}

/// `validate_path_within_target` for the running transform; no-op outside one.
#[napi(js_name = "engineCheckWithinTarget")]
pub fn engine_check_within_target(path: String, caller: String) -> Result<()> {
    with_target(|target| match target {
        Some(target) => check_within_target(target, Path::new(&path), &caller)
            .map_err(|e| Error::from_reason(e.message)),
        None => Ok(()),
    })
}

/// The running transform's target directory (`jssgTransform` options).
#[napi(js_name = "engineTargetDir")]
pub fn engine_target_dir() -> Option<String> {
    with_target(|target| target.map(|t| t.to_string_lossy().into_owned()))
}

fn parse_lang(lang: &str) -> std::result::Result<CoreLang, String> {
    CodemodLang::from_str(lang).map_err(|e| format!("invalid JSSG language '{lang}': {e}"))
}

/// The display name of a JSSG language (`options.language`), or an
/// `invalid JSSG language` error.
#[napi(js_name = "engineLanguageName")]
pub fn engine_language_name(lang: String) -> Result<String> {
    parse_lang(&lang)
        .map(|lang| lang.to_string())
        .map_err(Error::from_reason)
}

/// Parse `src` as a file at `filename` (`jssgTransform()` roots).
#[napi(js_name = "engineParseWithPath")]
pub fn engine_parse_with_path(
    lang: String,
    src: String,
    filename: String,
    target_dir: Option<String>,
) -> Result<SgRoot> {
    CoreRoot::parse(
        &lang,
        src,
        Some(filename),
        target_dir.as_deref().map(Path::new),
    )
    .map(SgRoot::from_core)
    .map_err(|e| Error::from_reason(format!("Failed to parse: {e}")))
}

// ---------------------------------------------------------------------------
// `codemod:ast-grep` functions
// ---------------------------------------------------------------------------

#[napi]
pub fn parse(lang: String, src: String) -> Result<SgRoot> {
    CoreRoot::parse(&lang, src, None, None)
        .map(SgRoot::from_core)
        .map_err(|e| Error::from_reason(format!("Failed to parse: {e}")))
}

/// Synchronous, like the QuickJS facade (natively there is nothing to await).
#[napi(js_name = "parseAsync")]
pub fn parse_async(lang: String, src: String) -> Result<SgRoot> {
    parse(lang, src)
}

#[napi(js_name = "parseFile")]
pub fn parse_file(lang: String, file_path: String) -> Result<SgRoot> {
    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| Error::from_reason(format!("Failed to read file: {e}")))?;
    CoreRoot::parse(&lang, content, Some(file_path), None)
        .map(SgRoot::from_core)
        .map_err(|e| Error::from_reason(format!("Failed to parse: {e}")))
}

#[napi]
pub fn kind(lang: String, kind_name: String) -> Result<u32> {
    use ast_grep_core::tree_sitter::LanguageExt;
    let lang = CodemodLang::from_str(&lang)
        .map_err(|e| Error::from_reason(format!("Language error: {e}")))?;
    Ok(lang.get_ts_language().id_for_node_kind(&kind_name, true) as u32)
}

// ---------------------------------------------------------------------------
// Plain data
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct Position {
    pub line: u32,
    pub column: u32,
    pub index: u32,
}

#[napi(object)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[napi(object, js_name = "Edit")]
pub struct JsEdit {
    #[napi(js_name = "startPos")]
    pub start_pos: u32,
    #[napi(js_name = "endPos")]
    pub end_pos: u32,
    #[napi(js_name = "insertedText")]
    pub inserted_text: String,
}

impl From<Edit> for JsEdit {
    fn from(edit: Edit) -> Self {
        JsEdit {
            start_pos: edit.start_pos,
            end_pos: edit.end_pos,
            inserted_text: edit.inserted_text,
        }
    }
}

#[napi(object)]
pub struct DefinitionOptionsJs {
    #[napi(js_name = "resolveExternal")]
    pub resolve_external: Option<bool>,
}

#[napi(object, object_from_js = false)]
pub struct Definition {
    pub node: SgNode,
    pub root: SgRoot,
    pub kind: String,
}

#[napi(object, object_from_js = false)]
pub struct FileReferences {
    pub root: SgRoot,
    pub nodes: Vec<Option<SgNode>>,
}

// ---------------------------------------------------------------------------
// SgRoot
// ---------------------------------------------------------------------------

#[napi]
pub struct SgRoot {
    core: CoreRoot,
}

impl SgRoot {
    fn from_core(core: CoreRoot) -> Self {
        SgRoot { core }
    }
}

#[napi]
impl SgRoot {
    #[napi(constructor)]
    pub fn new(env: Env) -> Result<Self> {
        Err(type_error(
            &env,
            "'SgRoot' is not constructible. Use 'parse(lang, src)'.",
        ))
    }

    #[napi]
    pub fn root(&self) -> SgNode {
        SgNode::from_core(self.core.root())
    }

    #[napi]
    pub fn filename(&self) -> String {
        self.core.filename().unwrap_or_default().to_string()
    }

    #[napi(js_name = "relativeFilename")]
    pub fn relative_filename(&self) -> String {
        self.core
            .relative_filename()
            .unwrap_or_default()
            .to_string()
    }

    #[napi]
    pub fn source(&self) -> String {
        self.core.root_text()
    }

    /// Rename the current file; relative paths resolve against its directory.
    #[napi]
    pub fn rename(&self, env: Env, new_path: String) -> Result<()> {
        let resolved = self
            .core
            .resolve_rename_path(&new_path)
            .map_err(|e| core_error(&env, e))?;
        with_target(|target| match target {
            Some(target) => check_within_target(target, Path::new(&resolved), "rename()"),
            None => Ok(()),
        })
        .map_err(|e| core_error(&env, e))?;
        self.core
            .set_rename_to(resolved)
            .map_err(|e| core_error(&env, e))
    }

    /// Write another file's content. Inside a transform the write is staged
    /// (and contained to the target directory); outside one it goes to disk.
    #[napi]
    pub fn write(&self, env: Env, content: String) -> Result<()> {
        let file_path = self
            .core
            .check_write_target()
            .map_err(|e| core_error(&env, e))?
            .to_string();
        let path = Path::new(&file_path);
        let staged = CONTEXT.with(|context| -> Result<bool> {
            let mut context = context.borrow_mut();
            let Some(context) = context.as_mut() else {
                return Ok(false);
            };
            check_within_target(&context.target_dir, path, "write()")
                .map_err(|e| core_error(&env, e))?;
            context.changes.push(FileChange {
                path: PathBuf::from(&file_path),
                result: ExecutionResult::Modified(ModifiedResult {
                    content: content.clone(),
                    rename_to: None,
                }),
            });
            Ok(true)
        })?;
        if !staged {
            std::fs::write(path, &content).map_err(|e| {
                Error::from_reason(format!("Failed to write file '{file_path}': {e}"))
            })?;
        }
        self.core.notify_written(path, &content);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SgNode
// ---------------------------------------------------------------------------

#[napi]
pub struct SgNode {
    core: CoreNode,
}

impl SgNode {
    fn from_core(core: CoreNode) -> Self {
        SgNode { core }
    }

    fn wrap_all(nodes: Vec<CoreNode>) -> Vec<SgNode> {
        nodes.into_iter().map(SgNode::from_core).collect()
    }

    fn matcher(&self, env: &Env, value: Unknown<'_>) -> Result<CoreMatcher> {
        let input = match value.get_type()? {
            ValueType::String => {
                MatcherInput::Pattern(unsafe { String::from_napi_value(env.raw(), value.raw())? })
            }
            ValueType::Number => {
                let number = unsafe { f64::from_napi_value(env.raw(), value.raw())? };
                MatcherInput::Kind(number as u16)
            }
            ValueType::Object => {
                let json: serde_json::Value =
                    unsafe { serde_json::Value::from_napi_value(env.raw(), value.raw())? };
                MatcherInput::Config(json)
            }
            _ => {
                return Err(type_error(
                    env,
                    "Matcher must be an object with a 'pattern' or 'kind' property",
                ))
            }
        };
        compile_matcher(input, self.core.root().lang()).map_err(|e| core_error(env, e))
    }
}

#[napi]
impl SgNode {
    #[napi(constructor)]
    pub fn new(env: Env) -> Result<Self> {
        Err(type_error(&env, "'SgNode' is not constructible."))
    }

    #[napi]
    pub fn text(&self) -> String {
        self.core.text()
    }

    #[napi]
    pub fn is(&self, kind: String) -> bool {
        self.core.is(&kind)
    }

    #[napi]
    pub fn kind(&self) -> String {
        self.core.kind()
    }

    #[napi]
    pub fn range(&self) -> Range {
        let range = self.core.range();
        let position = |p: codemod_sandbox::sg_core::Position| Position {
            line: p.line as u32,
            column: p.column as u32,
            index: p.index as u32,
        };
        Range {
            start: position(range.start),
            end: position(range.end),
        }
    }

    #[napi]
    pub fn id(&self) -> f64 {
        self.core.id() as f64
    }

    #[napi(js_name = "isLeaf")]
    pub fn is_leaf(&self) -> bool {
        self.core.is_leaf()
    }

    #[napi(js_name = "isNamed")]
    pub fn is_named(&self) -> bool {
        self.core.is_named()
    }

    #[napi(js_name = "isNamedLeaf")]
    pub fn is_named_leaf(&self) -> bool {
        self.core.is_named_leaf()
    }

    #[napi]
    pub fn parent(&self) -> Option<SgNode> {
        self.core.parent().map(SgNode::from_core)
    }

    #[napi]
    pub fn child(&self, nth: u32) -> Option<SgNode> {
        self.core.child(nth as usize).map(SgNode::from_core)
    }

    #[napi]
    pub fn children(&self) -> Vec<SgNode> {
        Self::wrap_all(self.core.children())
    }

    #[napi]
    pub fn ancestors(&self) -> Vec<SgNode> {
        Self::wrap_all(self.core.ancestors())
    }

    #[napi]
    pub fn next(&self) -> Option<SgNode> {
        self.core.next().map(SgNode::from_core)
    }

    #[napi(js_name = "nextAll")]
    pub fn next_all(&self) -> Vec<SgNode> {
        Self::wrap_all(self.core.next_all())
    }

    #[napi]
    pub fn prev(&self) -> Option<SgNode> {
        self.core.prev().map(SgNode::from_core)
    }

    #[napi(js_name = "prevAll")]
    pub fn prev_all(&self) -> Vec<SgNode> {
        Self::wrap_all(self.core.prev_all())
    }

    #[napi]
    pub fn field(&self, name: String) -> Option<SgNode> {
        self.core.field(&name).map(SgNode::from_core)
    }

    #[napi(js_name = "fieldChildren")]
    pub fn field_children(&self, name: String) -> Vec<SgNode> {
        Self::wrap_all(self.core.field_children(&name))
    }

    #[napi(ts_args_type = "matcher: string | number | object")]
    pub fn matches(&self, env: Env, matcher: Unknown<'_>) -> Result<bool> {
        let matcher = self.matcher(&env, matcher)?;
        Ok(self.core.matches(&matcher))
    }

    #[napi(ts_args_type = "matcher: string | number | object")]
    pub fn inside(&self, env: Env, matcher: Unknown<'_>) -> Result<bool> {
        let matcher = self.matcher(&env, matcher)?;
        Ok(self.core.inside(&matcher))
    }

    #[napi(ts_args_type = "matcher: string | number | object")]
    pub fn has(&self, env: Env, matcher: Unknown<'_>) -> Result<bool> {
        let matcher = self.matcher(&env, matcher)?;
        Ok(self.core.has(&matcher))
    }

    #[napi(ts_args_type = "matcher: string | number | object")]
    pub fn precedes(&self, env: Env, matcher: Unknown<'_>) -> Result<bool> {
        let matcher = self.matcher(&env, matcher)?;
        Ok(self.core.precedes(&matcher))
    }

    #[napi(ts_args_type = "matcher: string | number | object")]
    pub fn follows(&self, env: Env, matcher: Unknown<'_>) -> Result<bool> {
        let matcher = self.matcher(&env, matcher)?;
        Ok(self.core.follows(&matcher))
    }

    #[napi(js_name = "getMatch")]
    pub fn get_match(&self, meta_var: String) -> Option<SgNode> {
        self.core.get_match(&meta_var).map(SgNode::from_core)
    }

    #[napi(js_name = "getMultipleMatches")]
    pub fn get_multiple_matches(&self, meta_var: String) -> Vec<SgNode> {
        Self::wrap_all(self.core.get_multiple_matches(&meta_var))
    }

    /// The transformed meta-variable as its raw bytes (an array of numbers),
    /// exactly what the QuickJS facade returns.
    #[napi(js_name = "getTransformed")]
    pub fn get_transformed(&self, meta_var: String) -> Option<Vec<u32>> {
        self.core
            .get_transformed_raw(&meta_var)
            .map(|bytes| bytes.iter().map(|&b| b as u32).collect())
    }

    #[napi(ts_args_type = "matcher: string | number | object")]
    pub fn find(&self, env: Env, matcher: Unknown<'_>) -> Result<Option<SgNode>> {
        let matcher = self.matcher(&env, matcher)?;
        Ok(self.core.find(&matcher).map(SgNode::from_core))
    }

    #[napi(
        js_name = "findAll",
        ts_args_type = "matcher: string | number | object"
    )]
    pub fn find_all(&self, env: Env, matcher: Unknown<'_>) -> Result<Vec<SgNode>> {
        let matcher = self.matcher(&env, matcher)?;
        Ok(Self::wrap_all(self.core.find_all(&matcher)))
    }

    #[napi]
    pub fn replace(&self, text: String) -> JsEdit {
        self.core.replace(text).into()
    }

    #[napi(js_name = "commitEdits")]
    pub fn commit_edits(&self, edits: Vec<JsEdit>) -> String {
        let edits: Vec<Edit> = edits
            .into_iter()
            .map(|edit| Edit {
                start_pos: edit.start_pos,
                end_pos: edit.end_pos,
                inserted_text: edit.inserted_text,
            })
            .collect();
        self.core.commit_edits(&edits)
    }

    #[napi]
    pub fn debug(&self) -> String {
        self.core.debug()
    }

    #[napi(js_name = "getRoot")]
    pub fn get_root(&self) -> SgRoot {
        SgRoot::from_core(self.core.root().clone())
    }

    #[napi]
    pub fn definition(
        &self,
        env: Env,
        options: Option<DefinitionOptionsJs>,
    ) -> Result<Option<Definition>> {
        let Some(request) = self.core.semantic().map_err(|e| core_error(&env, e))? else {
            return Ok(None);
        };
        let resolve_external = options
            .and_then(|options| options.resolve_external)
            .unwrap_or(true);
        Ok(request
            .definition(DefinitionOptions { resolve_external })
            .map_err(|e| core_error(&env, e))?
            .map(|def| Definition {
                node: SgNode::from_core(def.node),
                root: SgRoot::from_core(def.root),
                kind: def.kind.to_string(),
            }))
    }

    /// One entry per file; unlike the QuickJS facade, files whose language
    /// is unsupported (and nodes not found) are `null` rather than holes.
    #[napi]
    pub fn references(&self, env: Env) -> Result<Vec<Option<FileReferences>>> {
        let Some(request) = self.core.semantic().map_err(|e| core_error(&env, e))? else {
            return Ok(Vec::new());
        };
        Ok(request
            .references()
            .map_err(|e| core_error(&env, e))?
            .into_iter()
            .map(|file| {
                file.map(|file| FileReferences {
                    root: SgRoot::from_core(file.root),
                    nodes: file
                        .nodes
                        .into_iter()
                        .map(|node| node.map(SgNode::from_core))
                        .collect(),
                })
            })
            .collect())
    }

    #[napi(js_name = "typeInfo")]
    pub fn type_info(&self, env: Env) -> Result<Option<String>> {
        let Some(request) = self.core.semantic().map_err(|e| core_error(&env, e))? else {
            return Ok(None);
        };
        request.type_info().map_err(|e| core_error(&env, e))
    }
}
