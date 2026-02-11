#[cfg(feature = "wasm")]
use crate::ast_grep::wasm_lang::WasmDoc;
#[cfg(not(feature = "wasm"))]
use ast_grep_core::tree_sitter::StrDoc as TSStrDoc;
use ast_grep_core::{AstGrep, Node, NodeMatch};

#[cfg(all(not(feature = "wasm"), not(feature = "native")))]
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

#[cfg(all(not(feature = "wasm"), not(feature = "native")))]
use ast_grep_language::SupportLang as Lang;

#[cfg(feature = "native")]
use crate::sandbox::engine::codemod_lang::CodemodLang;
#[cfg(feature = "native")]
use CodemodLang as Lang;

#[cfg(all(not(feature = "wasm"), not(feature = "native")))]
type TSDoc = TSStrDoc<SupportLang>;
#[cfg(feature = "native")]
type TSDoc = TSStrDoc<CodemodLang>;
#[cfg(feature = "wasm")]
type TSDoc = WasmDoc;

pub(crate) struct SgRootInner {
    pub(crate) grep: AstGrep<TSDoc>,
    filename: Option<String>,
    /// Optional rename target path set by root.rename()
    pub(crate) rename_to: Mutex<Option<String>>,
    /// Optional semantic provider for symbol indexing (native only)
    #[cfg(feature = "native")]
    pub(crate) semantic_provider: Option<Arc<dyn SemanticProvider>>,
    /// The current file being processed (for write() validation)
    #[cfg(feature = "native")]
    pub(crate) current_file_path: Option<String>,
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

