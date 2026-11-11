//! rquickjs facade for the JSSG `SgRoot` / `SgNode` classes.
//!
//! Every method is a thin adapter over [`super::sg_core`]: convert JS
//! arguments, call the core, convert the result, and map [`CoreError`] to a
//! QuickJS exception. Host-specific behaviour that depends on the QuickJS
//! context userdata (target-directory containment, dry-run and staged writes)
//! lives here rather than in the core.

use rquickjs::{
    class, class::Trace, methods, prelude::Opt, Ctx, Exception, IntoJs, JsLifetime, Result, Value,
};
use std::marker::PhantomData;

use crate::ast_grep::sg_core::{CoreError, CoreErrorKind, CoreNode, CoreRoot, DefinitionOptions};
use crate::ast_grep::types::JsEdit;
use crate::ast_grep::types::JsNodeRange;
use crate::ast_grep::utils::convert_matcher;

/// Raise a [`CoreError`] as the matching QuickJS exception.
pub(crate) fn throw_core_error(ctx: &Ctx<'_>, error: CoreError) -> rquickjs::Error {
    match error.kind {
        CoreErrorKind::Error => Exception::throw_message(ctx, &error.message),
        CoreErrorKind::TypeError => Exception::throw_type(ctx, &error.message),
    }
}

#[derive(Trace, Clone)]
#[class(rename_all = "camelCase")]
pub struct SgRootRjs<'js> {
    #[qjs(skip_trace)]
    pub(crate) core: CoreRoot,
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
        Ok(SgNodeRjs::from_core(self.core.root()))
    }

    pub fn filename(&self) -> Result<String> {
        Ok(self.core.filename().unwrap_or_default().to_string())
    }

    #[qjs(rename = "relativeFilename")]
    pub fn relative_filename(&self) -> Result<String> {
        Ok(self
            .core
            .relative_filename()
            .unwrap_or_default()
            .to_string())
    }

    pub fn source(&self) -> Result<String> {
        Ok(self.core.root_text())
    }

    /// Rename the current file to a new path.
    ///
    /// If the path is relative, it is resolved against the current file's parent directory.
    /// If absolute, it is used as-is.
    pub fn rename(&self, new_path: String, ctx: Ctx<'js>) -> Result<()> {
        let resolved_path = self
            .core
            .resolve_rename_path(&new_path)
            .map_err(|e| throw_core_error(&ctx, e))?;

        // Validate: resolved path must stay within the target directory
        #[cfg(feature = "native")]
        {
            use crate::sandbox::engine::execution_engine::validate_path_within_target;
            validate_path_within_target(&ctx, std::path::Path::new(&resolved_path), "rename()")?;
        }

        self.core
            .set_rename_to(resolved_path)
            .map_err(|e| throw_core_error(&ctx, e))
    }

    /// Write content to this file.
    ///
    /// This method is only valid for files obtained via `definition()` or `references()`.
    /// It cannot be called on the current file being processed - for that, return the
    /// modified content from the `transform()` function instead.
    ///
    /// After writing, the semantic provider's cache is updated with the new content.
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
            let file_path = self
                .core
                .check_write_target()
                .map_err(|e| throw_core_error(&ctx, e))?;

            let dry_run = ctx
                .userdata::<crate::sandbox::engine::execution_engine::DryRunExecutionFlag>()
                .map(|flag| flag.0)
                .unwrap_or(false);
            if dry_run {
                return Ok(());
            }

            let path = std::path::Path::new(file_path);
            let stage_writes = ctx
                .userdata::<crate::sandbox::engine::execution_engine::StageFileWrites>()
                .map(|flag| flag.0)
                .unwrap_or(false);
            if stage_writes {
                // A staging host owns the filesystem: hand the edit back as a
                // secondary change (like `jssgTransform`) instead of writing.
                use crate::sandbox::engine::execution_engine::{
                    validate_path_within_target, ExecutionResult, FileChange, JssgFileChanges,
                    ModifiedResult,
                };
                validate_path_within_target(&ctx, path, "write()")?;
                let changes = ctx
                    .userdata::<JssgFileChanges>()
                    .map(|guard| guard.clone())
                    .ok_or_else(|| {
                        Exception::throw_message(&ctx, "JssgFileChanges not found in userdata")
                    })?;
                changes
                    .changes
                    .lock()
                    .map_err(|e| {
                        Exception::throw_message(
                            &ctx,
                            &format!("Failed to lock file_changes mutex: {e}"),
                        )
                    })?
                    .push(FileChange {
                        path: path.to_path_buf(),
                        result: ExecutionResult::Modified(ModifiedResult {
                            content: content.clone(),
                            rename_to: None,
                        }),
                    });
                self.core.notify_written(path, &content);
                return Ok(());
            }

            std::fs::write(path, &content).map_err(|e| {
                Exception::throw_message(
                    &ctx,
                    &format!("Failed to write file '{}': {}", file_path, e),
                )
            })?;

            self.core.notify_written(path, &content);

            Ok(())
        }
    }
}

