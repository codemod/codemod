//! Binding-agnostic AST core behind the JSSG `SgRoot` / `SgNode` API.
//!
//! This module is pure Rust over `ast-grep-core`; it has no knowledge of any
//! JavaScript engine. JS facades (the rquickjs classes in
//! [`super::sg_node`], and the napi-rs addon) are thin wrappers that convert
//! arguments/results and map [`CoreError`] into their engine's exceptions.
//!
//! # Ownership model
//!
//! * [`CoreRoot`] is a cheap `Clone` handle (`Arc`) around one parsed document
//!   ([`AstGrep<CoreDoc>`]) plus per-file metadata (filename, target
//!   directory, pending `rename()` target, semantic seam). Two handles are the
//!   same root iff [`CoreRoot::ptr_eq`].
//! * [`CoreNode`] is a self-contained, `'static` node handle: it owns a clone
//!   of its `CoreRoot` (keeping the tree alive) and a `NodeMatch` whose
//!   lifetime is erased to `'static`. This is sound because the tree lives in
//!   the `Arc` allocation, never moves, and outlives every node that holds the
//!   `Arc`. A node also carries the meta-variable environment of the match
//!   that produced it (`getMatch` / `getMultipleMatches` / `getTransformed`).
//! * `CoreRoot` is `Send + Sync` (the `AstGrep` tree is immutable and the only
//!   mutable state is behind a `Mutex`), so roots may be parsed or matched
//!   off-thread. `CoreNode` wraps tree-sitter node pointers and is **not**
//!   `Send`; create and use nodes on the thread that owns the JS engine.
//!
//! # Semantic seam
//!
//! `definition()`, `references()` and `typeInfo()` go through
//! [`CoreNode::semantic`], which consults the root's [`SemanticSeam`]:
//! `Disabled` behaves like "no provider configured" (JS sees `null` / `[]`),
//! `Unsupported` makes every semantic call fail with the given message (used
//! by runtimes that do not wire semantic analysis), and `Provider` (native
//! builds only) forwards to a [`language_core::SemanticProvider`].
//!
//! # Host-specific behaviour kept out of the core
//!
//! Anything that needs host state stays in the facades: path containment
//! checks against the run's target directory, dry-run / staged writes for
//! `SgRoot.write()`, and the `jssgTransform` accumulator. The core exposes the
//! pure pieces (`resolve_rename_path`, `set_rename_to`, `check_write_target`,
//! `notify_written`).

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use crate::ast_grep::wasm_lang::{WasmDoc, WasmLang};
use ast_grep_config::{DeserializeEnv, RuleCore, SerializableRuleCore};
use ast_grep_core::matcher::{KindMatcher, Matcher, MatcherExt};
use ast_grep_core::meta_var::{MetaVarEnv, Underlying};
#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::{AstGrep, Doc, Node, NodeMatch, Pattern};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
use std::str::FromStr;
use std::sync::{Arc, Mutex};

#[cfg(feature = "native")]
use language_core::SemanticProvider;

/// Language type the core parses with in the current build.
#[cfg(feature = "native")]
pub type CoreLang = crate::sandbox::engine::codemod_lang::CodemodLang;
#[cfg(all(
    not(all(feature = "wasm", target_arch = "wasm32")),
    not(feature = "native")
))]
pub type CoreLang = ast_grep_language::SupportLang;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub type CoreLang = WasmLang;

