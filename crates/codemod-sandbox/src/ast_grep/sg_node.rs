#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use crate::ast_grep::wasm_lang::WasmDoc;
#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
use ast_grep_core::tree_sitter::StrDoc as TSStrDoc;
use ast_grep_core::{AstGrep, Node, NodeMatch};

#[cfg(all(
    not(all(feature = "wasm", target_arch = "wasm32")),
    not(feature = "native")
))]
use ast_grep_language::SupportLang;

#[cfg(feature = "native")]
use language_core::SemanticProvider;

use rquickjs::{
    class, class::Trace, methods, prelude::Opt, Ctx, Exception, IntoJs, JsLifetime, Result, Value,
};
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::ast_grep::types::JsEdit;
use crate::ast_grep::types::JsNodeRange;
use crate::ast_grep::utils::convert_matcher;

#[cfg(all(
    not(all(feature = "wasm", target_arch = "wasm32")),
    not(feature = "native")
))]
use ast_grep_language::SupportLang as Lang;

#[cfg(feature = "native")]
use crate::sandbox::engine::codemod_lang::CodemodLang;
#[cfg(feature = "native")]
use CodemodLang as Lang;

#[cfg(all(
    not(all(feature = "wasm", target_arch = "wasm32")),
    not(feature = "native")
))]
type TSDoc = TSStrDoc<SupportLang>;
#[cfg(feature = "native")]
type TSDoc = TSStrDoc<CodemodLang>;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
type TSDoc = WasmDoc;

/// Full source text of the parsed document, including any leading whitespace or BOM.
///
/// Prefer this over `grep.root().text()`: tree-sitter's root node range starts at the first
/// token, so its text drops leading padding.
#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
pub(crate) fn document_source(grep: &AstGrep<TSDoc>) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(grep.source())
}

/// Full source text of the parsed document, including any leading whitespace or BOM.
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub(crate) fn document_source(grep: &AstGrep<TSDoc>) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(grep.root().get_doc().source_text())
}

pub(crate) struct SgRootInner {
    pub(crate) grep: AstGrep<TSDoc>,
    filename: Option<String>,
    relative_filename: Option<String>,
    target_directory: Option<std::path::PathBuf>,
    /// Optional rename target path set by root.rename()
    pub(crate) rename_to: Mutex<Option<String>>,
    /// `filename`, canonicalized once, for `SgNode.key()`.
    canonical_filename: std::sync::OnceLock<String>,
    /// Optional semantic provider for symbol indexing (native only)
    #[cfg(feature = "native")]
    pub(crate) semantic_provider: Option<Arc<dyn SemanticProvider>>,
    /// The current file being processed (for write() validation)
    #[cfg(feature = "native")]
    pub(crate) current_file_path: Option<String>,
    /// Made by `jssgTransform` for a file it changes: `rename()` is part of
    /// that change, which the step commits, so callbacks may call it.
    #[cfg(feature = "native")]
    pub(crate) transformed_file: bool,
}

#[derive(Trace, Clone)]
#[class(rename_all = "camelCase")]
pub struct SgRootRjs<'js> {
    #[qjs(skip_trace)]
    pub(crate) inner: Arc<SgRootInner>,
    #[qjs(skip_trace)]
    _phantom: PhantomData<&'js ()>,
}

unsafe impl<'js> JsLifetime<'js> for SgRootRjs<'js> {
    type Changed<'to> = SgRootRjs<'to>;
}

#[methods]
impl<'js> SgRootRjs<'js> {
    #[qjs(constructor)]
    pub fn new_constructor_js(_ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(
            &_ctx,
            "'SgRoot' is not constructible. Use 'parse(lang, src)'.",
        ))
    }

    pub fn root(&self, _ctx: Ctx<'js>) -> Result<SgNodeRjs<'js>> {
        let node = self.inner.grep.root();
        let node_match: NodeMatch<_> = node.into();
        let static_node_match: NodeMatch<'static, TSDoc> =
            unsafe { std::mem::transmute(node_match) };
        Ok(SgNodeRjs {
            root: Arc::clone(&self.inner),
            inner_node: static_node_match,
            _phantom: PhantomData,
        })
    }

    pub fn filename(&self) -> Result<String> {
        Ok(self.inner.filename.clone().unwrap_or_default())
    }

    #[qjs(rename = "relativeFilename")]
    pub fn relative_filename(&self) -> Result<String> {
        Ok(self
            .inner
            .relative_filename
            .clone()
            .unwrap_or_else(|| self.inner.filename.clone().unwrap_or_default()))
    }

    pub fn source(&self) -> Result<String> {
        Ok(document_source(&self.inner.grep).into_owned())
    }

    /// Write content to this file.
    ///
    /// This method is only valid for files obtained via `definition()` or `references()`.
    /// It cannot be called on the current file being processed - for that, return the
    /// modified content from the `transform()` function instead.
    ///
    /// After writing, the semantic provider's cache is updated with the new content.
    /// Rename the current file to a new path.
    ///
    /// If the path is relative, it is resolved against the current file's parent directory.
    /// If absolute, it is used as-is.
    pub fn rename(&self, new_path: String, ctx: Ctx<'js>) -> Result<()> {
        #[cfg(feature = "native")]
        if in_callback_isolate(&ctx) && !self.inner.transformed_file {
            return Err(Exception::throw_message(
                &ctx,
                "root.rename() is for jssg transforms; in a forEachFile callback, call file.rename(path)",
            ));
        }
        if new_path.is_empty() {
            return Err(Exception::throw_message(
                &ctx,
                "rename() requires a non-empty path",
            ));
        }

        let resolved_path = if std::path::Path::new(&new_path).is_absolute() {
            #[cfg(feature = "native")]
            let new_path = crate::sandbox::engine::execution_engine::native_path(&new_path)
                .to_string_lossy()
                .into_owned();
            new_path
        } else {
            // Resolve relative to current file's parent directory, part by
            // part so the separators are the platform's.
            match &self.inner.filename {
                Some(filename) => {
                    let parent = std::path::Path::new(filename)
                        .parent()
                        .unwrap_or(std::path::Path::new("."));
                    std::path::Path::new(&new_path)
                        .components()
                        .fold(parent.to_path_buf(), |path, part| path.join(part))
                        .to_string_lossy()
                        .to_string()
                }
                None => new_path,
            }
        };

        // Validate: resolved path must stay within the target directory
        #[cfg(feature = "native")]
        {
            use crate::sandbox::engine::execution_engine::validate_path_within_target;
            validate_path_within_target(&ctx, std::path::Path::new(&resolved_path), "rename()")?;
        }

        let mut rename_to = self.inner.rename_to.lock().map_err(|e| {
            Exception::throw_message(&ctx, &format!("Failed to lock rename_to mutex: {e}"))
        })?;
        if rename_to.is_some() {
            return Err(Exception::throw_message(
                &ctx,
                "rename() has already been called for this file. It can only be called once.",
            ));
        }
        *rename_to = Some(resolved_path);
        Ok(())
    }

    pub fn write(&self, content: String, ctx: Ctx<'js>) -> Result<()> {
        #[cfg(not(feature = "native"))]
        {
            let _ = content;
            return Err(Exception::throw_message(
                &ctx,
                "write() is only available in native mode",
            ));
        }
        #[cfg(feature = "native")]
        {
            let file_path = match &self.inner.filename {
                Some(f) => f,
                None => {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Cannot write: file has no path",
                    ))
                }
            };

            if self.inner.current_file_path.is_none() && in_callback_isolate(&ctx) {
                return Err(Exception::throw_message(
                    &ctx,
                    "root.write() is for files found through definition() or references(), which roots parsed in a forEachFile callback have none of; call file.write(text) for the callback's file",
                ));
            }

            if let Some(current_file) = &self.inner.current_file_path {
                let file_path_normalized = std::path::Path::new(file_path)
                    .canonicalize()
                    .unwrap_or_else(|_| std::path::PathBuf::from(file_path));
                let current_file_normalized = std::path::Path::new(current_file)
                    .canonicalize()
                    .unwrap_or_else(|_| std::path::PathBuf::from(current_file));

                if file_path_normalized == current_file_normalized {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Cannot call write() on the current file. Return the modified content from transform() instead.",
                    ));
                }
            }

            let dry_run = ctx
                .userdata::<crate::sandbox::engine::execution_engine::DryRunExecutionFlag>()
                .map(|flag| flag.0)
                .unwrap_or(false);
            if dry_run {
                return Ok(());
            }

            let path = std::path::Path::new(file_path);
            // Answers describe the step's start: pin the old text before it
            // goes, so the rest of the step doesn't see this write.
            if let Some(ref provider) = self.inner.semantic_provider {
                let _ = provider.snapshot_content(path);
            }
            std::fs::write(path, &content).map_err(|e| {
                Exception::throw_message(
                    &ctx,
                    &format!("Failed to write file '{}': {}", file_path, e),
                )
            })?;

            Ok(())
        }
    }

    // Module resolution for codemods that rewrite imports (workspace scope).

    /// The file `specifier` resolves to from this root's file, or null.
    #[qjs(rename = "resolveModule")]
    pub fn resolve_module(&self, ctx: Ctx<'js>, specifier: String) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = specifier;
            return Err(Exception::throw_message(
                &ctx,
                &not_in_wasm("resolveModule"),
            ));
        }
        #[cfg(feature = "native")]
        {
            let (provider, file) = self.module_query(&ctx, "resolveModule")?;
            let path = provider
                .resolve_module(&file, &specifier)
                .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?;
            or_null(&ctx, path.map(|p| p.to_string_lossy().to_string()))
        }
    }

    /// What `name` (`"default"` for the default export) of `module` lands
    /// on, following re-exports: a `definition()`-shaped object, or null.
    /// `module` is a specifier resolved from this root's file, or a path.
    #[qjs(rename = "resolveExport")]
    pub fn resolve_export(
        &self,
        ctx: Ctx<'js>,
        module: String,
        name: String,
    ) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = (module, name);
            return Err(Exception::throw_message(
                &ctx,
                &not_in_wasm("resolveExport"),
            ));
        }
        #[cfg(feature = "native")]
        {
            let (provider, file) = self.module_query(&ctx, "resolveExport")?;
            let Some(module_path) = module_path(&*provider, &file, &module)
                .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?
            else {
                return Ok(Value::new_null(ctx));
            };
            match provider
                .resolve_export(&module_path, &name)
                .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?
            {
                Some(def) => definition_value(&ctx, &self.inner, &file, &def),
                None => Ok(Value::new_null(ctx)),
            }
        }
    }

    /// The node a `node.key()` names, in this root: null when the key names
    /// another file, or this file has no node of that range and kind (its
    /// text changed). Keys are plain strings, so they can be passed between
    /// workflow steps and turned back into nodes here.
    #[qjs(rename = "nodeByKey")]
    pub fn node_by_key(&self, ctx: Ctx<'js>, key: String) -> Result<Value<'js>> {
        let Some((file, start, end, kind)) = parse_node_key(&key) else {
            return Err(Exception::throw_type(
                &ctx,
                &format!("nodeByKey(key): `{key}` is not a node.key() (file:start-end:kind)"),
            ));
        };
        if file != self.inner.canonical_filename() {
            return Ok(Value::new_null(ctx));
        }
        match SgNodeRjs::node_with(&self.inner, start, end, kind) {
            Some(node) => node.into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    /// The specifiers other packages can import this file with (its
    /// package's name and `exports` subpaths that resolve to it); empty when
    /// it is not part of its package's public API.
    #[qjs(rename = "publicSpecifiers")]
    pub fn public_specifiers(&self, ctx: Ctx<'js>) -> Result<Vec<String>> {
        #[cfg(not(feature = "native"))]
        {
            return Err(Exception::throw_message(
                &ctx,
                &not_in_wasm("publicSpecifiers"),
            ));
        }
        #[cfg(feature = "native")]
        {
            let (provider, file) = self.module_query(&ctx, "publicSpecifiers")?;
            provider
                .public_specifiers(&file)
                .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))
        }
    }

    /// Whether importing this module runs code: a call made at module top
    /// level (`program.callees()`), an `import "./x"` side-effect import, or
    /// a top-level write to an object's member (`registry.x = …`,
    /// `window.y++`). Declarations alone (functions, classes, constants
    /// from literals) are not effects. A "may" answer: an unresolved call
    /// counts, even if it is pure.
    #[qjs(rename = "hasTopLevelEffects")]
    pub fn has_top_level_effects(&self, ctx: Ctx<'js>) -> Result<bool> {
        #[cfg(not(feature = "native"))]
        {
            return Err(Exception::throw_message(
                &ctx,
                &not_in_wasm("hasTopLevelEffects"),
            ));
        }
        #[cfg(feature = "native")]
        {
            let program = self.inner.grep.root();
            for statement in program.children() {
                match statement.kind().as_ref() {
                    "import_statement"
                        if !statement.children().any(|c| c.kind() == "import_clause") =>
                    {
                        return Ok(true);
                    }
                    "expression_statement" => {
                        let writes_member = statement.dfs().any(|n| {
                            matches!(
                                n.kind().as_ref(),
                                "assignment_expression"
                                    | "augmented_assignment_expression"
                                    | "update_expression"
                            ) && n.children().next().is_some_and(|target| {
                                matches!(
                                    target.kind().as_ref(),
                                    "member_expression" | "subscript_expression"
                                )
                            })
                        });
                        if writes_member {
                            return Ok(true);
                        }
                    }
                    _ => {}
                }
            }
            let length = self.inner.grep.root().range().end as u32;
            let Some(node) =
                SgNodeRjs::node_in(&self.inner, language_core::ByteRange::new(0, length))
            else {
                return Ok(false);
            };
            let calls = node.call_graph(ctx.clone(), Opt(None), true)?;
            Ok(calls.as_array().is_some_and(|calls| !calls.is_empty()))
        }
    }

    /// Every place in the workspace that refers to `target` (this root's
    /// file by default; a path or an SgRoot): `import` and `export … from`
    /// statements, `require()`, `import()`, and test mocks. An array of
    /// `{ node, root, kind, specifier }` with `unparsed`.
    #[qjs(rename = "importers")]
    pub fn importers(&self, ctx: Ctx<'js>, target: Opt<Value<'js>>) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = target;
            return Err(Exception::throw_message(&ctx, &not_in_wasm("importers")));
        }
        #[cfg(feature = "native")]
        {
            let (provider, file) = self.module_query(&ctx, "importers")?;
            let target_path = match target.0 {
                None => file.clone(),
                Some(value) if value.is_undefined() || value.is_null() => file.clone(),
                Some(value) => target_path(&ctx, &file, value, "importers")?,
            };
            let found = provider
                .importers(&target_path)
                .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?;

            let result = rquickjs::Array::new(ctx.clone())?;
            let mut seen = std::collections::HashSet::new();
            let mut count = 0;
            for file in &found.files {
                let root = if file.file_path == file_path_of(&self.inner) {
                    Arc::clone(&self.inner)
                } else {
                    let lang = Lang::from_str(&detect_language_from_path(&file.file_path))
                        .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?;
                    match SgRootRjs::try_new_with_semantic(
                        AstGrep::new(file.content.clone(), lang),
                        Some(file.file_path.to_string_lossy().to_string()),
                        self.inner.semantic_provider.clone(),
                        self.inner.current_file_path.clone(),
                        self.inner.target_directory.as_deref(),
                    ) {
                        Ok(root) => root.inner,
                        Err(_) => continue,
                    }
                };
                for importer in &file.importers {
                    let Some(mut node) = SgNodeRjs::node_in(&root, importer.range) else {
                        continue;
                    };
                    // A static import is found by its binding: report the statement.
                    if importer.kind == language_core::ImportKind::Import {
                        if let Some(statement) = node.ancestor_of_kind("import_statement") {
                            node = statement;
                        }
                    }
                    if !seen.insert(node.key()) {
                        continue;
                    }
                    let object = rquickjs::Object::new(ctx.clone())?;
                    object.set("node", node)?;
                    object.set(
                        "root",
                        SgRootRjs {
                            inner: Arc::clone(&root),
                            _phantom: PhantomData,
                        },
                    )?;
                    object.set("kind", importer.kind.as_str())?;
                    object.set("specifier", importer.specifier.clone())?;
                    result.set(count, object)?;
                    count += 1;
                }
            }
            let unparsed = rquickjs::Array::new(ctx.clone())?;
            for (index, file) in found.unparsed.iter().enumerate() {
                let object = rquickjs::Object::new(ctx.clone())?;
                object.set("path", file.file_path.to_string_lossy().to_string())?;
                object.set("message", file.message.clone())?;
                unparsed.set(index, object)?;
            }
            result.as_object().set("unparsed", unparsed)?;
            result.into_js(&ctx)
        }
    }

    /// A specifier this root's file can import `target` with (a path, or a
    /// root from `definition()`/`resolveExport()`), checked by resolving it;
    /// null when none reaches it. `options.like` keeps the style of the
    /// specifier being replaced; `options.prefer` is "relative" or "alias".
    #[qjs(rename = "specifierFor")]
    pub fn specifier_for(
        &self,
        ctx: Ctx<'js>,
        target: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = (target, options);
            return Err(Exception::throw_message(&ctx, &not_in_wasm("specifierFor")));
        }
        #[cfg(feature = "native")]
        {
            let (provider, file) = self.module_query(&ctx, "specifierFor")?;
            let target_path = target_path(&ctx, &file, target, "specifierFor")?;
            let mut spec_options = language_core::SpecifierOptions::default();
            if let Some(opts) = query_options(&ctx, options, "specifierFor", &["prefer", "like"])? {
                spec_options.like = opts.get::<_, Option<String>>("like")?;
                spec_options.prefer = match opts.get::<_, Option<String>>("prefer")?.as_deref() {
                    None => None,
                    Some("relative") => Some(language_core::SpecifierStyle::Relative),
                    Some("alias") => Some(language_core::SpecifierStyle::Alias),
                    Some(other) => {
                        return Err(Exception::throw_range(
                            &ctx,
                            &format!(
                                "specifierFor(options): prefer must be \"relative\" or \"alias\", not \"{other}\""
                            ),
                        ))
                    }
                };
            }
            let specifier = provider
                .specifier_for(&file, &target_path, &spec_options)
                .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?;
            or_null(&ctx, specifier)
        }
    }
}

