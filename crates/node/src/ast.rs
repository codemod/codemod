//! The `codemod:ast-grep` API for Node: `SgRoot` and `SgNode`, with the same
//! methods, matchers and languages as the engine's QuickJS binding
//! (`codemod-sandbox`'s `sg_node.rs`).
//!
//! Like `parse()` in a QuickJS `forEachFile` callback, roots parsed here have
//! no semantic provider: `definition()` returns `null` and `references()` an
//! empty array.

use std::str::FromStr;
use std::sync::Arc;

use ast_grep_core::tree_sitter::{LanguageExt, StrDoc};
use ast_grep_core::{AstGrep, Language, Node, NodeMatch};
use codemod_sandbox::matchers::json_matcher;
use codemod_sandbox::CodemodLang;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;

type Doc = StrDoc<CodemodLang>;

struct RootInner {
    grep: AstGrep<Doc>,
    filename: Option<String>,
    relative_filename: Option<String>,
}

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

#[napi(object)]
pub struct Edit {
    pub start_pos: u32,
    pub end_pos: u32,
    pub inserted_text: String,
}

/// Parse `source` as `language`. `filename` is what `root.filename()`
/// returns; `relativeFilename` what `root.relativeFilename()` returns.
#[napi]
pub fn parse(
    language: String,
    source: String,
    filename: Option<String>,
    relative_filename: Option<String>,
) -> Result<SgRoot> {
    let lang = CodemodLang::from_str(&language)
        .map_err(|e| Error::from_reason(format!("Unsupported language: {language}. Error: {e}")))?;
    Ok(SgRoot {
        inner: Arc::new(RootInner {
            grep: AstGrep::new(source, lang),
            filename,
            relative_filename,
        }),
    })
}

/// The language of a file, from its extension.
#[napi]
pub fn language_from_path(path: String) -> Option<String> {
    CodemodLang::from_path(path).map(|lang| lang.to_string())
}

/// The numeric id of a named node kind, for `{ kind: id }` matchers.
#[napi]
pub fn kind(language: String, kind_name: String) -> Result<u32> {
    let lang = CodemodLang::from_str(&language)
        .map_err(|e| Error::from_reason(format!("Language error: {e}")))?;
    Ok(lang.get_ts_language().id_for_node_kind(&kind_name, true) as u32)
}

#[napi]
pub struct SgRoot {
    inner: Arc<RootInner>,
}

#[napi]
impl SgRoot {
    #[napi]
    pub fn root(&self) -> SgNode {
        SgNode::new(&self.inner, self.inner.grep.root().into())
    }

    #[napi]
    pub fn filename(&self) -> String {
        self.inner.filename.clone().unwrap_or_default()
    }

    #[napi]
    pub fn relative_filename(&self) -> String {
        self.inner
            .relative_filename
            .clone()
            .unwrap_or_else(|| self.filename())
    }

    /// The full source text, including leading whitespace that the root
    /// node's own text leaves out.
    #[napi]
    pub fn source(&self) -> String {
        self.inner.grep.source().to_string()
    }

    #[napi]
    pub fn rename(&self, _path: String) -> Result<()> {
        Err(Error::from_reason(
            "root.rename() is for jssg transforms; in a forEachFile callback, call file.rename(path)",
        ))
    }

    #[napi]
    pub fn write(&self, _content: String) -> Result<()> {
        Err(Error::from_reason(
            "root.write() is for files found through definition() or references(), which roots parsed in a forEachFile callback have none of; call file.write(text) for the callback's file",
        ))
    }
}

#[napi]
pub struct SgNode {
    root: Arc<RootInner>,
    node: NodeMatch<'static, Doc>,
}

impl SgNode {
    fn new(root: &Arc<RootInner>, node: NodeMatch<'_, Doc>) -> Self {
        // SAFETY: the node borrows from `root.grep`, which this SgNode keeps
        // alive through its `Arc`, and whose tree is never changed.
        let node: NodeMatch<'static, Doc> = unsafe { std::mem::transmute(node) };
        Self {
            root: Arc::clone(root),
            node,
        }
    }