/// Document type backing every [`CoreRoot`] in the current build.
#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
pub type CoreDoc = StrDoc<CoreLang>;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub type CoreDoc = WasmDoc;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Which JS error class a facade should raise for a [`CoreError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreErrorKind {
    /// A plain `Error` (rquickjs: `Exception::throw_message`).
    Error,
    /// A `TypeError` (rquickjs: `Exception::throw_type`), e.g. invalid matcher.
    TypeError,
}

/// Error returned by core operations. `message` is the exact text JSSG users
/// see; facades must not rewrite it.
#[derive(Debug, Clone)]
pub struct CoreError {
    pub kind: CoreErrorKind,
    pub message: String,
}

impl CoreError {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            kind: CoreErrorKind::Error,
            message: message.into(),
        }
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Self {
            kind: CoreErrorKind::TypeError,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}

pub type CoreResult<T> = std::result::Result<T, CoreError>;

// ---------------------------------------------------------------------------
// Plain data types shared with the JS API
// ---------------------------------------------------------------------------

/// JS `Position`: zero-based line, column (as reported by ast-grep) and byte
/// index.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
    pub index: usize,
}

/// JS `Range`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct NodeRange {
    pub start: Position,
    pub end: Position,
}

/// JS `Edit` (`{ startPos, endPos, insertedText }`, byte offsets).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    #[serde(rename = "startPos")]
    pub start_pos: u32,
    #[serde(rename = "endPos")]
    pub end_pos: u32,
    #[serde(rename = "insertedText")]
    pub inserted_text: String,
}

// ---------------------------------------------------------------------------
// Matchers
// ---------------------------------------------------------------------------

/// Engine-neutral form of the JS matcher argument accepted by
/// `find`/`findAll`/`matches`/`inside`/`has`/`precedes`/`follows`.
#[derive(Debug, Clone)]
pub enum MatcherInput {
    /// A JS string: an ast-grep pattern.
    Pattern(String),
    /// A JS number: a tree-sitter kind id (truncated to `u16` by the facade).
    Kind(u16),
    /// A JS object: a `RuleConfig` (`{ rule, constraints, utils, ... }`)
    /// converted to JSON.
    Config(serde_json::Value),
}

/// Compiled matcher. Reusable across nodes of the same language.
#[allow(clippy::large_enum_variant)]
pub enum CoreMatcher {
    Pattern(Pattern),
    Kind(KindMatcher),
    Config(RuleCore),
}

impl Matcher for CoreMatcher {
    fn match_node_with_env<'tree, D: Doc>(
        &self,
        node: Node<'tree, D>,
        env: &mut Cow<'_, MetaVarEnv<'tree, D>>,
    ) -> Option<Node<'tree, D>> {
        match self {
            CoreMatcher::Pattern(p) => p.match_node_with_env(node, env),
            CoreMatcher::Kind(k) => k.match_node_with_env(node, env),
            CoreMatcher::Config(c) => c.match_node_with_env(node, env),
        }
    }
}

/// Compile a matcher for `lang`. Rule-config deserialization failures are
/// reported as [`CoreErrorKind::TypeError`].
pub fn compile_matcher(input: MatcherInput, lang: CoreLang) -> CoreResult<CoreMatcher> {
    match input {
        MatcherInput::Pattern(pattern) => Ok(CoreMatcher::Pattern(Pattern::new(&pattern, lang))),
        MatcherInput::Kind(kind_id) => Ok(CoreMatcher::Kind(KindMatcher::from_id(kind_id))),
        MatcherInput::Config(value) => {
            let serializable: SerializableRuleCore =
                serde_json::from_value(value).map_err(|e| CoreError::type_error(e.to_string()))?;
            let env = DeserializeEnv::new(lang);
            let config = serializable
                .get_matcher(env)
                .map_err(|e| CoreError::type_error(e.to_string()))?;
            Ok(CoreMatcher::Config(config))
        }
    }
}

// ---------------------------------------------------------------------------
// Semantic seam
// ---------------------------------------------------------------------------

/// How a root answers `definition()` / `references()` / `typeInfo()`.
#[derive(Clone, Default)]
pub enum SemanticSeam {
    /// No semantic analysis: `definition()`/`typeInfo()` yield `null`,
    /// `references()` yields `[]`. This is the behaviour of a JSSG run without
    /// a semantic provider.
    #[default]
    Disabled,
    /// Semantic analysis is not available in this runtime; every semantic
    /// call fails with this message.
    Unsupported(Arc<str>),
    /// Forward to a semantic provider (native builds only).
    #[cfg(feature = "native")]
    Provider(Arc<dyn SemanticProvider>),
}

impl SemanticSeam {
    #[cfg(feature = "native")]
    pub fn from_provider(provider: Option<Arc<dyn SemanticProvider>>) -> Self {
        match provider {
            Some(provider) => SemanticSeam::Provider(provider),
            None => SemanticSeam::Disabled,
        }
    }

    #[cfg(feature = "native")]
    fn provider(&self) -> Option<&Arc<dyn SemanticProvider>> {
        match self {
            SemanticSeam::Provider(provider) => Some(provider),
            _ => None,
        }
    }
}

/// Result of [`SemanticRequest::definition`].
pub struct DefinitionResult {
    pub node: CoreNode,
    pub root: CoreRoot,
    /// `"local"`, `"import"` or `"external"`.
    pub kind: &'static str,
}

/// One entry of [`SemanticRequest::references`].
pub struct FileReferences {
    pub root: CoreRoot,
    /// One slot per reported location; `None` where no node could be found at
    /// the location (the QuickJS facade leaves a hole in the JS array).
    pub nodes: Vec<Option<CoreNode>>,
}

/// Options for [`SemanticRequest::definition`] (JS `DefinitionOptions`).
#[derive(Debug, Clone, Copy)]
pub struct DefinitionOptions {
    pub resolve_external: bool,
}

impl Default for DefinitionOptions {
    fn default() -> Self {
        Self {
            resolve_external: true,
        }
    }
}

/// A semantic query bound to a node whose root has an active provider and a
/// file path. Obtained from [`CoreNode::semantic`].
pub struct SemanticRequest<'a> {
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    node: &'a CoreNode,
    #[cfg(feature = "native")]
    provider: Arc<dyn SemanticProvider>,
    #[cfg(feature = "native")]
    file_path: PathBuf,
    #[cfg(not(feature = "native"))]
    never: std::convert::Infallible,
}

// ---------------------------------------------------------------------------
// Roots
// ---------------------------------------------------------------------------

struct RootData {
    grep: AstGrep<CoreDoc>,
    filename: Option<String>,
    relative_filename: Option<String>,
    target_directory: Option<PathBuf>,
    /// Rename target recorded by `rename()`.
    rename_to: Mutex<Option<String>>,
    semantic: SemanticSeam,
    /// The file the current transform is processing (for `write()` checks).
    current_file_path: Option<String>,
}

/// Construction options for [`CoreRoot::from_ast_grep`].
#[derive(Clone, Default)]
pub struct RootOptions<'a> {
    /// Absolute (or caller-meaningful) path of the file; `None` for
    /// `parse(lang, src)` roots.
    pub filename: Option<String>,
    /// Run target directory, used to derive `relativeFilename()`.
    pub target_directory: Option<&'a Path>,
    pub semantic: SemanticSeam,
    /// Path of the file being transformed; `write()` refuses to write it.
    pub current_file_path: Option<String>,
}

/// Shared handle to a parsed document. See the module docs.
#[derive(Clone)]
pub struct CoreRoot {
    inner: Arc<RootData>,
}

impl CoreRoot {
    /// Parse `src` as `lang` (same rules and error strings as JS `parse`).
    pub fn parse(
        lang: &str,
        src: String,
        filename: Option<String>,
        target_directory: Option<&Path>,
    ) -> std::result::Result<Self, String> {
        #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
        {
            if !WasmLang::is_parser_initialized() {
                return Err(
                    "Tree-sitter parser not initialized. Call setupParser() first before parsing."
                        .to_string(),
                );
            }

            let lang =
                <WasmLang as std::str::FromStr>::from_str(lang).map_err(|e| e.to_string())?;
            let doc = WasmDoc::try_new(src, lang).map_err(|e| e.to_string())?;
            // SAFETY: preserved from the original implementation; `AstGrep<D>`
            // is a transparent wrapper around its document.
            let grep: AstGrep<CoreDoc> = unsafe { std::mem::transmute(doc) };
            Ok(Self::from_ast_grep(
                grep,
                RootOptions {
                    filename,
                    target_directory,
                    ..Default::default()
                },
            ))
        }

        #[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
        {
            let parsed_lang = CoreLang::from_str(lang)
                .map_err(|e| format!("Unsupported language: {lang}. Error: {e}"))?;
            let grep = AstGrep::new(src, parsed_lang);
            Ok(Self::from_ast_grep(
                grep,
                RootOptions {
                    filename,
                    target_directory,
                    ..Default::default()
                },
            ))
        }
    }

    /// Wrap an already-parsed document.
    pub fn from_ast_grep(grep: AstGrep<CoreDoc>, options: RootOptions<'_>) -> Self {
        let RootOptions {
            filename,
            target_directory,
            semantic,
            current_file_path,
        } = options;
        Self {
            inner: Arc::new(RootData {
                grep,
                relative_filename: compute_relative_filename(filename.as_deref(), target_directory),
                target_directory: target_directory.map(Path::to_path_buf),
                filename,
                rename_to: Mutex::new(None),
                semantic,
                current_file_path,
            }),
        }
    }

    /// The root node (`SgRoot.root()`).
    pub fn root(&self) -> CoreNode {
        let node_match: NodeMatch<'_, CoreDoc> = self.inner.grep.root().into();
        // SAFETY: the node borrows from `self.inner.grep`, which is kept alive
        // by the root handle stored in the returned `CoreNode`.
        unsafe { CoreNode::from_match(self, node_match) }
    }

    /// True if both handles refer to the same parsed document.
    pub fn ptr_eq(&self, other: &CoreRoot) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn ast_grep(&self) -> &AstGrep<CoreDoc> {
        &self.inner.grep
    }

    // `CoreLang` is `Copy` natively but only `Clone` for the wasm build.
    #[allow(clippy::clone_on_copy)]
    pub fn lang(&self) -> CoreLang {
        self.inner.grep.lang().clone()
    }

    /// File path given at construction (`None` for `parse(lang, src)`).
    pub fn filename(&self) -> Option<&str> {
        self.inner.filename.as_deref()
    }

    /// Path relative to the target directory, falling back to `filename()`.
    pub fn relative_filename(&self) -> Option<&str> {
        self.inner
            .relative_filename
            .as_deref()
            .or(self.inner.filename.as_deref())
    }

    pub fn target_directory(&self) -> Option<&Path> {
        self.inner.target_directory.as_deref()
    }

    pub fn current_file_path(&self) -> Option<&str> {
        self.inner.current_file_path.as_deref()
    }

    pub fn semantic(&self) -> &SemanticSeam {
        &self.inner.semantic
    }

    /// The full document text, leading whitespace and a BOM included (JS
    /// `SgRoot.source()`). tree-sitter starts the root node at its first
    /// token, so the root node's own text drops that padding (CDMD-5018).
    pub fn root_text(&self) -> String {
        self.document_source().into_owned()
    }

    #[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
    fn document_source(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.inner.grep.source())
    }

    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    fn document_source(&self) -> Cow<'_, str> {
        Cow::Owned(self.inner.grep.root().get_doc().source_text())
    }

    /// The full source the document was parsed from.
    pub fn source(&self) -> String {
        self.inner.grep.source().to_string()
    }

    /// Nodes matched by `matcher` in pre-order (the engine's selector pass
    /// that produces `options.matches`).
    pub fn select<M: Matcher>(&self, matcher: &M) -> Vec<CoreNode> {
        self.inner
            .grep
            .root()
            .dfs()
            .filter_map(|node| matcher.match_node(node))
            // SAFETY: matches borrow from `self.inner.grep`.
            .map(|node_match| unsafe { CoreNode::from_match(self, node_match) })
            .collect()
    }

    /// Resolve a `rename()` argument: rejects empty paths and resolves
    /// relative paths against the current file's directory. The caller is
    /// responsible for any containment check before [`Self::set_rename_to`].
    pub fn resolve_rename_path(&self, new_path: &str) -> CoreResult<String> {
        if new_path.is_empty() {
            return Err(CoreError::error("rename() requires a non-empty path"));
        }
        if Path::new(new_path).is_absolute() {
            return Ok(new_path.to_string());
        }
        Ok(match &self.inner.filename {
            Some(filename) => {
                let parent = Path::new(filename).parent().unwrap_or(Path::new("."));
                parent.join(new_path).to_string_lossy().to_string()
            }
            None => new_path.to_string(),
        })
    }

    /// Record the rename target. Fails if `rename()` was already called.
    pub fn set_rename_to(&self, resolved_path: String) -> CoreResult<()> {
        let mut rename_to = self
            .inner
            .rename_to
            .lock()
            .map_err(|e| CoreError::error(format!("Failed to lock rename_to mutex: {e}")))?;
        if rename_to.is_some() {
            return Err(CoreError::error(
                "rename() has already been called for this file. It can only be called once.",
            ));
        }
        *rename_to = Some(resolved_path);
        Ok(())
    }

    /// Rename target recorded by `rename()`, if any.
    pub fn rename_to(&self) -> Option<String> {
        self.inner
            .rename_to
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Validate an `SgRoot.write()` call and return the path to write: the
    /// root must have a filename and must not be the file currently being
    /// transformed. Containment, dry-run and staging are the host's concern.
    pub fn check_write_target(&self) -> CoreResult<&str> {
        let file_path = self
            .inner
            .filename
            .as_deref()
            .ok_or_else(|| CoreError::error("Cannot write: file has no path"))?;

        if let Some(current_file) = &self.inner.current_file_path {
            let file_path_normalized = Path::new(file_path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(file_path));
            let current_file_normalized = Path::new(current_file)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(current_file));

            if file_path_normalized == current_file_normalized {
                return Err(CoreError::error(
                    "Cannot call write() on the current file. Return the modified content from transform() instead.",
                ));
            }
        }
        Ok(file_path)
    }

    /// Tell the semantic provider (if any) that `path` now holds `content`.
    /// Errors from the provider are ignored, matching `SgRoot.write()`.
    pub fn notify_written(&self, path: &Path, content: &str) {
        #[cfg(feature = "native")]
        if let Some(provider) = self.inner.semantic.provider() {
            let _ = provider.notify_file_processed(path, content);
        }
        #[cfg(not(feature = "native"))]
        let _ = (path, content);
    }
}

/// Host-side containment rule shared by every facade (`rename()`, staged
/// `write()`, `jssgTransform()`): `path` (canonicalized, or its parent when
/// it does not exist yet) must lie under the canonical `target_dir`. `caller`
/// names the API in the error (e.g. `"rename()"`).
pub fn check_within_target(target_dir: &Path, path: &Path, caller: &str) -> CoreResult<()> {
    let canonical_target = target_dir
        .canonicalize()
        .unwrap_or_else(|_| target_dir.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| {
        // File may not exist yet (e.g. rename target); canonicalize the parent instead
        if let Some(parent) = path.parent() {
            let canonical_parent = parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf());
            canonical_parent.join(path.file_name().unwrap_or_default())
        } else {
            path.to_path_buf()
        }
    });
    if canonical_path.starts_with(&canonical_target) {
        Ok(())
    } else {
        Err(CoreError::error(format!(
            "{} path '{}' is outside the target directory '{}'",
            caller,
            path.display(),
            target_dir.display()
        )))
    }
}

fn compute_relative_filename(
    filename: Option<&str>,
    target_directory: Option<&Path>,
) -> Option<String> {
    let filename = filename?;
    let target_directory = target_directory?;

    let file_path = Path::new(filename);
    if let Ok(relative) = file_path.strip_prefix(target_directory) {
        return Some(relative.to_string_lossy().replace('\\', "/"));
    }

    let canonical_target = target_directory.canonicalize().ok()?;
    let canonical_file = file_path.canonicalize().ok().or_else(|| {
        file_path.parent().and_then(|parent| {
            let file_name = file_path.file_name()?;
            parent
                .canonicalize()
                .ok()
                .map(|canonical_parent| canonical_parent.join(file_name))
        })
    })?;

    canonical_file
        .strip_prefix(&canonical_target)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// Owned node handle. See the module docs for the lifetime model.
#[derive(Clone)]
pub struct CoreNode {
    // Field order matters: `node` borrows from `root` and is dropped first.
    node: NodeMatch<'static, CoreDoc>,
    root: CoreRoot,
}

impl CoreNode {
    /// Erase the lifetime of a match taken from `root`'s tree.
    ///
    /// # Safety
    /// `node_match` must borrow from `root.ast_grep()` (not from another
    /// document).
    unsafe fn from_match(root: &CoreRoot, node_match: NodeMatch<'_, CoreDoc>) -> Self {
        let node: NodeMatch<'static, CoreDoc> = std::mem::transmute(node_match);
        CoreNode {
            node,
            root: root.clone(),
        }
    }

    fn wrap(&self, node: Node<'_, CoreDoc>) -> CoreNode {
        // SAFETY: every node reachable from `self.node` belongs to `self.root`.
        unsafe { CoreNode::from_match(&self.root, node.into()) }
    }

    fn wrap_match(&self, node_match: NodeMatch<'_, CoreDoc>) -> CoreNode {
        // SAFETY: see `wrap`.
        unsafe { CoreNode::from_match(&self.root, node_match) }
    }

    /// The underlying ast-grep match (node + meta-variable environment),
    /// borrowed for no longer than this handle.
    pub fn node_match<'a>(&'a self) -> &'a NodeMatch<'a, CoreDoc> {
        // SAFETY: shortening the erased `'static` back to the borrow of
        // `self`, which keeps the root (and therefore the tree) alive.
        // `NodeMatch` is invariant, so this cannot be done by coercion.
        unsafe {
            std::mem::transmute::<&'a NodeMatch<'static, CoreDoc>, &'a NodeMatch<'a, CoreDoc>>(
                &self.node,
            )
        }
    }

    /// The root this node belongs to (`SgNode.getRoot()`).
    pub fn root(&self) -> &CoreRoot {
        &self.root
    }

    /// Compile a matcher for this node's language.
    pub fn compile_matcher(&self, input: MatcherInput) -> CoreResult<CoreMatcher> {
        compile_matcher(input, self.root.lang())
    }

    pub fn text(&self) -> String {
        self.node.text().to_string()
    }

    pub fn kind(&self) -> String {
        self.node.kind().to_string()
    }

    pub fn is(&self, kind: &str) -> bool {
        self.node.kind() == kind
    }

    pub fn range(&self) -> NodeRange {
        let start_pos = self.node.start_pos();
        let end_pos = self.node.end_pos();
        let byte_range = self.node.range();
        NodeRange {
            start: Position {
                line: start_pos.line(),
                column: start_pos.column(&self.node),
                index: byte_range.start,
            },
            end: Position {
                line: end_pos.line(),
                column: end_pos.column(&self.node),
                index: byte_range.end,
            },
        }
    }

    /// Byte range `[start, end)` of this node in the root source.
    pub fn byte_range(&self) -> std::ops::Range<usize> {
        self.node.range()
    }

    pub fn id(&self) -> usize {
        self.node.node_id()
    }

    pub fn is_leaf(&self) -> bool {
        self.node.is_leaf()
    }

    pub fn is_named(&self) -> bool {
        self.node.is_named()
    }

    pub fn is_named_leaf(&self) -> bool {
        self.node.is_named_leaf()
    }

    pub fn parent(&self) -> Option<CoreNode> {
        self.node.parent().map(|n| self.wrap(n))
    }

    pub fn child(&self, nth: usize) -> Option<CoreNode> {
        self.node.child(nth).map(|n| self.wrap(n))
    }

    pub fn children(&self) -> Vec<CoreNode> {
        self.node.children().map(|n| self.wrap(n)).collect()
    }

    pub fn ancestors(&self) -> Vec<CoreNode> {
        self.node.ancestors().map(|n| self.wrap(n)).collect()
    }

    pub fn next(&self) -> Option<CoreNode> {
        self.node.next().map(|n| self.wrap(n))
    }

    pub fn next_all(&self) -> Vec<CoreNode> {
        self.node.next_all().map(|n| self.wrap(n)).collect()
    }

    pub fn prev(&self) -> Option<CoreNode> {
        self.node.prev().map(|n| self.wrap(n))
    }

    pub fn prev_all(&self) -> Vec<CoreNode> {
        self.node.prev_all().map(|n| self.wrap(n)).collect()
    }

    pub fn field(&self, name: &str) -> Option<CoreNode> {
        self.node.field(name).map(|n| self.wrap(n))
    }

    pub fn field_children(&self, name: &str) -> Vec<CoreNode> {
        self.node
            .field_children(name)
            .map(|n| self.wrap(n))
            .collect()
    }

    pub fn matches<M: Matcher>(&self, matcher: &M) -> bool {
        self.node.matches(matcher)
    }

    pub fn inside<M: Matcher>(&self, matcher: &M) -> bool {
        self.node.inside(matcher)
    }

    pub fn has<M: Matcher>(&self, matcher: &M) -> bool {
        self.node.has(matcher)
    }

    pub fn precedes<M: Matcher>(&self, matcher: &M) -> bool {
        self.node.precedes(matcher)
    }

    pub fn follows<M: Matcher>(&self, matcher: &M) -> bool {
        self.node.follows(matcher)
    }

    pub fn find<M: Matcher>(&self, matcher: &M) -> Option<CoreNode> {
        self.node.find(matcher).map(|m| self.wrap_match(m))
    }

    pub fn find_all<M: Matcher>(&self, matcher: &M) -> Vec<CoreNode> {
        self.node
            .find_all(matcher)
            .map(|m| self.wrap_match(m))
            .collect()
    }

    pub fn get_match(&self, meta_var: &str) -> Option<CoreNode> {
        self.node
            .get_env()
            .get_match(meta_var)
            .map(|n| self.wrap(n.clone()))
    }

    pub fn get_multiple_matches(&self, meta_var: &str) -> Vec<CoreNode> {
        self.node
            .get_env()
            .get_multiple_matches(meta_var)
            .into_iter()
            .map(|n| self.wrap(n))
            .collect()
    }

    /// Raw transformed meta-variable (`transform:` section of a rule) in the
    /// document's underlying encoding. The QuickJS facade hands this to JS
    /// unchanged for compatibility.
    pub fn get_transformed_raw(&self, meta_var: &str) -> Option<&Underlying<CoreDoc>> {
        self.node.get_env().get_transformed(meta_var)
    }

    /// Transformed meta-variable decoded as text (UTF-8 builds only).
    #[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
    pub fn get_transformed(&self, meta_var: &str) -> Option<String> {
        self.get_transformed_raw(meta_var)
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    /// An edit replacing this node's text (`SgNode.replace()`).
    pub fn replace(&self, text: String) -> Edit {
        let byte_range = self.node.range();
        Edit {
            start_pos: byte_range.start as u32,
            end_pos: byte_range.end as u32,
            inserted_text: text,
        }
    }

    /// Apply `edits` to this node's text (`SgNode.commitEdits()`): edits are
    /// sorted by start, overlapping edits are skipped. On the root node they
    /// apply to the whole document, so leading blank lines, whitespace, and a
    /// BOM outside the root node's range are kept byte for byte (CDMD-5018).
    pub fn commit_edits(&self, edits: &[Edit]) -> String {
        let (old_content, offset) = if self.node.parent().is_none() {
            (self.root.document_source(), 0)
        } else {
            (self.node.text(), self.node.range().start)
        };
        if edits.is_empty() {
            return old_content.into_owned();
        }

        let mut sorted_edits: Vec<&Edit> = edits.iter().collect();
        sorted_edits.sort_by_key(|edit| edit.start_pos);

        let mut new_content = String::with_capacity(old_content.len());
        let mut start = 0;

        for edit in sorted_edits {
            let pos = edit.start_pos as usize - offset;
            // Skip overlapping edits
            if start > pos {
                continue;
            }
            new_content.push_str(&old_content[start..pos]);
            new_content.push_str(&edit.inserted_text);
            start = edit.end_pos as usize - offset;
        }

        // Add trailing content
        new_content.push_str(&old_content[start..]);
        new_content
    }

    /// Human-readable tree dump (`SgNode.debug()`).
    pub fn debug(&self) -> String {
        fn format_node<D: Doc>(node: &Node<D>, indent: usize) -> String {
            let indent_str = "  ".repeat(indent);
            let kind = node.kind();
            let is_named = node.is_named();

            let mut result = String::new();

            if is_named {
                let text = node.text();
                let has_children = node.children().next().is_some();

                if has_children {
                    result.push_str(&format!("{indent_str}{kind}:\n"));
                    for child in node.children() {
                        result.push_str(&format_node(&child, indent + 1));
                    }
                } else {
                    // Leaf named node — show text inline
                    let display_text = if text.len() > 40 {
                        match text.char_indices().nth(40) {
                            Some((idx, _)) => format!("{}...", &text[..idx]),
                            None => text.to_string(),
                        }
                    } else {
                        text.to_string()
                    };
                    result.push_str(&format!("{indent_str}{kind}: {display_text:?}\n"));
                }
            } else {
                // Anonymous node (punctuation, keywords)
                let text = node.text();
                result.push_str(&format!("{indent_str}[{text:?}]\n"));
            }

            result
        }

        let node: Node<CoreDoc> = self.node.clone().into();
        format_node(&node, 0)
    }

    /// Enter the semantic seam.
    ///
    /// * `Err(_)` when the root's seam is [`SemanticSeam::Unsupported`].
    /// * `Ok(None)` when semantic analysis is disabled or the root has no
    ///   filename: JS `definition()`/`typeInfo()` return `null` and
    ///   `references()` returns `[]`.
    /// * `Ok(Some(request))` otherwise.
    pub fn semantic(&self) -> CoreResult<Option<SemanticRequest<'_>>> {
        match &self.root.inner.semantic {
            SemanticSeam::Disabled => Ok(None),
            SemanticSeam::Unsupported(message) => Err(CoreError::error(message.to_string())),
            #[cfg(feature = "native")]
            SemanticSeam::Provider(provider) => {
                let Some(filename) = &self.root.inner.filename else {
                    return Ok(None);
                };
                Ok(Some(SemanticRequest {
                    node: self,
                    provider: Arc::clone(provider),
                    file_path: PathBuf::from(filename),
                }))
            }
        }
    }
}