#[cfg(feature = "native")]
impl SgRootRjs<'_> {
    /// The provider and this root's file, or why a module query cannot run.
    fn module_query(
        &self,
        ctx: &Ctx<'_>,
        method: &str,
    ) -> Result<(Arc<dyn SemanticProvider>, std::path::PathBuf)> {
        let Some(filename) = self.inner.filename.as_ref() else {
            return Err(Exception::throw_message(
                ctx,
                &format!("{method}() needs this root's file, and it was parsed from a string."),
            ));
        };
        let Some(provider) = self.inner.semantic_provider.clone() else {
            return Err(Exception::throw_message(
                ctx,
                &format!(
                    "{method}() needs semantic analysis in workspace scope, and it is off for this \
                     code. Set `semanticAnalysis: \"workspace\"` on the step (`semantic_analysis: \
                     workspace` in workflow.yaml, `--semantic-workspace <dir>` with `codemod jssg run`)."
                ),
            ));
        };
        Ok((provider, std::path::PathBuf::from(filename)))
    }
}

/// A query's `target`: a path (relative to `file`'s directory, or absolute)
/// or an SgRoot with a file.
#[cfg(feature = "native")]
fn target_path<'js>(
    ctx: &Ctx<'js>,
    file: &std::path::Path,
    target: Value<'js>,
    method: &str,
) -> Result<std::path::PathBuf> {
    let path = if let Some(path) = target.as_string() {
        std::path::PathBuf::from(path.to_string()?)
    } else if let Ok(root) = rquickjs::Class::<SgRootRjs>::from_value(&target) {
        match root.borrow().inner.filename.clone() {
            Some(name) => std::path::PathBuf::from(name),
            None => {
                return Err(Exception::throw_type(
                    ctx,
                    &format!(
                        "{method}(target): this root was parsed from a string and has no file"
                    ),
                ))
            }
        }
    } else {
        return Err(Exception::throw_type(
            ctx,
            &format!("{method}(target): target must be a file path or an SgRoot"),
        ));
    };
    Ok(if path.is_relative() {
        file.parent().unwrap_or(file).join(path)
    } else {
        path
    })
}

/// A root's file, canonicalized, for comparing with provider paths.
#[cfg(feature = "native")]
fn file_path_of(root: &SgRootInner) -> std::path::PathBuf {
    std::path::PathBuf::from(root.canonical_filename())
}

/// `module` as a path: absolute paths as they are, anything else resolved
/// as a specifier from `from_file`.
#[cfg(feature = "native")]
fn module_path(
    provider: &dyn SemanticProvider,
    from_file: &std::path::Path,
    module: &str,
) -> language_core::SemanticResult<Option<std::path::PathBuf>> {
    let path = std::path::Path::new(module);
    if path.is_absolute() {
        return Ok(Some(path.to_path_buf()));
    }
    provider.resolve_module(from_file, module)
}

/// Whether this context runs workflow callbacks, whose file changes go
/// through the step's commit.
#[cfg(feature = "native")]
fn in_callback_isolate(ctx: &Ctx<'_>) -> bool {
    ctx.userdata::<crate::sandbox::engine::execution_engine::CallbackIsolate>()
        .is_some()
}

