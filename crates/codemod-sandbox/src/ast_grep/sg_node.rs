#[cfg(feature = "wasm")]
use crate::ast_grep::wasm_lang::WasmDoc;
#[cfg(not(feature = "wasm"))]
use ast_grep_core::tree_sitter::StrDoc as TSStrDoc;
use ast_grep_core::{AstGrep, Node, NodeMatch};

#[cfg(not(feature = "wasm"))]
use codemod_ast_grep_dynamic_lang::DynamicLang;

use rquickjs::{class, class::Trace, methods, Ctx, Exception, IntoJs, JsLifetime, Result, Value};
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Arc;

use crate::ast_grep::types::JsEdit;
use crate::ast_grep::types::JsNodeRange;
use crate::ast_grep::utils::{convert_matcher, JsMatcherRjs};

#[cfg(not(feature = "wasm"))]
type TSDoc = TSStrDoc<DynamicLang>;
#[cfg(feature = "wasm")]
type TSDoc = WasmDoc;

pub(crate) struct SgRootInner {
    grep: AstGrep<TSDoc>,
    filename: Option<String>,
}

#[derive(Trace)]
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
        let str_slice: &str = "asd";
        Ok(str_slice.to_string())
    }
}

impl<'js> SgRootRjs<'js> {
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
                }),
                _phantom: PhantomData,
            })
        }

        #[cfg(not(feature = "wasm"))]
        {
            let lang = DynamicLang::from_str(&lang_str)
                .map_err(|e| format!("Unsupported language: {lang_str}. Error: {e}"))?;
            let grep = AstGrep::new(src, lang);
            Ok(SgRootRjs {
                inner: Arc::new(SgRootInner { grep, filename }),
                _phantom: PhantomData,
            })
        }
    }

    pub fn try_new_from_ast_grep(
        grep: AstGrep<TSDoc>,
        filename: Option<String>,
    ) -> std::result::Result<Self, String> {
        Ok(SgRootRjs {
            inner: Arc::new(SgRootInner { grep, filename }),
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

    pub fn find(&self, value: Value<'js>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let lang = *self.inner_node.lang();
        let matcher = convert_matcher(value, lang, &ctx)?;

        let create_sg_node = |node: NodeMatch<TSDoc>, ctx: &Ctx<'js>| -> Result<Value<'js>> {
            let static_node_match: NodeMatch<'static, TSDoc> = unsafe { std::mem::transmute(node) };
            let sg_node = SgNodeRjs {
                root: self.root.clone(),
                inner_node: static_node_match,
                _phantom: PhantomData,
            };
            sg_node.into_js(ctx)
        };

        match matcher {
            JsMatcherRjs::Pattern(pattern) => match self.inner_node.find(pattern) {
                Some(node) => create_sg_node(node, &ctx),
                None => Ok(Value::new_null(ctx)),
            },
            JsMatcherRjs::Kind(kind_matcher) => match self.inner_node.find(kind_matcher) {
                Some(node) => create_sg_node(node, &ctx),
                None => Ok(Value::new_null(ctx)),
            },
            JsMatcherRjs::Config(config) => match self.inner_node.find(config) {
                Some(node) => create_sg_node(node, &ctx),
                None => Ok(Value::new_null(ctx)),
            },
        }
    }

    #[qjs(rename = "findAll")]
    pub fn find_all(&self, value: Value<'js>, ctx: Ctx<'js>) -> Result<Vec<SgNodeRjs<'js>>> {
        let lang = *self.inner_node.lang();
        let matcher = convert_matcher(value, lang, &ctx)?;

        let create_sg_node = |node: NodeMatch<TSDoc>| -> SgNodeRjs<'js> {
            let static_node_match: NodeMatch<'static, TSDoc> = unsafe { std::mem::transmute(node) };
            SgNodeRjs {
                root: self.root.clone(),
                inner_node: static_node_match,
                _phantom: PhantomData,
            }
        };

        match matcher {
            JsMatcherRjs::Pattern(pattern) => Ok(self
                .inner_node
                .find_all(pattern)
                .map(create_sg_node)
                .collect()),
            JsMatcherRjs::Kind(kind_matcher) => Ok(self
                .inner_node
                .find_all(kind_matcher)
                .map(create_sg_node)
                .collect()),
            JsMatcherRjs::Config(config) => Ok(self
                .inner_node
                .find_all(config)
                .map(create_sg_node)
                .collect()),
        }
    }

    pub fn matches(&self, value: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = *self.inner_node.lang();
        let matcher = convert_matcher(value, lang, &ctx)?;

        match matcher {
            JsMatcherRjs::Pattern(pattern) => Ok(self.inner_node.matches(pattern)),
            JsMatcherRjs::Kind(kind_matcher) => Ok(self.inner_node.matches(kind_matcher)),
            JsMatcherRjs::Config(config) => Ok(self.inner_node.matches(config)),
        }
    }

    pub fn inside(&self, value: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = *self.inner_node.lang();
        let matcher = convert_matcher(value, lang, &ctx)?;

        match matcher {
            JsMatcherRjs::Pattern(pattern) => Ok(self.inner_node.inside(pattern)),
            JsMatcherRjs::Kind(kind_matcher) => Ok(self.inner_node.inside(kind_matcher)),
            JsMatcherRjs::Config(config) => Ok(self.inner_node.inside(config)),
        }
    }

    pub fn has(&self, value: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let lang = *self.inner_node.lang();
        let matcher = convert_matcher(value, lang, &ctx)?;

        match matcher {
            JsMatcherRjs::Pattern(pattern) => Ok(self.inner_node.has(pattern)),
            JsMatcherRjs::Kind(kind_matcher) => Ok(self.inner_node.has(kind_matcher)),
            JsMatcherRjs::Config(config) => Ok(self.inner_node.has(config)),
        }
    }

    #[qjs(rename = "getMatch")]
    pub fn get_match(&self, m: String, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self
            .inner_node
            .get_env()
            .get_match(&m)
            .cloned()
            .map(NodeMatch::from)
        {
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

    #[qjs(rename = "getMultipleMatches")]
    pub fn get_multiple_matches(&self, m: String) -> Result<Vec<SgNodeRjs<'js>>> {
        let nodes = self
            .inner_node
            .get_env()
            .get_multiple_matches(&m)
            .into_iter()
            .map(|node| {
                let node_match = NodeMatch::from(node);
                let static_node_match: NodeMatch<'static, TSDoc> =
                    unsafe { std::mem::transmute(node_match) };
                SgNodeRjs {
                    root: self.root.clone(),
                    inner_node: static_node_match,
                    _phantom: PhantomData,
                }
            })
            .collect();

        Ok(nodes)
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

    #[qjs(rename = "getRoot")]
    pub fn get_root(&self, _ctx: Ctx<'js>) -> Result<SgRootRjs<'js>> {
        Ok(SgRootRjs {
            inner: Arc::clone(&self.root),
            _phantom: PhantomData,
        })
    }
}