    fn wrap(&self, node: Node<'_, Doc>) -> SgNode {
        SgNode::new(&self.root, node.into())
    }

    fn wrap_all<'a>(&self, nodes: impl Iterator<Item = Node<'a, Doc>>) -> Vec<SgNode> {
        nodes.map(|node| self.wrap(node)).collect()
    }

    fn matcher(&self, matcher: Value) -> Result<codemod_sandbox::matchers::Matcher> {
        json_matcher(matcher, *self.root.grep.lang()).map_err(Error::from_reason)
    }
}

#[napi]
impl SgNode {
    #[napi]
    pub fn text(&self) -> String {
        self.node.text().to_string()
    }

    #[napi]
    pub fn is(&self, kind: String) -> bool {
        self.node.kind() == kind
    }

    #[napi]
    pub fn kind(&self) -> String {
        self.node.kind().to_string()
    }

    #[napi]
    pub fn range(&self) -> Range {
        let start = self.node.start_pos();
        let end = self.node.end_pos();
        let bytes = self.node.range();
        Range {
            start: Position {
                line: start.line() as u32,
                column: start.column(&self.node) as u32,
                index: bytes.start as u32,
            },
            end: Position {
                line: end.line() as u32,
                column: end.column(&self.node) as u32,
                index: bytes.end as u32,
            },
        }
    }

    #[napi]
    pub fn id(&self) -> u32 {
        self.node.node_id() as u32
    }

    #[napi]
    pub fn is_leaf(&self) -> bool {
        self.node.is_leaf()
    }

    #[napi]
    pub fn is_named(&self) -> bool {
        self.node.is_named()
    }

    #[napi]
    pub fn is_named_leaf(&self) -> bool {
        self.node.is_named_leaf()
    }

    #[napi]
    pub fn parent(&self) -> Option<SgNode> {
        self.node.parent().map(|node| self.wrap(node))
    }

    #[napi]
    pub fn child(&self, nth: u32) -> Option<SgNode> {
        self.node.child(nth as usize).map(|node| self.wrap(node))
    }

    #[napi]
    pub fn children(&self) -> Vec<SgNode> {
        self.wrap_all(self.node.children())
    }

    #[napi]
    pub fn ancestors(&self) -> Vec<SgNode> {
        self.wrap_all(self.node.ancestors())
    }

    #[napi]
    pub fn next(&self) -> Option<SgNode> {
        self.node.next().map(|node| self.wrap(node))
    }

    #[napi]
    pub fn next_all(&self) -> Vec<SgNode> {
        self.wrap_all(self.node.next_all())
    }

    #[napi]
    pub fn prev(&self) -> Option<SgNode> {
        self.node.prev().map(|node| self.wrap(node))
    }

    #[napi]
    pub fn prev_all(&self) -> Vec<SgNode> {
        self.wrap_all(self.node.prev_all())
    }

    #[napi]
    pub fn field(&self, name: String) -> Option<SgNode> {
        self.node.field(&name).map(|node| self.wrap(node))
    }

    #[napi]
    pub fn field_children(&self, name: String) -> Vec<SgNode> {
        self.wrap_all(self.node.field_children(&name))
    }

    #[napi]
    pub fn matches(&self, matcher: Value) -> Result<bool> {
        Ok(self.node.matches(&self.matcher(matcher)?))
    }

    #[napi]
    pub fn inside(&self, matcher: Value) -> Result<bool> {
        Ok(self.node.inside(&self.matcher(matcher)?))
    }

    #[napi]
    pub fn has(&self, matcher: Value) -> Result<bool> {
        Ok(self.node.has(&self.matcher(matcher)?))
    }

    #[napi]
    pub fn precedes(&self, matcher: Value) -> Result<bool> {
        Ok(self.node.precedes(&self.matcher(matcher)?))
    }