impl SgRootInner {
    /// Why a semantic query on this root can't run: its text is not the text
    /// the provider answers about, because the step produced it. Answers
    /// describe the file as it was when the step started; mapping their
    /// offsets into this text would silently point at the wrong code.
    #[cfg(feature = "native")]
    fn edited_in_this_step(
        &self,
        provider: &dyn SemanticProvider,
        file_path: &std::path::Path,
        method: &str,
    ) -> Option<String> {
        let snapshot = provider.snapshot_content(file_path)?;
        if *snapshot == *document_source(&self.grep) {
            return None;
        }
        let name = self
            .relative_filename
            .as_deref()
            .or(self.filename.as_deref())
            .unwrap_or_default();
        Some(format!(
            "{method}(): {name} changed in this step; semantic answers describe the file as it \
             was when the step started. Query it in the next step."
        ))
    }

    /// The file this root was parsed from, canonicalized (so two roots for
    /// one file agree), or "" for a root parsed from a string.
    fn canonical_filename(&self) -> &str {
        self.canonical_filename
            .get_or_init(|| match &self.filename {
                #[cfg(feature = "native")]
                Some(name) => std::fs::canonicalize(name)
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|_| name.clone()),
                #[cfg(not(feature = "native"))]
                Some(name) => name.clone(),
                None => String::new(),
            })
    }
}

impl<'js> SgRootRjs<'js> {
    /// Mark a root `jssgTransform` made for the file it changes.
    #[cfg(feature = "native")]
    pub(crate) fn into_transformed_file(mut self) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.transformed_file = true;
        }
        self
    }

    /// Get the rename target path, if rename() was called.
    pub fn get_rename_to(&self) -> Option<String> {
        self.inner
            .rename_to
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn try_new(
        lang_str: String,
        src: String,
        filename: Option<String>,
        target_directory: Option<&std::path::Path>,
    ) -> std::result::Result<Self, String> {
        #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
        {
            if !crate::ast_grep::wasm_lang::WasmLang::is_parser_initialized() {
                return Err(
                    "Tree-sitter parser not initialized. Call setupParser() first before parsing."
                        .to_string(),
                );
            }

            let lang = crate::ast_grep::wasm_lang::WasmLang::from_str(&lang_str)
                .map_err(|e| e.to_string())?;
            let doc = crate::ast_grep::wasm_lang::WasmDoc::try_new(src, lang)
                .map_err(|e| e.to_string())?;

            Ok(SgRootRjs {
                inner: Arc::new(SgRootInner {
                    grep: unsafe { std::mem::transmute(doc) },
                    relative_filename: compute_relative_filename(
                        filename.as_deref(),
                        target_directory,
                    ),
                    target_directory: target_directory.map(|path| path.to_path_buf()),
                    filename,
                    rename_to: Mutex::new(None),
                    canonical_filename: std::sync::OnceLock::new(),
                }),
                _phantom: PhantomData,
            })
        }

        #[cfg(all(
            not(all(feature = "wasm", target_arch = "wasm32")),
            not(feature = "native")
        ))]
        {
            let lang = SupportLang::from_str(&lang_str)
                .map_err(|e| format!("Unsupported language: {lang_str}. Error: {e}"))?;
            let grep = AstGrep::new(src, lang);
            Ok(SgRootRjs {
                inner: Arc::new(SgRootInner {
                    grep,
                    relative_filename: compute_relative_filename(
                        filename.as_deref(),
                        target_directory,
                    ),
                    target_directory: target_directory.map(|path| path.to_path_buf()),
                    filename,
                    rename_to: Mutex::new(None),
                    canonical_filename: std::sync::OnceLock::new(),
                    #[cfg(feature = "native")]
                    semantic_provider: None,
                    #[cfg(feature = "native")]
                    current_file_path: None,
                    #[cfg(feature = "native")]
                    transformed_file: false,
                }),
                _phantom: PhantomData,
            })
        }

        #[cfg(feature = "native")]
        {
            let lang = CodemodLang::from_str(&lang_str)
                .map_err(|e| format!("Unsupported language: {lang_str}. Error: {e}"))?;
            let grep = AstGrep::new(src, lang);
            Ok(SgRootRjs {
                inner: Arc::new(SgRootInner {
                    grep,
                    relative_filename: compute_relative_filename(
                        filename.as_deref(),
                        target_directory,
                    ),
                    target_directory: target_directory.map(|path| path.to_path_buf()),
                    filename,
                    rename_to: Mutex::new(None),
                    canonical_filename: std::sync::OnceLock::new(),
                    #[cfg(feature = "native")]
                    semantic_provider: None,
                    #[cfg(feature = "native")]
                    current_file_path: None,
                    #[cfg(feature = "native")]
                    transformed_file: false,
                }),
                _phantom: PhantomData,
            })
        }
    }

    pub fn try_new_from_ast_grep(
        grep: AstGrep<TSDoc>,
        filename: Option<String>,
        target_directory: Option<&std::path::Path>,
    ) -> std::result::Result<Self, String> {
        Ok(SgRootRjs {
            inner: Arc::new(SgRootInner {
                grep,
                relative_filename: compute_relative_filename(filename.as_deref(), target_directory),
                target_directory: target_directory.map(|path| path.to_path_buf()),
                filename,
                rename_to: Mutex::new(None),
                canonical_filename: std::sync::OnceLock::new(),
                #[cfg(feature = "native")]
                semantic_provider: None,
                #[cfg(feature = "native")]
                current_file_path: None,
                #[cfg(feature = "native")]
                transformed_file: false,
            }),
            _phantom: PhantomData,
        })
    }

    /// Create a new SgRootRjs with a semantic provider for symbol indexing.
    #[cfg(feature = "native")]
    pub fn try_new_with_semantic(
        grep: AstGrep<TSDoc>,
        filename: Option<String>,
        semantic_provider: Option<Arc<dyn SemanticProvider>>,
        current_file_path: Option<String>,
        target_directory: Option<&std::path::Path>,
    ) -> std::result::Result<Self, String> {
        Ok(SgRootRjs {
            inner: Arc::new(SgRootInner {
                grep,
                relative_filename: compute_relative_filename(filename.as_deref(), target_directory),
                target_directory: target_directory.map(|path| path.to_path_buf()),
                filename,
                rename_to: Mutex::new(None),
                canonical_filename: std::sync::OnceLock::new(),
                semantic_provider,
                current_file_path,
                transformed_file: false,
            }),
            _phantom: PhantomData,
        })
    }
}

fn compute_relative_filename(
    filename: Option<&str>,
    target_directory: Option<&std::path::Path>,
) -> Option<String> {
    let filename = filename?;
    let target_directory = target_directory?;

    let file_path = std::path::Path::new(filename);
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

#[derive(Trace, Clone)]
#[class(rename_all = "camelCase")]
pub struct SgNodeRjs<'js> {
    #[qjs(skip_trace)] // Strong reference to keep root alive
    pub(crate) root: Arc<SgRootInner>,
    #[qjs(skip_trace)] // NodeMatch is not Trace
    pub(crate) inner_node: NodeMatch<'static, TSDoc>,
    #[qjs(skip_trace)]
    pub(crate) _phantom: PhantomData<&'js ()>,
}

unsafe impl<'js> JsLifetime<'js> for SgNodeRjs<'js> {
    type Changed<'to> = SgNodeRjs<'to>;
}

/// Helper to find the tightest node containing a byte range.
#[cfg(feature = "native")]
fn find_node_at_range<'a>(
    root: &'a Node<'a, TSDoc>,
    start: usize,
    end: usize,
) -> Option<Node<'a, TSDoc>> {
    let mut current = root.clone();

    // traverse down to find the tightest node containing the range
    loop {
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
                    return Some(child);
                }
                current = child;
            }
            None => {
                // no child contains the range, current is the tightest
                let current_range = current.range();
                if current_range.start <= start && current_range.end >= end {
                    return Some(current);
                }
                return None;
            }
        }
    }
}

#[methods]
impl<'js> SgNodeRjs<'js> {
    pub fn text(&self) -> Result<String> {
        Ok(self.inner_node.text().to_string())
    }

    pub fn is(&self, kind: String) -> Result<bool> {
        Ok(self.inner_node.kind() == kind)
    }

    pub fn kind(&self) -> Result<String> {
        Ok(self.inner_node.kind().to_string())
    }

    pub fn range(&self, _ctx: Ctx<'js>) -> Result<JsNodeRange> {
        let start_pos_obj = self.inner_node.start_pos();
        let end_pos_obj = self.inner_node.end_pos();
        let byte_range = self.inner_node.range();

        let result = JsNodeRange {
            start: crate::ast_grep::types::JsPosition {
                line: start_pos_obj.line(),
                column: start_pos_obj.column(&self.inner_node),
                index: byte_range.start,
            },
            end: crate::ast_grep::types::JsPosition {
                line: end_pos_obj.line(),
                column: end_pos_obj.column(&self.inner_node),
                index: byte_range.end,
            },
        };
        Ok(result)
    }

    pub fn id(&self) -> Result<usize> {
        Ok(self.inner_node.node_id())
    }

    #[qjs(rename = "isLeaf")]
    pub fn is_leaf(&self) -> Result<bool> {
        Ok(self.inner_node.is_leaf())
    }

    #[qjs(rename = "isNamed")]
    pub fn is_named(&self) -> Result<bool> {
        Ok(self.inner_node.is_named())
    }

    #[qjs(rename = "isNamedLeaf")]
    pub fn is_named_leaf(&self) -> Result<bool> {
        Ok(self.inner_node.is_named_leaf())
    }