impl<'a> SemanticRequest<'a> {
    /// `SgNode.definition()`.
    pub fn definition(self, options: DefinitionOptions) -> CoreResult<Option<DefinitionResult>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = options;
            match self.never {}
        }
        #[cfg(feature = "native")]
        {
            let def_options = language_core::DefinitionOptions {
                resolve_external: options.resolve_external,
            };
            let lookup =
                self.provider
                    .get_definition(&self.file_path, self.byte_range(), def_options);
            let def_result = match lookup {
                Ok(Some(def_result)) => def_result,
                Ok(None) => return Ok(None),
                Err(e) => return Err(CoreError::error(format!("Failed to get definition: {}", e))),
            };

            let loc = &def_result.location;
            let kind = def_result.kind.as_str();
            let root = if loc.file_path == self.file_path {
                // Definition is in the same file, use existing root
                self.node.root.clone()
            } else {
                // Definition is in a different file, create new root
                let lang_str = detect_language_from_path(&loc.file_path);
                let lang = CoreLang::from_str(&lang_str)
                    .map_err(|e| CoreError::error(format!("Unsupported language: {}", e)))?;
                let grep = AstGrep::new(def_result.content.clone(), lang);
                self.sibling_root(grep, &loc.file_path)
            };

            Ok(
                find_node_at_range(&root, loc.range.start as usize, loc.range.end as usize)
                    .map(|node| DefinitionResult { node, root, kind }),
            )
        }
    }

    /// `SgNode.references()`. One slot per file reported by the provider;
    /// `None` for files whose language is unsupported (the QuickJS facade
    /// leaves a hole in the JS array).
    pub fn references(self) -> CoreResult<Vec<Option<FileReferences>>> {
        #[cfg(not(feature = "native"))]
        {
            match self.never {}
        }
        #[cfg(feature = "native")]
        {
            let refs_result = self
                .provider
                .find_references(&self.file_path, self.byte_range())
                .map_err(|e| CoreError::error(format!("Failed to find references: {}", e)))?;

            Ok(refs_result
                .files
                .iter()
                .map(|file_refs| {
                    let root = if file_refs.file_path == self.file_path {
                        // Use existing root for same file
                        self.node.root.clone()
                    } else {
                        // Create new root for different file
                        let lang_str = detect_language_from_path(&file_refs.file_path);
                        // Skip files with unsupported languages
                        let lang = CoreLang::from_str(&lang_str).ok()?;
                        let grep = AstGrep::new(file_refs.content.clone(), lang);
                        self.sibling_root(grep, &file_refs.file_path)
                    };
                    let nodes = file_refs
                        .locations
                        .iter()
                        .map(|loc| {
                            find_node_at_range(
                                &root,
                                loc.range.start as usize,
                                loc.range.end as usize,
                            )
                        })
                        .collect();
                    Some(FileReferences { root, nodes })
                })
                .collect())
        }
    }

    /// `SgNode.typeInfo()` (not implemented by current providers).
    pub fn type_info(self) -> CoreResult<Option<String>> {
        #[cfg(not(feature = "native"))]
        {
            match self.never {}
        }
        #[cfg(feature = "native")]
        {
            self.provider
                .get_type(&self.file_path, self.byte_range())
                .map_err(|e| CoreError::error(format!("Failed to get type: {}", e)))
        }
    }

    #[cfg(feature = "native")]
    fn byte_range(&self) -> language_core::ByteRange {
        let byte_range = self.node.node.range();
        language_core::ByteRange::new(byte_range.start as u32, byte_range.end as u32)
    }

    /// A root for another file that shares this root's semantic context.
    #[cfg(feature = "native")]
    fn sibling_root(&self, grep: AstGrep<CoreDoc>, file_path: &Path) -> CoreRoot {
        let current = &self.node.root;
        CoreRoot::from_ast_grep(
            grep,
            RootOptions {
                filename: Some(file_path.to_string_lossy().to_string()),
                target_directory: current.target_directory(),
                semantic: current.semantic().clone(),
                current_file_path: current.inner.current_file_path.clone(),
            },
        )
    }
}