    pub fn source(&self) -> Result<String> {
        Ok(self.inner.grep.source().to_string())
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
        if new_path.is_empty() {
            return Err(Exception::throw_message(
                &ctx,
                "rename() requires a non-empty path",
            ));
        }

        let resolved_path = if std::path::Path::new(&new_path).is_absolute() {
            new_path
        } else {
            // Resolve relative to current file's parent directory
            match &self.inner.filename {
                Some(filename) => {
                    let parent = std::path::Path::new(filename)
                        .parent()
                        .unwrap_or(std::path::Path::new("."));
                    parent.join(&new_path).to_string_lossy().to_string()
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

    #[cfg(feature = "native")]
    pub fn write(&self, content: String, ctx: Ctx<'js>) -> Result<()> {
        let file_path = match &self.inner.filename {
            Some(f) => f,
            None => {
                return Err(Exception::throw_message(
                    &ctx,
                    "Cannot write: file has no path",
                ))
            }
        };

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

        let path = std::path::Path::new(file_path);
        std::fs::write(path, &content).map_err(|e| {
            Exception::throw_message(
                &ctx,
                &format!("Failed to write file '{}': {}", file_path, e),
            )
        })?;

        if let Some(ref provider) = self.inner.semantic_provider {
            let _ = provider.notify_file_processed(path, &content);
        }

        Ok(())
    }
}

impl<'js> SgRootRjs<'js> {
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
    ) -> std::result::Result<Self, String> {
        #[cfg(feature = "wasm")]
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
                    filename,
                    rename_to: Mutex::new(None),
                }),
                _phantom: PhantomData,
            })
        }

        #[cfg(all(not(feature = "wasm"), not(feature = "native")))]
        {
            let lang = SupportLang::from_str(&lang_str)
                .map_err(|e| format!("Unsupported language: {lang_str}. Error: {e}"))?;
            let grep = AstGrep::new(src, lang);
            Ok(SgRootRjs {
                inner: Arc::new(SgRootInner {
                    grep,
                    filename,
                    rename_to: Mutex::new(None),
                    #[cfg(feature = "native")]
                    semantic_provider: None,
                    #[cfg(feature = "native")]
                    current_file_path: None,
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
                    filename,
                    rename_to: Mutex::new(None),
                    #[cfg(feature = "native")]
                    semantic_provider: None,
                    #[cfg(feature = "native")]
                    current_file_path: None,
                }),
                _phantom: PhantomData,
            })
        }
    }

    pub fn try_new_from_ast_grep(
        grep: AstGrep<TSDoc>,
        filename: Option<String>,
    ) -> std::result::Result<Self, String> {
        Ok(SgRootRjs {
            inner: Arc::new(SgRootInner {
                grep,
                filename,
                rename_to: Mutex::new(None),
                #[cfg(feature = "native")]
                semantic_provider: None,
                #[cfg(feature = "native")]
                current_file_path: None,
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
    ) -> std::result::Result<Self, String> {
        Ok(SgRootRjs {
            inner: Arc::new(SgRootInner {
                grep,
                filename,
                rename_to: Mutex::new(None),
                semantic_provider,
                current_file_path,
            }),
            _phantom: PhantomData,
        })
    }
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
            Some(s) => s.into_js(&ctx),
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
    pub fn commit_edits(&self, edits: Vec<JsEdit>) -> Result<String> {
        let mut sorted_edits = edits.clone();
        sorted_edits.sort_by_key(|edit| edit.start_pos);

        let mut new_content = String::new();
        let old_content = self.inner_node.text();

        let offset = self.inner_node.range().start;
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
        Ok(new_content)
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
    #[cfg(feature = "native")]
    #[qjs(rename = "definition")]
    pub fn definition(
        &self,
        ctx: Ctx<'js>,
        options: Opt<rquickjs::Object<'js>>,
    ) -> Result<Value<'js>> {
        let provider = match &self.root.semantic_provider {
            Some(p) => p,
            None => return Ok(Value::new_null(ctx)),
        };

        let file_path = match &self.root.filename {
            Some(f) => std::path::PathBuf::from(f),
            None => return Ok(Value::new_null(ctx)),
        };

        // Parse options
        let resolve_external = if let Some(opts) = options.0 {
            opts.get::<_, Option<bool>>("resolveExternal")?
                .unwrap_or(true)
        } else {
            true
        };

        let def_options = language_core::DefinitionOptions { resolve_external };

        let byte_range = self.inner_node.range();
        let range = language_core::ByteRange::new(byte_range.start as u32, byte_range.end as u32);

        match provider.get_definition(&file_path, range, def_options) {
            Ok(Some(def_result)) => {
                let is_same_file = def_result.location.file_path == file_path;
                let loc = &def_result.location;

                // Convert DefinitionKind to string for JS
                let kind_str = def_result.kind.as_str();

                if is_same_file {
                    // Definition is in the same file, use existing root
                    let root_node = self.root.grep.root();
                    if let Some(node) = find_node_at_range(
                        &root_node,
                        loc.range.start as usize,
                        loc.range.end as usize,
                    ) {
                        let node_match: NodeMatch<_> = node.into();
                        let static_node_match: NodeMatch<'static, TSDoc> =
                            unsafe { std::mem::transmute(node_match) };

                        let result_obj = rquickjs::Object::new(ctx.clone())?;

                        let sg_node = SgNodeRjs {
                            root: Arc::clone(&self.root),
                            inner_node: static_node_match,
                            _phantom: PhantomData,
                        };
                        result_obj.set("node", sg_node)?;

                        let sg_root = SgRootRjs {
                            inner: Arc::clone(&self.root),
                            _phantom: PhantomData,
                        };
                        result_obj.set("root", sg_root)?;
                        result_obj.set("kind", kind_str)?;

                        return result_obj.into_js(&ctx);
                    }
                } else {
                    // Definition is in a different file, create new root
                    let lang_str = detect_language_from_path(&def_result.location.file_path);
                    let lang = Lang::from_str(&lang_str).map_err(|e| {
                        Exception::throw_message(&ctx, &format!("Unsupported language: {}", e))
                    })?;
                    let grep = AstGrep::new(def_result.content.clone(), lang);

                    if let Ok(new_root) = SgRootRjs::try_new_with_semantic(
                        grep,
                        Some(def_result.location.file_path.to_string_lossy().to_string()),
                        self.root.semantic_provider.clone(),
                        self.root.current_file_path.clone(),
                    ) {
                        let root_node = new_root.inner.grep.root();
                        if let Some(node) = find_node_at_range(
                            &root_node,
                            loc.range.start as usize,
                            loc.range.end as usize,
                        ) {
                            let node_match: NodeMatch<_> = node.into();
                            let static_node_match: NodeMatch<'static, TSDoc> =
                                unsafe { std::mem::transmute(node_match) };

                            let result_obj = rquickjs::Object::new(ctx.clone())?;

                            let sg_node = SgNodeRjs {
                                root: Arc::clone(&new_root.inner),
                                inner_node: static_node_match,
                                _phantom: PhantomData,
                            };
                            result_obj.set("node", sg_node)?;
                            result_obj.set("root", new_root)?;
                            result_obj.set("kind", kind_str)?;

                            return result_obj.into_js(&ctx);
                        }
                    }
                }

                Ok(Value::new_null(ctx))
            }
            Ok(None) => Ok(Value::new_null(ctx)),
            Err(e) => Err(Exception::throw_message(
                &ctx,
                &format!("Failed to get definition: {}", e),
            )),
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
    #[cfg(feature = "native")]
    #[qjs(rename = "references")]
    pub fn references(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let provider = match &self.root.semantic_provider {
            Some(p) => p,
            None => return rquickjs::Array::new(ctx.clone())?.into_js(&ctx),
        };

        let file_path = match &self.root.filename {
            Some(f) => std::path::PathBuf::from(f),
            None => return rquickjs::Array::new(ctx.clone())?.into_js(&ctx),
        };

        let byte_range = self.inner_node.range();
        let range = language_core::ByteRange::new(byte_range.start as u32, byte_range.end as u32);

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

    /// Get type information for the symbol at this node's position.
    ///
    /// TODO: Currently, this is not implemented by any of the semantic providers. So it's not publicly available.
    ///
    /// Returns null if:
    /// - No semantic provider is configured
    /// - Type information is not available
    #[cfg(feature = "native")]
    #[qjs(rename = "typeInfo")]
    pub fn type_info(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let provider = match &self.root.semantic_provider {
            Some(p) => p,
            None => return Ok(Value::new_null(ctx)),
        };

        let file_path = match &self.root.filename {
            Some(f) => std::path::PathBuf::from(f),
            None => return Ok(Value::new_null(ctx)),
        };

        let byte_range = self.inner_node.range();
        let range = language_core::ByteRange::new(byte_range.start as u32, byte_range.end as u32);

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
#[cfg(not(feature = "wasm"))]
fn get_language(grep: &AstGrep<TSDoc>) -> Lang {
    *grep.lang()
}

#[cfg(feature = "wasm")]
fn get_language(grep: &AstGrep<TSDoc>) -> crate::ast_grep::wasm_lang::WasmLang {
    grep.lang().clone()
}