    pub fn parent(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.inner_node.parent() {
            Some(node) => {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                let sg_node = SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                };
                sg_node.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn child(&self, nth: usize, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.inner_node.child(nth) {
            Some(node) => {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                let sg_node = SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                };
                sg_node.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn children(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(self
            .inner_node
            .children()
            .map(|node: Node<TSDoc>| {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect())
    }

    pub fn ancestors(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(self
            .inner_node
            .ancestors()
            .map(|node: Node<TSDoc>| {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect())
    }

    pub fn next(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.inner_node.next() {
            Some(node) => {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                let sg_node = SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                };
                sg_node.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "nextAll")]
    pub fn next_all(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(self
            .inner_node
            .next_all()
            .map(|node: Node<TSDoc>| {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect())
    }

    pub fn prev(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.inner_node.prev() {
            Some(node) => {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                let sg_node = SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                };
                sg_node.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "prevAll")]
    pub fn prev_all(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(self
            .inner_node
            .prev_all()
            .map(|node: Node<TSDoc>| {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect())
    }

    pub fn field(&self, name: String, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.inner_node.field(&name) {
            Some(node) => {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                let sg_node = SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                };
                sg_node.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "fieldChildren")]
    pub fn field_children(&self, name: String) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(self
            .inner_node
            .field_children(&name)
            .map(|node: Node<TSDoc>| {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect())
    }

    pub fn matches(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = get_language(&self.root.grep);
        let matcher = convert_matcher(matcher, lang, &ctx)?;
        Ok(self.inner_node.matches(&matcher))
    }

    pub fn inside(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = get_language(&self.root.grep);
        let matcher = convert_matcher(matcher, lang, &ctx)?;
        Ok(self.inner_node.inside(&matcher))
    }

    pub fn has(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = get_language(&self.root.grep);
        let matcher = convert_matcher(matcher, lang, &ctx)?;
        Ok(self.inner_node.has(&matcher))
    }

    pub fn precedes(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = get_language(&self.root.grep);
        let matcher = convert_matcher(matcher, lang, &ctx)?;
        Ok(self.inner_node.precedes(&matcher))
    }

    pub fn follows(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = get_language(&self.root.grep);
        let matcher = convert_matcher(matcher, lang, &ctx)?;
        Ok(self.inner_node.follows(&matcher))
    }

    #[qjs(rename = "getMatch")]
    pub fn get_match(&self, meta_var: String, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.inner_node.get_env().get_match(&meta_var) {
            Some(node) => {
                let node_match: NodeMatch<_> = node.clone().into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                let sg_node = SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                };
                sg_node.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "getMultipleMatches")]
    pub fn get_multiple_matches(&self, meta_var: String) -> Result<Vec<SgNodeRjs<'js>>> {
        let matches = self.inner_node.get_env().get_multiple_matches(&meta_var);
        Ok(matches
            .into_iter()
            .map(|node| {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect())
    }

    #[qjs(rename = "getTransformed")]
    pub fn get_transformed(&self, meta_var: String, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.inner_node.get_env().get_transformed(&meta_var) {
            // The transformed text, not its bytes.
            Some(bytes) => String::from_utf8_lossy(bytes).into_owned().into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn find(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let lang = get_language(&self.root.grep);
        let matcher = convert_matcher(matcher, lang, &ctx)?;
        match self.inner_node.find(&matcher) {
            Some(node_match) => {
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                let sg_node = SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                };
                sg_node.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "findAll")]
    pub fn find_all(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<Vec<SgNodeRjs<'js>>> {
        let lang = get_language(&self.root.grep);
        let matcher = convert_matcher(matcher, lang, &ctx)?;
        Ok(self
            .inner_node
            .find_all(&matcher)
            .map(|node_match| {
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect())
    }

    pub fn replace(&self, text: String) -> Result<JsEdit> {
        let byte_range = self.inner_node.range();
        Ok(JsEdit {
            start_pos: byte_range.start as u32,
            end_pos: byte_range.end as u32,
            inserted_text: text,
        })
    }

    #[qjs(rename = "commitEdits")]
    pub fn commit_edits(&self, edits: Vec<JsEdit>, ctx: Ctx<'js>) -> Result<String> {
        // tree-sitter starts the root node at its first token, so the root node's text omits
        // leading blank lines, whitespace, and a BOM. Committing against that text silently
        // dropped those bytes from every file, even when `edits` was empty. When committing on
        // the root node, splice into the full document source instead so untouched bytes are
        // preserved exactly.
        let is_root = self.inner_node.parent().is_none();
        let (old_content, offset) = if is_root {
            (document_source(&self.root.grep), 0)
        } else {
            (self.inner_node.text(), self.inner_node.range().start)
        };

        let edits = edits
            .into_iter()
            .map(|edit| {
                (
                    edit.start_pos as usize,
                    edit.end_pos as usize,
                    edit.inserted_text,
                )
            })
            .collect();
        splice_edits(&old_content, offset, edits)
            .map_err(|message| Exception::throw_message(&ctx, &message))
    }

    pub fn debug(&self) -> Result<String> {
        fn format_node<D: ast_grep_core::Doc>(
            node: &ast_grep_core::Node<D>,
            indent: usize,
        ) -> String {
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

        let node: ast_grep_core::Node<TSDoc> = self.inner_node.clone().into();
        Ok(format_node(&node, 0))
    }

    #[qjs(rename = "getRoot")]
    pub fn get_root(&self, _ctx: Ctx<'js>) -> Result<SgRootRjs<'js>> {
        Ok(SgRootRjs {
            inner: Arc::clone(&self.root),
            _phantom: PhantomData,
        })
    }

    /// Get the definition of the symbol at this node's position.
    ///
    /// Returns an object with:
    /// - `node`: The SgNode at the definition location
    /// - `root`: The SgRoot for the file containing the definition
    ///
    /// Returns null if:
    /// - No semantic provider is configured
    /// - No symbol is found at this position
    /// - The definition cannot be resolved (e.g., external symbol)
    #[qjs(rename = "definition")]
    pub fn definition(
        &self,
        ctx: Ctx<'js>,
        options: Opt<rquickjs::Object<'js>>,
    ) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = options;
            return Ok(Value::new_null(ctx));
        }
        #[cfg(feature = "native")]
        {
            let provider = match &self.root.semantic_provider {
                Some(p) => p,
                None => return Ok(Value::new_null(ctx)),
            };

            let file_path = match &self.root.filename {
                Some(f) => std::path::PathBuf::from(f),
                None => return Ok(Value::new_null(ctx)),
            };
            if let Some(message) =
                self.root
                    .edited_in_this_step(&**provider, &file_path, "definition")
            {
                return Err(Exception::throw_message(&ctx, &message));
            }

            // Parse options
            let (resolve_external, follow) = match options.0 {
                Some(opts) => (
                    opts.get::<_, Option<bool>>("resolveExternal")?
                        .unwrap_or(true),
                    opts.get::<_, Option<bool>>("follow")?.unwrap_or(false),
                ),
                None => (true, false),
            };

            let def_options = language_core::DefinitionOptions { resolve_external };

            let byte_range = self.inner_node.range();
            let range =
                language_core::ByteRange::new(byte_range.start as u32, byte_range.end as u32);

            let get = |path: &std::path::Path, range| {
                provider
                    .get_definition(path, range, def_options)
                    .map_err(|e| {
                        Exception::throw_message(&ctx, &format!("Failed to get definition: {}", e))
                    })
            };
            let mut found = get(&file_path, range)?;
            // `follow`: a use of an imported name lands on its import binding
            // in this file; ask again from there to reach the declaration.
            if follow {
                for _ in 0..8 {
                    let Some(def) = &found else { break };
                    if def.kind != language_core::DefinitionKind::Local
                        || def.location.kind != language_core::SymbolKind::Import
                    {
                        break;
                    }
                    match get(&def.location.file_path, def.location.range)? {
                        Some(next) if next.location != def.location => found = Some(next),
                        _ => break,
                    }
                }
            }
            match found {
                Some(def_result) => self.definition_object(&ctx, &file_path, &def_result),
                None => Ok(Value::new_null(ctx)),
            }
        }
    }

    /// Find all references to the symbol at this node's position.
    ///
    /// Returns an array of objects, one per file, each containing:
    /// - `root`: The SgRoot for the file
    /// - `nodes`: Array of SgNode objects for each reference in that file
    ///
    /// Returns an empty array if:
    /// - No semantic provider is configured
    /// - No symbol is found at this position
    ///
    /// In lightweight mode, this only searches files that have been processed.
    /// In accurate mode, this searches the entire workspace.
    #[qjs(rename = "references")]
    pub fn references(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            return rquickjs::Array::new(ctx.clone())?.into_js(&ctx);
        }
        #[cfg(feature = "native")]
        {
            let provider = match &self.root.semantic_provider {
                Some(p) => p,
                None => return rquickjs::Array::new(ctx.clone())?.into_js(&ctx),
            };

            let file_path = match &self.root.filename {
                Some(f) => std::path::PathBuf::from(f),
                None => return rquickjs::Array::new(ctx.clone())?.into_js(&ctx),
            };
            if let Some(message) =
                self.root
                    .edited_in_this_step(&**provider, &file_path, "references")
            {
                return Err(Exception::throw_message(&ctx, &message));
            }

            let byte_range = self.inner_node.range();
            let range =
                language_core::ByteRange::new(byte_range.start as u32, byte_range.end as u32);

            match provider.find_references(&file_path, range) {
                Ok(refs_result) => {
                    let result_array = rquickjs::Array::new(ctx.clone())?;

                    for (idx, file_refs) in refs_result.files.iter().enumerate() {
                        let is_same_file = file_refs.file_path == file_path;

                        let file_obj = rquickjs::Object::new(ctx.clone())?;

                        if is_same_file {
                            // Use existing root for same file
                            let sg_root = SgRootRjs {
                                inner: Arc::clone(&self.root),
                                _phantom: PhantomData,
                            };
                            file_obj.set("root", sg_root)?;

                            // Find nodes for each location
                            let nodes_array = rquickjs::Array::new(ctx.clone())?;
                            let root_node = self.root.grep.root();

                            for (node_idx, loc) in file_refs.locations.iter().enumerate() {
                                if let Some(node) = find_node_at_range(
                                    &root_node,
                                    loc.range.start as usize,
                                    loc.range.end as usize,
                                ) {
                                    let node_match: NodeMatch<_> = node.into();
                                    let static_node_match: NodeMatch<'static, TSDoc> =
                                        unsafe { std::mem::transmute(node_match) };

                                    let sg_node = SgNodeRjs {
                                        root: Arc::clone(&self.root),
                                        inner_node: static_node_match,
                                        _phantom: PhantomData,
                                    };
                                    nodes_array.set(node_idx, sg_node)?;
                                }
                            }
                            file_obj.set("nodes", nodes_array)?;
                        } else {
                            // Create new root for different file
                            let lang_str = detect_language_from_path(&file_refs.file_path);
                            let lang = match Lang::from_str(&lang_str) {
                                Ok(l) => l,
                                Err(_) => continue, // Skip files with unsupported languages
                            };
                            let grep = AstGrep::new(file_refs.content.clone(), lang);

                            if let Ok(new_root) = SgRootRjs::try_new_with_semantic(
                                grep,
                                Some(file_refs.file_path.to_string_lossy().to_string()),
                                self.root.semantic_provider.clone(),
                                self.root.current_file_path.clone(),
                                self.root.target_directory.as_deref(),
                            ) {
                                file_obj.set("root", new_root.clone())?;

                                // Find nodes for each location
                                let nodes_array = rquickjs::Array::new(ctx.clone())?;
                                let root_node = new_root.inner.grep.root();

                                for (node_idx, loc) in file_refs.locations.iter().enumerate() {
                                    if let Some(node) = find_node_at_range(
                                        &root_node,
                                        loc.range.start as usize,
                                        loc.range.end as usize,
                                    ) {
                                        let node_match: NodeMatch<_> = node.into();
                                        let static_node_match: NodeMatch<'static, TSDoc> =
                                            unsafe { std::mem::transmute(node_match) };

                                        let sg_node = SgNodeRjs {
                                            root: Arc::clone(&new_root.inner),
                                            inner_node: static_node_match,
                                            _phantom: PhantomData,
                                        };
                                        nodes_array.set(node_idx, sg_node)?;
                                    }
                                }
                                file_obj.set("nodes", nodes_array)?;
                            } else {
                                // Skip files we can't parse
                                continue;
                            }
                        }

                        result_array.set(idx, file_obj)?;
                    }

                    result_array.into_js(&ctx)
                }
                Err(e) => Err(Exception::throw_message(
                    &ctx,
                    &format!("Failed to find references: {}", e),
                )),
            }
        }
    }

    /// A string naming this node across answers and roots: its file, range
    /// and kind. Two nodes with the same key are the same syntax node, even
    /// when they come from different queries (which wrap nodes in new
    /// objects, and other files in new roots). Use it in a Set or Map.
    #[qjs(rename = "key")]
    pub fn key(&self) -> String {
        let range = self.inner_node.range();
        format!(
            "{}:{}-{}:{}",
            self.root.canonical_filename(),
            range.start,
            range.end,
            self.inner_node.kind()
        )
    }

    /// The node in JSON, as in a step's result: its key. A later step makes
    /// it a node again with `root.nodeByKey(key)`, or passes the key where
    /// an analysis takes a node.
    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String {
        self.key()
    }

    /// Whether `other` is this node or inside it (same file, its range within
    /// this one's), even when it comes from another query or root. False for
    /// anything but an SgNode.
    #[qjs(rename = "contains")]
    pub fn contains(&self, other: Value<'js>) -> bool {
        rquickjs::Class::<SgNodeRjs>::from_value(&other).is_ok_and(|other| {
            let other = other.borrow();
            let (outer, inner) = (self.inner_node.range(), other.inner_node.range());
            self.root.canonical_filename() == other.root.canonical_filename()
                && outer.start <= inner.start
                && inner.end <= outer.end
        })
    }

    /// Whether `other` is the same syntax node (same file, range and kind),
    /// even from another query or root. False for anything but an SgNode.
    #[qjs(rename = "equals")]
    pub fn equals(&self, other: Value<'js>) -> bool {
        rquickjs::Class::<SgNodeRjs>::from_value(&other)
            .is_ok_and(|other| other.borrow().key() == self.key())
    }

    /// Whether this node's value is computed from `source`: a node, or a
    /// predicate over the steps of `flowsFrom(options)`.
    #[qjs(rename = "dependsOn")]
    pub fn depends_on(
        &self,
        ctx: Ctx<'js>,
        source: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<bool> {
        let steps = self.data_flow(ctx.clone(), options, true)?;
        any_step(&ctx, "dependsOn", steps, source)
    }

    /// Whether this node's value reaches `target`: a node, or a predicate
    /// over the steps of `flowsTo(options)`.
    #[qjs(rename = "reaches")]
    pub fn reaches(
        &self,
        ctx: Ctx<'js>,
        target: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<bool> {
        let steps = self.data_flow(ctx.clone(), options, false)?;
        any_step(&ctx, "reaches", steps, target)
    }

    /// Follow this node's value backward: what it is computed from.
    ///
    /// Returns an array of steps, nearest first. Each step has the `node` and
    /// its `root`, the node's `kind`, the edge that reached it (`via`), and
    /// `boundary: true` where the value comes from outside the module's own
    /// code (a parameter, an import, a global, a call result).
    ///
    /// Empty when no semantic provider is configured.
    #[qjs(rename = "flowsFrom")]
    pub fn flows_from(&self, ctx: Ctx<'js>, options: Opt<Value<'js>>) -> Result<Value<'js>> {
        self.data_flow(ctx, options, true)
    }

    /// Follow this node's value forward: the bindings, calls, returns, property
    /// writes and JSX attributes it reaches.
    ///
    /// Returns the same step objects as `flowsFrom()`. A step with
    /// `via: "argument"` is a call that receives the value (`index` is the
    /// argument position).
    #[qjs(rename = "flowsTo")]
    pub fn flows_to(&self, ctx: Ctx<'js>, options: Opt<Value<'js>>) -> Result<Value<'js>> {
        self.data_flow(ctx, options, false)
    }

    /// Functions called from the function (or by the call) at this node.
    ///
    /// Returns an array of `{ node, root, name, resolved, via, depth, callSite }`.
    /// A call whose callee is not a function in the workspace comes back with
    /// `resolved: false` and the call as its `node`.
    #[qjs(rename = "callees")]
    pub fn callees(&self, ctx: Ctx<'js>, options: Opt<Value<'js>>) -> Result<Value<'js>> {
        self.call_graph(ctx, options, true)
    }

    /// Functions that call the function at this node. Needs workspace scope
    /// to see callers in other files.
    #[qjs(rename = "callers")]
    pub fn callers(&self, ctx: Ctx<'js>, options: Opt<Value<'js>>) -> Result<Value<'js>> {
        self.call_graph(ctx, options, false)
    }

    /// Why the function at this node is called from outside the workspace's
    /// code: its package exports it (`{ kind: "export", label:
    /// "@acme/ui#Button" }`), or a rule in the selector's
    /// `semanticAnalysis.entryPoints` declares it (`{ kind: "http", label:
    /// "GET app/api/users/[id]/route.ts" }`). `[]` when neither applies.
    /// `callers()` never includes these. Needs workspace scope.
    #[qjs(rename = "entryPoints")]
    pub fn entry_points(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            return Err(Exception::throw_message(&ctx, &not_in_wasm("entryPoints")));
        }
        #[cfg(feature = "native")]
        {
            let (provider, file_path, range) = self
                .semantic_query("entryPoints")
                .map_err(|message| Exception::throw_message(&ctx, &message))?;
            let found = provider.get_entry_points(&file_path, range).map_err(|e| {
                self.query_error(&ctx, "entryPoints", "Failed to find entry points", e)
            })?;
            let result = rquickjs::Array::new(ctx.clone())?;
            for (index, entry) in found.iter().enumerate() {
                let object = rquickjs::Object::new(ctx.clone())?;
                object.set("kind", entry.kind.clone())?;
                object.set("label", entry.label.clone())?;
                result.set(index, object)?;
            }
            result.into_js(&ctx)
        }
    }

    /// Places that change the variable at this node, or the object it holds.
    ///
    /// `options.after` (an SgNode in the same file) keeps only the mutations
    /// that can run after that node. Returns an array of
    /// `{ node, root, kind, confidence, name, deferred, viaAlias }`.
    #[qjs(rename = "mutations")]
    pub fn mutations(&self, ctx: Ctx<'js>, options: Opt<Value<'js>>) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = options;
            return Err(Exception::throw_message(&ctx, &not_in_wasm("mutations")));
        }
        #[cfg(feature = "native")]
        {
            let result = rquickjs::Array::new(ctx.clone())?;
            let (provider, file_path, range) = self
                .semantic_query("mutations")
                .map_err(|message| Exception::throw_message(&ctx, &message))?;

            let mut after = None;
            if let Some(opts) = query_options(&ctx, options, "mutations", &["after"])? {
                let value: Value = opts.get("after")?;
                if !value.is_undefined() && !value.is_null() {
                    let Ok(node) = rquickjs::Class::<SgNodeRjs>::from_value(&value) else {
                        return Err(Exception::throw_type(
                            &ctx,
                            "mutations({ after }): `after` must be an SgNode. For a flow step or \
                             a call-graph entry, pass its `.node`.",
                        ));
                    };
                    let node = node.borrow();
                    let same_file = Arc::ptr_eq(&node.root, &self.root)
                        || (node.root.filename.is_some()
                            && node.root.filename == self.root.filename
                            && node.root.grep.root().text() == self.root.grep.root().text());
                    if !same_file {
                        return Err(Exception::throw_range(
                            &ctx,
                            &format!(
                                "mutations({{ after }}): `after` is in {}, not in this node's \
                                 file ({}). Mutations are found within one file.",
                                node.root.filename.as_deref().unwrap_or("a string"),
                                self.root.filename.as_deref().unwrap_or("?"),
                            ),
                        ));
                    }
                    let node_range = node.inner_node.range();
                    after = Some(language_core::ByteRange::new(
                        node_range.start as u32,
                        node_range.end as u32,
                    ));
                }
            }

            let found = provider
                .get_mutations(&file_path, range, language_core::MutationOptions { after })
                .map_err(|e| self.query_error(&ctx, "mutations", "Failed to find mutations", e))?;
            let mut count = 0;
            for mutation in &found.mutations {
                let Some(node) = Self::node_in(&self.root, mutation.range) else {
                    continue;
                };
                let object = rquickjs::Object::new(ctx.clone())?;
                object.set("node", node)?;
                object.set(
                    "root",
                    SgRootRjs {
                        inner: Arc::clone(&self.root),
                        _phantom: PhantomData,
                    },
                )?;
                object.set("kind", mutation.kind.as_str())?;
                object.set("confidence", mutation.confidence.as_str())?;
                object.set("name", or_null(&ctx, mutation.name.clone())?)?;
                object.set("deferred", mutation.deferred)?;
                object.set("viaAlias", mutation.via_alias)?;
                result.set(count, object)?;
                count += 1;
            }
            result.into_js(&ctx)
        }
    }

    /// The type of the expression at this node's range, printed by the
    /// TypeScript 7 checker. Experimental; JavaScript/TypeScript in workspace
    /// scope with types on (`types: "ts7"`).
    ///
    /// Returns null if:
    /// - No semantic provider is configured
    /// - Types are off, or no expression has exactly this range
    #[qjs(rename = "typeInfo")]
    pub fn type_info(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            return Ok(Value::new_null(ctx));
        }
        #[cfg(feature = "native")]
        {
            let provider = match &self.root.semantic_provider {
                Some(p) => p,
                None => return Ok(Value::new_null(ctx)),
            };

            let file_path = match &self.root.filename {
                Some(f) => std::path::PathBuf::from(f),
                None => return Ok(Value::new_null(ctx)),
            };
            if let Some(message) =
                self.root
                    .edited_in_this_step(&**provider, &file_path, "typeInfo")
            {
                return Err(Exception::throw_message(&ctx, &message));
            }

            let byte_range = self.inner_node.range();
            let range =
                language_core::ByteRange::new(byte_range.start as u32, byte_range.end as u32);

            match provider.get_type(&file_path, range) {
                Ok(Some(type_str)) => type_str.into_js(&ctx),
                Ok(None) => Ok(Value::new_null(ctx)),
                Err(e) => Err(Exception::throw_message(
                    &ctx,
                    &format!("Failed to get type: {}", e),
                )),
            }
        }
    }
}

/// The data-flow and call-graph queries behind `flowsFrom()`, `flowsTo()`,
/// `callees()` and `callers()`.
impl<'js> SgNodeRjs<'js> {
    fn data_flow(
        &self,
        ctx: Ctx<'js>,
        options: Opt<Value<'js>>,
        backward: bool,
    ) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = options;
            let method = if backward { "flowsFrom" } else { "flowsTo" };
            return Err(Exception::throw_message(&ctx, &not_in_wasm(method)));
        }
        #[cfg(feature = "native")]
        {
            let result = rquickjs::Array::new(ctx.clone())?;
            let (provider, file_path, range) = self
                .semantic_query(if backward { "flowsFrom" } else { "flowsTo" })
                .map_err(|message| Exception::throw_message(&ctx, &message))?;

            let mut flow_options = language_core::FlowOptions {
                direction: if backward {
                    language_core::FlowDirection::Backward
                } else {
                    language_core::FlowDirection::Forward
                },
                ..Default::default()
            };
            let method = if backward { "flowsFrom" } else { "flowsTo" };
            if let Some(opts) = query_options(
                &ctx,
                options,
                method,
                &[
                    "throughCalls",
                    "maxDepth",
                    "stopAt",
                    "interprocedural",
                    "maxCallers",
                ],
            )? {
                if let Some(through_calls) = opts.get::<_, Option<bool>>("throughCalls")? {
                    flow_options.through_calls = through_calls;
                }
                if let Some(interprocedural) = opts.get::<_, Option<bool>>("interprocedural")? {
                    flow_options.interprocedural = interprocedural;
                }
                flow_options.max_callers = whole_number(&ctx, &opts, method, "maxCallers")?;
                flow_options.max_depth = max_depth(&ctx, &opts, method)?;
                flow_options.stop_at = stop_at(
                    &ctx,
                    &opts,
                    method,
                    language_core::FlowEdgeKind::ALL,
                    language_core::FlowEdgeKind::as_str,
                )?;
            }

            let flow = match provider.get_data_flow(&file_path, range, flow_options) {
                Ok(flow) => flow,
                // An expression the analysis does not track (in a type
                // position: `typeof import("./x")`, `x as T`'s type) has no
                // flow: `[]`, not an error. Statements and declarations throw.
                Err(language_core::SemanticError::NotApplicable { .. })
                    if is_expression_kind(&self.inner_node.kind()) =>
                {
                    language_core::FlowResult::default()
                }
                Err(e) => {
                    return Err(self.query_error(&ctx, method, "Failed to follow data flow", e))
                }
            };

            let mut roots = Vec::new();
            let mut steps = Vec::new();
            for (file_index, file) in flow.files.iter().enumerate() {
                let Some(root) = self.root_for(&mut roots, &file.file_path, &file.content) else {
                    continue;
                };
                for (step_index, step) in file.steps.iter().enumerate() {
                    if let Some(node) = Self::node_in(&root, step.range) {
                        steps.push(((file_index, step_index), step, node, Arc::clone(&root)));
                    }
                }
            }
            steps.sort_by_key(|(_, step, _, _)| step.depth);

            // `from` points at another step object: make them all first.
            let mut objects = std::collections::HashMap::new();
            let mut links = Vec::new();
            for (index, (at, step, node, root)) in steps.into_iter().enumerate() {
                let object = rquickjs::Object::new(ctx.clone())?;
                objects.insert(at, object.clone());
                links.push((
                    object.clone(),
                    step.from.map(|from| {
                        let file = step.from_file.map_or(at.0, |file| file as usize);
                        (file, from as usize)
                    }),
                ));
                object.set("node", node)?;
                object.set(
                    "root",
                    SgRootRjs {
                        inner: root,
                        _phantom: PhantomData,
                    },
                )?;
                object.set("kind", step.kind.as_str())?;
                object.set("via", step.via.as_str())?;
                object.set("name", or_null(&ctx, step.name.clone())?)?;
                object.set("index", or_null(&ctx, step.index)?)?;
                object.set("depth", step.depth)?;
                object.set("boundary", step.boundary)?;
                result.set(index, object)?;
            }
            for (object, from) in links {
                match from.and_then(|at| objects.get(&at)) {
                    Some(previous) => object.set("from", previous.clone())?,
                    None => object.set("from", Value::new_null(ctx.clone()))?,
                }
            }
            result.as_object().set("truncated", flow.truncated)?;
            result.into_js(&ctx)
        }
    }

    fn call_graph(
        &self,
        ctx: Ctx<'js>,
        options: Opt<Value<'js>>,
        callees: bool,
    ) -> Result<Value<'js>> {
        #[cfg(not(feature = "native"))]
        {
            let _ = options;
            let method = if callees { "callees" } else { "callers" };
            return Err(Exception::throw_message(&ctx, &not_in_wasm(method)));
        }
        #[cfg(feature = "native")]
        {
            let result = rquickjs::Array::new(ctx.clone())?;
            let (provider, file_path, range) = self
                .semantic_query(if callees { "callees" } else { "callers" })
                .map_err(|message| Exception::throw_message(&ctx, &message))?;

            let mut graph_options = language_core::CallGraphOptions {
                direction: if callees {
                    language_core::CallGraphDirection::Callees
                } else {
                    language_core::CallGraphDirection::Callers
                },
                ..Default::default()
            };
            let method = if callees { "callees" } else { "callers" };
            if let Some(opts) =
                query_options(&ctx, options, method, &["transitive", "maxDepth", "stopAt"])?
            {
                graph_options.transitive =
                    opts.get::<_, Option<bool>>("transitive")?.unwrap_or(false);
                graph_options.max_depth = max_depth(&ctx, &opts, method)?;
                graph_options.stop_at = stop_at(
                    &ctx,
                    &opts,
                    method,
                    language_core::CallEdgeKind::ALL,
                    language_core::CallEdgeKind::as_str,
                )?;
            }

            let graph = provider
                .get_call_graph(&file_path, range, graph_options)
                .map_err(|e| self.query_error(&ctx, method, "Failed to walk the call graph", e))?;

            // Call sites can sit in files that have no entry of their own.
            let contents: std::collections::HashMap<&std::path::Path, &str> = graph
                .files
                .iter()
                .map(|file| (file.file_path.as_path(), file.content.as_str()))
                .collect();
            let mut roots = Vec::new();
            let mut entries = Vec::new();
            for file in &graph.files {
                let Some(root) = self.root_for(&mut roots, &file.file_path, &file.content) else {
                    continue;
                };
                for entry in &file.entries {
                    if let Some(node) = Self::node_in(&root, entry.range) {
                        entries.push((entry, node, Arc::clone(&root)));
                    }
                }
            }
            entries.sort_by_key(|(entry, _, _)| entry.depth);

            for (index, (entry, node, root)) in entries.into_iter().enumerate() {
                let object = rquickjs::Object::new(ctx.clone())?;
                object.set("node", node)?;
                object.set(
                    "root",
                    SgRootRjs {
                        inner: root,
                        _phantom: PhantomData,
                    },
                )?;
                object.set("name", or_null(&ctx, entry.name.clone())?)?;
                object.set("resolved", entry.resolved)?;
                object.set("via", entry.via.as_str())?;
                object.set("depth", entry.depth)?;
                // Types on: where the checker says an unresolved call's
                // member is declared (a library `.d.ts`, an interface).
                if let Some(declaration) = &entry.declaration {
                    let found = (|| {
                        let content = match contents.get(declaration.file_path.as_path()) {
                            Some(content) => (*content).to_string(),
                            None => std::fs::read_to_string(&declaration.file_path).ok()?,
                        };
                        let root = self.root_for(&mut roots, &declaration.file_path, &content)?;
                        let node = Self::node_in(&root, declaration.range)?;
                        Some((node, root))
                    })();
                    if let Some((node, root)) = found {
                        let target = rquickjs::Object::new(ctx.clone())?;
                        target.set("node", node)?;
                        target.set(
                            "root",
                            SgRootRjs {
                                inner: root,
                                _phantom: PhantomData,
                            },
                        )?;
                        target.set("name", or_null(&ctx, declaration.name.clone())?)?;
                        object.set("declaration", target)?;
                    }
                }

                let call_site = entry.call_site.as_ref().and_then(|(path, call_range)| {
                    let content = match contents.get(path.as_path()) {
                        Some(content) => (*content).to_string(),
                        None => std::fs::read_to_string(path).ok()?,
                    };
                    let root = self.root_for(&mut roots, path, &content)?;
                    let node = Self::node_in(&root, *call_range)?;
                    Some((node, root))
                });
                if !callees {
                    let callee = entry.callee.as_ref().and_then(|callee| {
                        let content = match contents.get(callee.file_path.as_path()) {
                            Some(content) => (*content).to_string(),
                            None => std::fs::read_to_string(&callee.file_path).ok()?,
                        };
                        let root = self.root_for(&mut roots, &callee.file_path, &content)?;
                        let node = Self::node_in(&root, callee.range)?;
                        Some((node, root, callee.name.clone()))
                    });
                    match callee {
                        Some((node, root, name)) => {
                            let callee = rquickjs::Object::new(ctx.clone())?;
                            callee.set("node", node)?;
                            callee.set(
                                "root",
                                SgRootRjs {
                                    inner: root,
                                    _phantom: PhantomData,
                                },
                            )?;
                            callee.set("name", or_null(&ctx, name)?)?;
                            object.set("callee", callee)?;
                        }
                        None => object.set("callee", Value::new_null(ctx.clone()))?,
                    }
                }
                match call_site {
                    Some((node, root)) => {
                        let site = rquickjs::Object::new(ctx.clone())?;
                        site.set("node", node)?;
                        site.set(
                            "root",
                            SgRootRjs {
                                inner: root,
                                _phantom: PhantomData,
                            },
                        )?;
                        object.set("callSite", site)?;
                    }
                    None => {
                        // The call graph always records the call; only an
                        // unreadable file can lose it.
                        let path = entry
                            .call_site
                            .as_ref()
                            .map(|(path, _)| path.display().to_string())
                            .unwrap_or_default();
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!(
                                "{}(): could not read {} to find the call for `{}`",
                                if callees { "callees" } else { "callers" },
                                path,
                                entry.name.as_deref().unwrap_or("?"),
                            ),
                        ));
                    }
                }
                result.set(index, object)?;
            }
            result.as_object().set("truncated", graph.truncated)?;
            let unparsed = rquickjs::Array::new(ctx.clone())?;
            for (index, file) in graph.unparsed.iter().enumerate() {
                let object = rquickjs::Object::new(ctx.clone())?;
                object.set("path", file.file_path.to_string_lossy().to_string())?;
                object.set("message", file.message.clone())?;
                unparsed.set(index, object)?;
            }
            result.as_object().set("unparsed", unparsed)?;
            result.into_js(&ctx)
        }
    }
}

#[cfg(feature = "native")]
impl<'js> SgNodeRjs<'js> {
    /// The object `definition()` returns. See [`definition_value`].
    fn definition_object(
        &self,
        ctx: &Ctx<'js>,
        file_path: &std::path::Path,
        def_result: &language_core::DefinitionResult,
    ) -> Result<Value<'js>> {
        definition_value(ctx, &self.root, file_path, def_result)
    }

    /// The provider, file and byte range a data-flow or call-graph query
    /// about this node needs, or why the query cannot run here and what to do.
    ///
    /// These queries throw rather than answer `[]`: an empty answer reads as
    /// "no callers" or "no sources", which a codemod may act on.
    fn semantic_query(
        &self,
        method: &str,
    ) -> std::result::Result<
        (
            Arc<dyn SemanticProvider>,
            std::path::PathBuf,
            language_core::ByteRange,
        ),
        String,
    > {
        let lang = get_language(&self.root.grep);
        let supported = matches!(
            lang,
            Lang::Static(
                ast_grep_language::SupportLang::JavaScript
                    | ast_grep_language::SupportLang::TypeScript
                    | ast_grep_language::SupportLang::Tsx
                    | ast_grep_language::SupportLang::Python
            )
        );
        if !supported {
            return Err(format!(
                "{method}() supports JavaScript, TypeScript and Python only, and this file is parsed as {lang}."
            ));
        }
        let Some(filename) = self.root.filename.as_ref() else {
            return Err(format!(
                "{method}() needs the file this node is in, and its root has no path: it was \
                 parsed from a string. Query a root the step parsed from a file (in forEachFile, \
                 `parse(file)`)."
            ));
        };
        let Some(provider) = self.root.semantic_provider.clone() else {
            return Err(format!(
                "{method}() needs semantic analysis, and it is off for this code. Set \
                 `semanticAnalysis: \"file\"` on the jssg step or the forEachFile selector \
                 (`\"workspace\"` to follow calls across files), or `semantic_analysis: file` \
                 in workflow.yaml. With `codemod jssg run`, pass `--semantic-workspace <dir>` \
                 for workspace scope."
            ));
        };
        let file_path = std::path::PathBuf::from(filename);
        if let Some(message) = self
            .root
            .edited_in_this_step(&*provider, &file_path, method)
        {
            return Err(message);
        }
        let range = self.inner_node.range();
        Ok((
            provider,
            file_path,
            language_core::ByteRange::new(range.start as u32, range.end as u32),
        ))
    }

    /// The root for a file a semantic answer points into: this node's root
    /// for its own file, otherwise one parsed from `content` and kept in
    /// `roots`, so every node of the answer in that file shares it.
    fn root_for(
        &self,
        roots: &mut Vec<(std::path::PathBuf, Arc<SgRootInner>)>,
        file_path: &std::path::Path,
        content: &str,
    ) -> Option<Arc<SgRootInner>> {
        let own = self.root.filename.as_ref().is_some_and(|name| {
            let own_path = std::path::Path::new(name);
            own_path == file_path || own_path.canonicalize().is_ok_and(|p| p == file_path)
        });
        if own {
            return Some(Arc::clone(&self.root));
        }
        if let Some((_, root)) = roots.iter().find(|(path, _)| path == file_path) {
            return Some(Arc::clone(root));
        }
        let lang = Lang::from_str(&detect_language_from_path(file_path)).ok()?;
        let root = SgRootRjs::try_new_with_semantic(
            AstGrep::new(content, lang),
            Some(file_path.to_string_lossy().to_string()),
            self.root.semantic_provider.clone(),
            self.root.current_file_path.clone(),
            self.root.target_directory.as_deref(),
        )
        .ok()?;
        roots.push((file_path.to_path_buf(), Arc::clone(&root.inner)));
        Some(root.inner)
    }

    /// The exception for a failed query: a query that does not apply to this
    /// node names the node's kind and what to ask instead (a TypeError);
    /// anything else keeps the provider's message.
    fn query_error(
        &self,
        ctx: &Ctx<'js>,
        method: &str,
        failed: &str,
        error: language_core::SemanticError,
    ) -> rquickjs::Error {
        match error {
            language_core::SemanticError::NotApplicable { reason } => Exception::throw_type(
                ctx,
                &format!("{method}() on `{}`: {reason}.", self.inner_node.kind()),
            ),
            other => Exception::throw_message(ctx, &format!("{failed}: {other}")),
        }
    }

    /// The nearest ancestor of this kind.
    fn ancestor_of_kind(&self, kind: &str) -> Option<Self> {
        let mut current = self.inner_node.get_node().parent();
        while let Some(node) = current {
            if node.kind() == kind {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                return Some(SgNodeRjs {
                    root: Arc::clone(&self.root),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                });
            }
            current = node.parent();
        }
        None
    }

    /// The innermost syntax node with this byte range, or the tightest one around it.
    fn node_in(root: &Arc<SgRootInner>, range: language_core::ByteRange) -> Option<Self> {
        let (start, end) = (range.start as usize, range.end as usize);
        let mut current = root.grep.root();
        let top = current.range();
        if top.start > start || top.end < end {
            return None;
        }
        loop {
            let inner = current.children().find(|child| {
                let child_range = child.range();
                child_range.start <= start && child_range.end >= end
            });
            match inner {
                Some(child) => current = child,
                None => break,
            }
        }
        let node_match: NodeMatch<_> = current.into();
        let static_node_match: NodeMatch<'static, TSDoc> =
            unsafe { std::mem::transmute(node_match) };
        Some(SgNodeRjs {
            root: Arc::clone(root),
            inner_node: static_node_match,
            _phantom: PhantomData,
        })
    }
}

/// What `codemod/ast-grep`'s analyses (`analysis.rs`) need from nodes and roots.
#[cfg(feature = "native")]
impl<'js> SgNodeRjs<'js> {
    /// The language this node's file is parsed as, as `parse` names it.
    pub(crate) fn language_name(&self) -> String {
        get_language(&self.root.grep).to_string()
    }

    /// The innermost node with this byte range in `root`.
    pub(crate) fn node_at(
        root: &Arc<SgRootInner>,
        range: language_core::ByteRange,
    ) -> Option<Self> {
        Self::node_in(root, range)
    }

    /// A node for a match in `root`.
    pub(crate) fn from_match(root: &Arc<SgRootInner>, node_match: NodeMatch<'_, TSDoc>) -> Self {
        let static_node_match: NodeMatch<'static, TSDoc> =
            unsafe { std::mem::transmute(node_match) };
        SgNodeRjs {
            root: Arc::clone(root),
            inner_node: static_node_match,
            _phantom: PhantomData,
        }
    }

    /// The text a metavariable (`P` for `$P`) captured in this match.
    pub(crate) fn captured_text(&self, name: &str) -> Option<String> {
        self.inner_node
            .get_env()
            .get_match(name)
            .map(|node| node.text().to_string())
    }

    /// The node a metavariable (`P` for `$P`) captured in this match.
    pub(crate) fn captured_node(&self, name: &str) -> Option<Self> {
        let node = self.inner_node.get_env().get_match(name)?.clone();
        Some(Self::from_match(&self.root, node.into()))
    }

    /// The nearest function around this node, or the node itself when it is one.
    pub(crate) fn enclosing_function(&self) -> Option<Self> {
        const FUNCTIONS: &[&str] = &[
            "function_declaration",
            "generator_function_declaration",
            "function_expression",
            "function",
            "generator_function",
            "arrow_function",
            "method_definition",
            "function_definition",
            "lambda",
        ];
        if FUNCTIONS.contains(&&*self.inner_node.kind()) {
            return Some(self.clone());
        }
        let mut current = self.inner_node.get_node().parent();
        while let Some(node) = current {
            if FUNCTIONS.contains(&&*node.kind()) {
                return Some(Self::from_match(&self.root, node.into()));
            }
            current = node.parent();
        }
        None
    }

    /// Every match of `pattern` in this root's file.
    pub(crate) fn matches_in(
        root: &Arc<SgRootInner>,
        pattern: &crate::ast_grep::utils::JsMatcherRjs,
    ) -> Vec<Self> {
        root.grep
            .root()
            .find_all(pattern)
            .map(|node_match| Self::from_match(root, node_match))
            .collect()
    }
}

/// The parts of a `node.key()`: file, start, end and kind.
pub(crate) fn parse_node_key(key: &str) -> Option<(&str, usize, usize, &str)> {
    let (rest, kind) = key.rsplit_once(':')?;
    let (file, range) = rest.rsplit_once(':')?;
    let (start, end) = range.split_once('-')?;
    Some((file, start.parse().ok()?, end.parse().ok()?, kind))
}

impl SgNodeRjs<'_> {
    /// The node in `root` with this range and kind: the innermost node with
    /// the range, then outward through nodes with the same range (`f()` and
    /// its expression statement) to the kind.
    pub(crate) fn node_with(
        root: &Arc<SgRootInner>,
        start: usize,
        end: usize,
        kind: &str,
    ) -> Option<Self> {
        let mut current = root.grep.root();
        loop {
            let inner = current.children().find(|child| {
                let r = child.range();
                r.start <= start && r.end >= end
            });
            match inner {
                Some(child) => current = child,
                None => break,
            }
        }
        let mut candidate = Some(current);
        while let Some(node) = candidate {
            let r = node.range();
            if r.start != start || r.end != end {
                return None;
            }
            if node.kind() == kind {
                let node_match: NodeMatch<_> = node.into();
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                return Some(SgNodeRjs {
                    root: Arc::clone(root),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                });
            }
            candidate = node.parent();
        }
        None
    }
}

/// A root for a file the analyses read from disk, with the step's semantic
/// analysis, so its nodes answer `callers()`, `callees()` and the rest.
#[cfg(feature = "native")]
pub(crate) fn root_for_path(
    provider: Option<Arc<dyn SemanticProvider>>,
    file_path: &std::path::Path,
    target_directory: Option<&std::path::Path>,
) -> Option<Arc<SgRootInner>> {
    let content = std::fs::read_to_string(file_path).ok()?;
    root_for_text(provider, file_path, content, target_directory)
}

/// [`root_for_path`], for the file's text as the caller has it (in a dry
/// run, as earlier steps left it).
#[cfg(feature = "native")]
pub(crate) fn root_for_text(
    provider: Option<Arc<dyn SemanticProvider>>,
    file_path: &std::path::Path,
    content: String,
    target_directory: Option<&std::path::Path>,
) -> Option<Arc<SgRootInner>> {
    let lang = Lang::from_str(&detect_language_from_path(file_path)).ok()?;
    let root = SgRootRjs::try_new_with_semantic(
        AstGrep::new(content, lang),
        Some(file_path.to_string_lossy().to_string()),
        provider,
        None,
        target_directory,
    )
    .ok()?;
    Some(root.inner)
}

/// The object `definition()` returns: `node`, `root`, `kind`, `exportName`
/// and `nameNode`. `own_root` answers for its own file; other files get a
/// new root. Null when the range has no node.
#[cfg(feature = "native")]
fn definition_value<'js>(
    ctx: &Ctx<'js>,
    own_root: &Arc<SgRootInner>,
    file_path: &std::path::Path,
    def_result: &language_core::DefinitionResult,
) -> Result<Value<'js>> {
    let loc = &def_result.location;
    let root = if loc.file_path == file_path {
        Arc::clone(own_root)
    } else {
        let lang_str = detect_language_from_path(&loc.file_path);
        let lang = Lang::from_str(&lang_str)
            .map_err(|e| Exception::throw_message(ctx, &format!("Unsupported language: {}", e)))?;
        let grep = AstGrep::new(def_result.content.clone(), lang);
        match SgRootRjs::try_new_with_semantic(
            grep,
            Some(loc.file_path.to_string_lossy().to_string()),
            own_root.semantic_provider.clone(),
            own_root.current_file_path.clone(),
            own_root.target_directory.as_deref(),
        ) {
            Ok(new_root) => new_root.inner,
            Err(_) => return Ok(Value::new_null(ctx.clone())),
        }
    };
    let wrap = |node: Node<'_, TSDoc>| {
        let node_match: NodeMatch<_> = node.into();
        let static_node_match: NodeMatch<'static, TSDoc> =
            unsafe { std::mem::transmute(node_match) };
        SgNodeRjs {
            root: Arc::clone(&root),
            inner_node: static_node_match,
            _phantom: PhantomData,
        }
    };
    let root_node = root.grep.root();
    let Some(node) =
        find_node_at_range(&root_node, loc.range.start as usize, loc.range.end as usize)
    else {
        return Ok(Value::new_null(ctx.clone()));
    };
    // The declared name: the node itself for a binding, the identifier
    // inside it for a declaration or `export default function f`.
    // A default export's symbol is named "default": fall back to the
    // `name` field of the declaration inside.
    let name_node = node
        .dfs()
        .find(|n| n.kind().ends_with("identifier") && n.text() == loc.name.as_str())
        .or_else(|| {
            node.dfs()
                .find_map(|n| n.field("name"))
                .filter(|n| n.kind().ends_with("identifier"))
        });

    let result_obj = rquickjs::Object::new(ctx.clone())?;
    result_obj.set("node", wrap(node))?;
    result_obj.set("nameNode", or_null(ctx, name_node.map(wrap))?)?;
    result_obj.set(
        "root",
        SgRootRjs {
            inner: Arc::clone(&root),
            _phantom: PhantomData,
        },
    )?;
    result_obj.set("kind", def_result.kind.as_str())?;
    result_obj.set("exportName", or_null(ctx, def_result.export_name.clone())?)?;
    result_obj.into_js(ctx)
}

/// Whether a step of `steps` matches `target`: an SgNode (the step's node
/// is that node) or a predicate called with each step.
fn any_step<'js>(
    ctx: &Ctx<'js>,
    method: &str,
    steps: Value<'js>,
    target: Value<'js>,
) -> Result<bool> {
    let Some(steps) = steps.into_array() else {
        return Ok(false);
    };
    if let Some(predicate) = target.as_function() {
        for step in steps.iter::<Value>() {
            let step = step?;
            let hit: Value = predicate.call((step,))?;
            if hit.as_bool().unwrap_or(false)
                || hit.as_int().is_some_and(|n| n != 0)
                || (hit.is_object() && !hit.is_null())
            {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let Ok(node) = rquickjs::Class::<SgNodeRjs>::from_value(&target) else {
        return Err(Exception::throw_type(
            ctx,
            &format!("{method}(target): target must be an SgNode or a function of a step"),
        ));
    };
    let key = node.borrow().key();
    for step in steps.iter::<rquickjs::Object>() {
        let value: Value = step?.get("node")?;
        if rquickjs::Class::<SgNodeRjs>::from_value(&value).is_ok_and(|n| n.borrow().key() == key) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a node kind is an expression in JS/TS or Python syntax: a query
/// on one the analysis does not track answers `[]` rather than throwing.
#[cfg(feature = "native")]
fn is_expression_kind(kind: &str) -> bool {
    kind.ends_with("_expression")
        || matches!(
            kind,
            "identifier"
                | "property_identifier"
                | "shorthand_property_identifier"
                | "this"
                | "super"
                | "string"
                | "template_string"
                | "number"
                | "true"
                | "false"
                | "null"
                | "undefined"
                | "regex"
                | "array"
                | "object"
                // Python
                | "call"
                | "attribute"
                | "subscript"
                | "integer"
                | "float"
                | "none"
                | "list"
                | "dictionary"
                | "tuple"
                | "set"
                | "concatenated_string"
                | "lambda"
        )
}

/// `null`, not `undefined`, for a missing value: the public types say
/// `string | null`, and `=== null` checks must work.
#[cfg(feature = "native")]
fn or_null<'js, T: IntoJs<'js>>(ctx: &Ctx<'js>, value: Option<T>) -> Result<Value<'js>> {
    match value {
        Some(value) => value.into_js(ctx),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

/// The options object of a data-flow, call-graph or mutation query. Throws on
/// something that is not an object, and on a key the query does not know
/// (a typo there would otherwise silently change the answer).
#[cfg(feature = "native")]
fn query_options<'js>(
    ctx: &Ctx<'js>,
    options: Opt<Value<'js>>,
    method: &str,
    known: &[&str],
) -> Result<Option<rquickjs::Object<'js>>> {
    let Some(value) = options.0 else {
        return Ok(None);
    };
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    let Some(object) = value.into_object() else {
        return Err(Exception::throw_type(
            ctx,
            &format!("{method}(options): options must be an object"),
        ));
    };
    for key in object.keys::<String>() {
        let key = key?;
        if !known.contains(&key.as_str()) {
            return Err(Exception::throw_type(
                ctx,
                &format!(
                    "{method}(options): unknown option `{key}`; expected {}",
                    known
                        .iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
            ));
        }
    }
    Ok(Some(object))
}

/// A positive whole-number option, or absent.
#[cfg(feature = "native")]
fn whole_number<'js>(
    ctx: &Ctx<'js>,
    options: &rquickjs::Object<'js>,
    method: &str,
    key: &str,
) -> Result<Option<u32>> {
    let value: Value = options.get(key)?;
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    match value.as_number() {
        Some(n) if n >= 1.0 && n.fract() == 0.0 && n <= u32::MAX as f64 => Ok(Some(n as u32)),
        _ => Err(Exception::throw_range(
            ctx,
            &format!("{method}(options): {key} must be a whole number of at least 1"),
        )),
    }
}

/// `maxDepth`: a positive whole number, or absent.
#[cfg(feature = "native")]
fn max_depth<'js>(
    ctx: &Ctx<'js>,
    options: &rquickjs::Object<'js>,
    method: &str,
) -> Result<Option<u32>> {
    let value: Value = options.get("maxDepth")?;
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    match value.as_number() {
        Some(n) if n >= 1.0 && n.fract() == 0.0 && n <= u32::MAX as f64 => Ok(Some(n as u32)),
        _ => Err(Exception::throw_range(
            ctx,
            &format!("{method}(options): maxDepth must be a whole number of at least 1"),
        )),
    }
}

/// `stopAt`: an array of edge kinds (by their JavaScript names), or absent.
#[cfg(feature = "native")]
fn stop_at<'js, T: Copy + Into<u32>>(
    ctx: &Ctx<'js>,
    options: &rquickjs::Object<'js>,
    method: &str,
    all: &[T],
    name: fn(&T) -> &'static str,
) -> Result<language_core::KindSet<T>> {
    let value: Value = options.get("stopAt")?;
    if value.is_undefined() || value.is_null() {
        return Ok(language_core::KindSet::default());
    }
    let expected = || {
        all.iter()
            .map(|kind| format!("\"{}\"", name(kind)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let Some(array) = value.into_array() else {
        return Err(Exception::throw_type(
            ctx,
            &format!(
                "{method}(options): stopAt must be an array of edge kinds: {}",
                expected()
            ),
        ));
    };
    let mut set = language_core::KindSet::default();
    for item in array.iter::<Value>() {
        let item = item?;
        let text = item.as_string().map(|s| s.to_string()).transpose()?;
        match text.and_then(|text| all.iter().copied().find(|kind| name(kind) == text)) {
            Some(kind) => set.insert(kind),
            None => {
                return Err(Exception::throw_type(
                    ctx,
                    &format!(
                        "{method}(options): unknown edge kind in stopAt; expected {}",
                        expected()
                    ),
                ))
            }
        }
    }
    Ok(set)
}

/// Why a data-flow or call-graph query cannot run in the WebAssembly build.
#[cfg(not(feature = "native"))]
fn not_in_wasm(method: &str) -> String {
    format!(
        "{method}() is not available in the WebAssembly build of JSSG: it needs the native \
         semantic provider. Run the codemod with the codemod CLI."
    )
}

/// Detect language from file path extension.
#[cfg(feature = "native")]
fn detect_language_from_path(path: &std::path::Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts") | Some("mts") | Some("cts") => "typescript".to_string(),
        Some("tsx") => "tsx".to_string(),
        Some("js") | Some("mjs") | Some("cjs") => "javascript".to_string(),
        Some("jsx") => "jsx".to_string(),
        Some("py") | Some("pyi") => "python".to_string(),
        _ => "typescript".to_string(), // Default to typescript
    }
}

/// Get the language from an AstGrep instance.
#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
fn get_language(grep: &AstGrep<TSDoc>) -> Lang {
    *grep.lang()
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
fn get_language(grep: &AstGrep<TSDoc>) -> crate::ast_grep::wasm_lang::WasmLang {
    grep.lang().clone()
}

/// Apply `(start, end, text)` edits, positions in document bytes, to `old`,
/// the text that starts at document byte `offset`. Of two edits that overlap
/// the first stands. An edit outside `old` is an error: it belongs to another
/// node.
pub fn splice_edits(
    old: &str,
    offset: usize,
    mut edits: Vec<(usize, usize, String)>,
) -> std::result::Result<String, String> {
    edits.sort_by_key(|(start, _, _)| *start);
    let mut text = String::with_capacity(old.len());
    let mut done = 0;
    for (start, end, inserted) in edits {
        let inside = start >= offset && end >= start && end - offset <= old.len();
        let (from, to) = (start.wrapping_sub(offset), end.wrapping_sub(offset));
        if !inside || !old.is_char_boundary(from) || !old.is_char_boundary(to) {
            return Err(format!(
                "commitEdits: the edit at {start}..{end} is outside this node ({offset}..{}); commit edits on a node that contains them, such as the root",
                offset + old.len()
            ));
        }
        if done > from {
            continue;
        }
        text.push_str(&old[done..from]);
        text.push_str(&inserted);
        done = to;
    }
    text.push_str(&old[done..]);
    Ok(text)
}

#[cfg(test)]
mod splice_tests {
    use super::splice_edits;

    #[test]
    fn edits_are_applied_in_order_and_overlaps_dropped() {
        let edits = vec![
            (14, 15, "2".to_string()),
            (10, 11, "b".to_string()),
            (10, 12, "x".to_string()),
        ];
        assert_eq!(splice_edits("a = 1;", 10, edits).unwrap(), "b = 2;");
        assert_eq!(splice_edits("same", 3, Vec::new()).unwrap(), "same");
    }

    #[test]
    fn an_edit_outside_the_node_is_an_error() {
        for edit in [(2, 4, String::new()), (12, 30, String::new())] {
            let error = splice_edits("a = 1;", 10, vec![edit]).unwrap_err();
            assert!(error.contains("outside this node"), "{error}");
        }
    }
}