/// Find the tightest node in `root` containing the byte range.
#[cfg(feature = "native")]
fn find_node_at_range(root: &CoreRoot, start: usize, end: usize) -> Option<CoreNode> {
    let root_node = root.inner.grep.root();
    let mut current = root_node.clone();

    // traverse down to find the tightest node containing the range
    let found = loop {
        let mut found_child = None;
        for child in current.children() {
            let child_range = child.range();
            if child_range.start <= start && child_range.end >= end {
                // This child contains our range, go deeper
                found_child = Some(child);
                break;
            }
        }

        match found_child {
            Some(child) => {
                // check if this child exactly matches or is tighter
                let child_range = child.range();
                if child_range.start == start && child_range.end == end {
                    break Some(child);
                }
                current = child;
            }
            None => {
                // no child contains the range, current is the tightest
                let current_range = current.range();
                if current_range.start <= start && current_range.end >= end {
                    break Some(current);
                }
                break None;
            }
        }
    };
    // SAFETY: `found` borrows from `root`'s tree.
    found.map(|node| unsafe { CoreNode::from_match(root, node.into()) })
}

/// Detect language from file path extension (semantic cross-file roots).
#[cfg(feature = "native")]
fn detect_language_from_path(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts") | Some("mts") | Some("cts") => "typescript".to_string(),
        Some("tsx") => "tsx".to_string(),
        Some("js") | Some("mjs") | Some("cjs") => "javascript".to_string(),
        Some("jsx") => "jsx".to_string(),
        Some("py") | Some("pyi") => "python".to_string(),
        _ => "typescript".to_string(), // Default to typescript
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    fn parse(src: &str) -> CoreRoot {
        CoreRoot::parse("typescript", src.to_string(), None, None).unwrap()
    }

    #[test]
    fn root_handles_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CoreRoot>();
        assert_send_sync::<CoreMatcher>();
    }

    #[test]
    fn find_replace_commit_roundtrip() {
        let root = parse("console.log(a);\nconsole.log(b);\n");
        let matcher = root
            .root()
            .compile_matcher(MatcherInput::Pattern("console.log($A)".into()))
            .unwrap();
        let nodes = root.root().find_all(&matcher);
        assert_eq!(nodes.len(), 2);
        let edits: Vec<Edit> = nodes
            .iter()
            .map(|n| {
                let arg = n.get_match("A").unwrap().text();
                n.replace(format!("logger.info({arg})"))
            })
            .collect();
        assert_eq!(
            root.root().commit_edits(&edits),
            "logger.info(a);\nlogger.info(b);\n"
        );
    }

    #[test]
    fn nodes_outlive_root_handle() {
        let node = {
            let root = parse("let x = 1;");
            root.root().child(0).unwrap()
        };
        assert_eq!(node.text(), "let x = 1;");
        assert_eq!(node.root().root_text(), "let x = 1;");
        assert!(node.parent().unwrap().parent().is_none());
    }

    #[test]
    fn config_matcher_errors_are_type_errors() {
        let root = parse("x");
        let err = match root
            .root()
            .compile_matcher(MatcherInput::Config(serde_json::json!({"rule": 5})))
        {
            Ok(_) => panic!("expected an error"),
            Err(err) => err,
        };
        assert_eq!(err.kind, CoreErrorKind::TypeError);
    }

    #[test]
    fn unsupported_semantic_seam_errors() {
        let grep = AstGrep::new("x", CoreLang::from_str("typescript").unwrap());
        let root = CoreRoot::from_ast_grep(
            grep,
            RootOptions {
                filename: Some("/tmp/a.ts".into()),
                semantic: SemanticSeam::Unsupported("semantic analysis is unsupported".into()),
                ..Default::default()
            },
        );
        let err = root.root().semantic().err().unwrap();
        assert_eq!(err.message, "semantic analysis is unsupported");
        assert!(parse("x").root().semantic().unwrap().is_none());
    }

    #[test]
    fn rename_resolution_and_once_only() {
        let root = CoreRoot::parse(
            "typescript",
            "x".into(),
            Some("/repo/src/a.less".into()),
            Some(Path::new("/repo")),
        )
        .unwrap();
        assert_eq!(root.relative_filename(), Some("src/a.less"));
        let resolved = root.resolve_rename_path("a.css").unwrap();
        assert_eq!(resolved, "/repo/src/a.css");
        root.set_rename_to(resolved).unwrap();
        assert!(root.set_rename_to("b".into()).is_err());
        assert_eq!(root.rename_to().as_deref(), Some("/repo/src/a.css"));
        assert!(root.resolve_rename_path("").is_err());
    }
}