    #[napi]
    pub fn follows(&self, matcher: Value) -> Result<bool> {
        Ok(self.node.follows(&self.matcher(matcher)?))
    }

    #[napi]
    pub fn get_match(&self, meta_var: String) -> Option<SgNode> {
        self.node
            .get_env()
            .get_match(&meta_var)
            .map(|node| self.wrap(node.clone()))
    }

    #[napi]
    pub fn get_multiple_matches(&self, meta_var: String) -> Vec<SgNode> {
        self.wrap_all(
            self.node
                .get_env()
                .get_multiple_matches(&meta_var)
                .into_iter(),
        )
    }

    #[napi]
    pub fn get_transformed(&self, meta_var: String) -> Option<String> {
        self.node
            .get_env()
            .get_transformed(&meta_var)
            .map(|text| String::from_utf8_lossy(text).into_owned())
    }

    #[napi]
    pub fn find(&self, matcher: Value) -> Result<Option<SgNode>> {
        let matcher = self.matcher(matcher)?;
        Ok(self
            .node
            .find(&matcher)
            .map(|found| SgNode::new(&self.root, found)))
    }

    #[napi]
    pub fn find_all(&self, matcher: Value) -> Result<Vec<SgNode>> {
        let matcher = self.matcher(matcher)?;
        Ok(self
            .node
            .find_all(&matcher)
            .map(|found| SgNode::new(&self.root, found))
            .collect())
    }

    #[napi]
    pub fn replace(&self, text: String) -> Edit {
        let range = self.node.range();
        Edit {
            start_pos: range.start as u32,
            end_pos: range.end as u32,
            inserted_text: text,
        }
    }

    #[napi]
    pub fn commit_edits(&self, edits: Vec<Edit>) -> String {
        // On the root, splice into the whole document: the root node's text
        // leaves out leading whitespace and a BOM.
        let is_root = self.node.parent().is_none();
        let (old, offset) = if is_root {
            (self.root.grep.source().to_string(), 0)
        } else {
            (self.node.text().to_string(), self.node.range().start)
        };
        if edits.is_empty() {
            return old;
        }
        let mut edits = edits;
        edits.sort_by_key(|edit| edit.start_pos);
        let mut text = String::with_capacity(old.len());
        let mut start = 0;
        for edit in edits {
            let at = (edit.start_pos as usize).saturating_sub(offset);
            if start > at {
                continue; // overlapping
            }
            text.push_str(&old[start..at]);
            text.push_str(&edit.inserted_text);
            start = (edit.end_pos as usize).saturating_sub(offset);
        }
        text.push_str(&old[start.min(old.len())..]);
        text
    }

    #[napi]
    pub fn debug(&self) -> String {
        fn format(node: &Node<'_, Doc>, indent: usize) -> String {
            let pad = "  ".repeat(indent);
            let kind = node.kind();
            if !node.is_named() {
                return format!("{pad}[{:?}]\n", node.text());
            }
            if node.children().next().is_some() {
                let mut out = format!("{pad}{kind}:\n");
                for child in node.children() {
                    out.push_str(&format(&child, indent + 1));
                }
                return out;
            }
            let text = node.text();
            let shown = match text.char_indices().nth(40) {
                Some((at, _)) => format!("{}...", &text[..at]),
                None => text.to_string(),
            };
            format!("{pad}{kind}: {shown:?}\n")
        }
        let node: Node<'_, Doc> = self.node.clone().into();
        format(&node, 0)
    }

    #[napi]
    pub fn get_root(&self) -> SgRoot {
        SgRoot {
            inner: Arc::clone(&self.root),
        }
    }

    /// Always `null`: roots parsed in Node have no semantic provider.
    #[napi]
    pub fn definition(&self, _options: Option<Value>) -> Option<Value> {
        None
    }

    /// Always empty: roots parsed in Node have no semantic provider.
    #[napi]
    pub fn references(&self) -> Vec<Value> {
        Vec::new()
    }

    #[napi]
    pub fn type_info(&self) -> Option<String> {
        None
    }
}