impl<'js> SgRootRjs<'js> {
    pub(crate) fn from_core(core: CoreRoot) -> Self {
        SgRootRjs {
            core,
            _phantom: PhantomData,
        }
    }

    pub fn try_new(
        lang_str: String,
        src: String,
        filename: Option<String>,
        target_directory: Option<&std::path::Path>,
    ) -> std::result::Result<Self, String> {
        CoreRoot::parse(&lang_str, src, filename, target_directory).map(Self::from_core)
    }
}

#[derive(Trace, Clone)]
#[class(rename_all = "camelCase")]
pub struct SgNodeRjs<'js> {
    #[qjs(skip_trace)] // Holds the root alive; not a JS value
    pub(crate) core: CoreNode,
    #[qjs(skip_trace)]
    pub(crate) _phantom: PhantomData<&'js ()>,
}

unsafe impl<'js> JsLifetime<'js> for SgNodeRjs<'js> {
    type Changed<'to> = SgNodeRjs<'to>;
}

impl<'js> SgNodeRjs<'js> {
    pub(crate) fn from_core(core: CoreNode) -> Self {
        SgNodeRjs {
            core,
            _phantom: PhantomData,
        }
    }

    fn wrap_all(nodes: Vec<CoreNode>) -> Vec<SgNodeRjs<'js>> {
        nodes.into_iter().map(SgNodeRjs::from_core).collect()
    }

    fn wrap_opt(node: Option<CoreNode>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match node {
            Some(node) => SgNodeRjs::from_core(node).into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    fn matcher(
        &self,
        matcher: Value<'js>,
        ctx: &Ctx<'js>,
    ) -> Result<crate::ast_grep::sg_core::CoreMatcher> {
        convert_matcher(matcher, self.core.root().lang(), ctx)
    }
}

#[methods]
impl<'js> SgNodeRjs<'js> {
    pub fn text(&self) -> Result<String> {
        Ok(self.core.text())
    }

    pub fn is(&self, kind: String) -> Result<bool> {
        Ok(self.core.is(&kind))
    }

    pub fn kind(&self) -> Result<String> {
        Ok(self.core.kind())
    }

    pub fn range(&self, _ctx: Ctx<'js>) -> Result<JsNodeRange> {
        Ok(self.core.range())
    }

    pub fn id(&self) -> Result<usize> {
        Ok(self.core.id())
    }

    #[qjs(rename = "isLeaf")]
    pub fn is_leaf(&self) -> Result<bool> {
        Ok(self.core.is_leaf())
    }

    #[qjs(rename = "isNamed")]
    pub fn is_named(&self) -> Result<bool> {
        Ok(self.core.is_named())
    }

    #[qjs(rename = "isNamedLeaf")]
    pub fn is_named_leaf(&self) -> Result<bool> {
        Ok(self.core.is_named_leaf())
    }

    pub fn parent(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::wrap_opt(self.core.parent(), ctx)
    }

    pub fn child(&self, nth: usize, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::wrap_opt(self.core.child(nth), ctx)
    }

    pub fn children(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(Self::wrap_all(self.core.children()))
    }

    pub fn ancestors(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(Self::wrap_all(self.core.ancestors()))
    }

    pub fn next(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::wrap_opt(self.core.next(), ctx)
    }

    #[qjs(rename = "nextAll")]
    pub fn next_all(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(Self::wrap_all(self.core.next_all()))
    }

    pub fn prev(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::wrap_opt(self.core.prev(), ctx)
    }

    #[qjs(rename = "prevAll")]
    pub fn prev_all(&self) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(Self::wrap_all(self.core.prev_all()))
    }

    pub fn field(&self, name: String, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::wrap_opt(self.core.field(&name), ctx)
    }

    #[qjs(rename = "fieldChildren")]
    pub fn field_children(&self, name: String) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(Self::wrap_all(self.core.field_children(&name)))
    }

    pub fn matches(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let matcher = self.matcher(matcher, &ctx)?;
        Ok(self.core.matches(&matcher))
    }

    pub fn inside(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let matcher = self.matcher(matcher, &ctx)?;
        Ok(self.core.inside(&matcher))
    }

    pub fn has(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let matcher = self.matcher(matcher, &ctx)?;
        Ok(self.core.has(&matcher))
    }

    pub fn precedes(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let matcher = self.matcher(matcher, &ctx)?;
        Ok(self.core.precedes(&matcher))
    }

    pub fn follows(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let matcher = self.matcher(matcher, &ctx)?;
        Ok(self.core.follows(&matcher))
    }

    #[qjs(rename = "getMatch")]
    pub fn get_match(&self, meta_var: String, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::wrap_opt(self.core.get_match(&meta_var), ctx)
    }

    #[qjs(rename = "getMultipleMatches")]
    pub fn get_multiple_matches(&self, meta_var: String) -> Result<Vec<SgNodeRjs<'js>>> {
        Ok(Self::wrap_all(self.core.get_multiple_matches(&meta_var)))
    }

    #[qjs(rename = "getTransformed")]
    pub fn get_transformed(&self, meta_var: String, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.core.get_transformed_raw(&meta_var) {
            Some(s) => s.into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn find(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let matcher = self.matcher(matcher, &ctx)?;
        Self::wrap_opt(self.core.find(&matcher), ctx)
    }

    #[qjs(rename = "findAll")]
    pub fn find_all(&self, matcher: Value<'js>, ctx: Ctx<'js>) -> Result<Vec<SgNodeRjs<'js>>> {
        let matcher = self.matcher(matcher, &ctx)?;
        Ok(Self::wrap_all(self.core.find_all(&matcher)))
    }

    pub fn replace(&self, text: String) -> Result<JsEdit> {
        Ok(self.core.replace(text))
    }

    #[qjs(rename = "commitEdits")]
    pub fn commit_edits(&self, edits: Vec<JsEdit>) -> Result<String> {
        Ok(self.core.commit_edits(&edits))
    }

    pub fn debug(&self) -> Result<String> {
        Ok(self.core.debug())
    }

    #[qjs(rename = "getRoot")]
    pub fn get_root(&self, _ctx: Ctx<'js>) -> Result<SgRootRjs<'js>> {
        Ok(SgRootRjs::from_core(self.core.root().clone()))
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
        let Some(request) = self
            .core
            .semantic()
            .map_err(|e| throw_core_error(&ctx, e))?
        else {
            return Ok(Value::new_null(ctx));
        };

        // Parse options
        let resolve_external = if let Some(opts) = options.0 {
            opts.get::<_, Option<bool>>("resolveExternal")?
                .unwrap_or(true)
        } else {
            true
        };

        match request
            .definition(DefinitionOptions { resolve_external })
            .map_err(|e| throw_core_error(&ctx, e))?
        {
            Some(def) => {
                let result_obj = rquickjs::Object::new(ctx.clone())?;
                result_obj.set("node", SgNodeRjs::from_core(def.node))?;
                result_obj.set("root", SgRootRjs::from_core(def.root))?;
                result_obj.set("kind", def.kind)?;
                result_obj.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
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
        let Some(request) = self
            .core
            .semantic()
            .map_err(|e| throw_core_error(&ctx, e))?
        else {
            return rquickjs::Array::new(ctx.clone())?.into_js(&ctx);
        };

        let files = request
            .references()
            .map_err(|e| throw_core_error(&ctx, e))?;

        let result_array = rquickjs::Array::new(ctx.clone())?;
        for (idx, file_refs) in files.into_iter().enumerate() {
            // Unsupported-language files leave a hole, as before.
            let Some(file_refs) = file_refs else {
                continue;
            };
            let file_obj = rquickjs::Object::new(ctx.clone())?;
            file_obj.set("root", SgRootRjs::from_core(file_refs.root))?;

            let nodes_array = rquickjs::Array::new(ctx.clone())?;
            for (node_idx, node) in file_refs.nodes.into_iter().enumerate() {
                if let Some(node) = node {
                    nodes_array.set(node_idx, SgNodeRjs::from_core(node))?;
                }
            }
            file_obj.set("nodes", nodes_array)?;
            result_array.set(idx, file_obj)?;
        }

        result_array.into_js(&ctx)
    }

    /// Get type information for the symbol at this node's position.
    ///
    /// TODO: Currently, this is not implemented by any of the semantic providers. So it's not publicly available.
    ///
    /// Returns null if:
    /// - No semantic provider is configured
    /// - Type information is not available
    #[qjs(rename = "typeInfo")]
    pub fn type_info(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let Some(request) = self
            .core
            .semantic()
            .map_err(|e| throw_core_error(&ctx, e))?
        else {
            return Ok(Value::new_null(ctx));
        };

        match request.type_info().map_err(|e| throw_core_error(&ctx, e))? {
            Some(type_str) => type_str.into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }
}
