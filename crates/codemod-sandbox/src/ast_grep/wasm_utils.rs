use crate::ast_grep::wasm_lang::{WasmDoc, WasmLang};
use ast_grep_config::{Fixer, RuleConfig, SerializableRuleConfig};
use ast_grep_core::{
    AstGrep, Language, NodeMatch as SgNodeMatch, Pattern,
    matcher::PatternNode,
    meta_var::{MetaVarEnv, MetaVariable},
    replacer::Replacer,
};
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::from_value as from_js_val;
use std::collections::BTreeMap;
use std::error::Error;
use wasm_bindgen::{JsValue, prelude::JsError};
use web_tree_sitter_sg::{Point, TreeCursor};

type NodeMatch<'a> = SgNodeMatch<'a, WasmDoc>;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DumpNode {
    pub id: u32,
    pub field: Option<String>,
    pub kind: String,
    pub start: Pos,
    pub end: Pos,
    pub is_named: bool,
    pub children: Vec<DumpNode>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pos {
    pub row: u32,
    pub column: u32,
}

impl From<Point> for Pos {
    fn from(point: Point) -> Self {
        Self {
            row: point.row(),
            column: point.column(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct WasmNode {
    pub text: String,
    pub range: (usize, usize, usize, usize),
}

#[derive(Serialize, Deserialize)]
pub struct WasmMatch {
    pub id: usize,
    pub node: WasmNode,
    pub env: BTreeMap<String, WasmNode>,
    pub message: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PatternKind {
    Terminal,
    MetaVar,
    Internal,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternTree {
    pub kind: String,
    pub start: Pos,
    pub end: Pos,
    pub is_named: bool,
    pub children: Vec<PatternTree>,
    pub text: Option<String>,
    pub pattern: Option<PatternKind>,
}

pub fn dump_one_node(cursor: &mut TreeCursor, target: &mut Vec<DumpNode>) {
    let node = cursor.current_node();
    let kind = if node.is_missing() {
        format!("MISSING {}", node.type_())
    } else {
        format!("{}", node.type_())
    };
    let start = node.start_position().into();
    let end = node.end_position().into();
    let field = cursor.current_field_name().map(|c| format!("{}", c));
    let mut children = vec![];
    if cursor.goto_first_child() {
        dump_nodes(cursor, &mut children);
        cursor.goto_parent();
    }
    target.push(DumpNode {
        id: node.id(),
        field,
        kind,
        start,
        end,
        children,
        is_named: node.is_named(),
    })
}

fn dump_nodes(cursor: &mut TreeCursor, target: &mut Vec<DumpNode>) {
    loop {
        dump_one_node(cursor, target);
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

pub fn dump_pattern_impl(query: String, selector: Option<String>) -> Result<PatternTree, JsError> {
    let lang = WasmLang::get_current();
    let processed = lang.pre_process_pattern(&query);
    let doc = WasmDoc::try_new(processed.to_string(), lang)?;
    let root = AstGrep::doc(doc);
    let pattern = if let Some(sel) = selector {
        Pattern::contextual(&query, &sel, lang)?
    } else {
        Pattern::try_new(&query, lang)?
    };
    let found = root
        .root()
        .find(&pattern)
        .ok_or_else(|| JsError::new("pattern node not found"))?;
    let ret = dump_pattern_tree(root.root(), found.node_id(), &pattern.node);
    Ok(ret)
}

fn dump_pattern_tree(
    node: ast_grep_core::Node<WasmDoc>,
    node_id: usize,
    pattern: &PatternNode,
) -> PatternTree {
    if node.node_id() == node_id {
        return dump_pattern_impl_inner(node, pattern);
    }
    let children: Vec<_> = node
        .children()
        .map(|n| dump_pattern_tree(n, node_id, pattern))
        .collect();
    let ts = node.get_inner_node().0;
    let text = if children.is_empty() {
        Some(node.text().into())
    } else {
        None
    };
    let kind = if ts.is_missing() {
        format!("MISSING {}", node.kind())
    } else {
        node.kind().to_string()
    };
    PatternTree {
        kind,
        start: ts.start_position().into(),
        end: ts.end_position().into(),
        is_named: node.is_named(),
        children,
        text,
        pattern: None,
    }
}

fn dump_pattern_impl_inner(
    node: ast_grep_core::Node<WasmDoc>,
    pattern: &PatternNode,
) -> PatternTree {
    use PatternNode as PN;
    let ts = node.get_inner_node().0;
    let kind = if ts.is_missing() {
        format!("MISSING {}", node.kind())
    } else {
        node.kind().to_string()
    };
    match pattern {
        PN::MetaVar { .. } => {
            let lang = node.lang();
            let expando = lang.expando_char();
            let text = node.text().to_string();
            let text = text.replace(expando, "$");
            PatternTree {
                kind,
                start: ts.start_position().into(),
                end: ts.end_position().into(),
                is_named: true,
                children: vec![],
                text: Some(text),
                pattern: Some(PatternKind::MetaVar),
            }
        }
        PN::Terminal { is_named, .. } => PatternTree {
            kind,
            start: ts.start_position().into(),
            end: ts.end_position().into(),
            is_named: *is_named,
            children: vec![],
            text: Some(node.text().into()),
            pattern: Some(PatternKind::Terminal),
        },
        PN::Internal { children, .. } => {
            let children = children
                .iter()
                .zip(node.children())
                .map(|(pn, n)| dump_pattern_impl_inner(n, pn))
                .collect();
            PatternTree {
                kind,
                start: ts.start_position().into(),
                end: ts.end_position().into(),
                is_named: true,
                children,
                text: None,
                pattern: Some(PatternKind::Internal),
            }
        }
    }
}

// TODO: move to ast-grep-core
fn get_message(rule: &RuleConfig<WasmLang>, node: &NodeMatch) -> String {
    let parsed = Fixer::from_str(&rule.message, &rule.language).expect("should work");
    let bytes = parsed.generate_replacement(node);
    String::from_utf8_lossy(&bytes).into_owned()
}

impl WasmMatch {
    pub fn from_match(nm: NodeMatch, rule: &RuleConfig<WasmLang>) -> Self {
        let node = nm.get_node().clone();
        let id = node.node_id();
        let node = WasmNode::from(node);
        let env = nm.get_env().clone();
        let env = env_to_map(env);
        let message = get_message(rule, &nm);
        Self {
            node,
            env,
            message,
            id,
        }
    }
}

fn env_to_map(env: MetaVarEnv<'_, WasmDoc>) -> BTreeMap<String, WasmNode> {
    let mut map = BTreeMap::new();
    for id in env.get_matched_variables() {
        match id {
            MetaVariable::Capture(name, _) => {
                if let Some(node) = env.get_match(&name) {
                    map.insert(name, WasmNode::from(node.clone()));
                } else if let Some(bytes) = env.get_transformed(&name) {
                    let node = WasmNode {
                        text: String::from_utf8_lossy(bytes).into_owned(),
                        range: (0, 0, 0, 0),
                    };
                    map.insert(name, WasmNode::from(node));
                }
            }
            MetaVariable::MultiCapture(name) => {
                let nodes = env.get_multiple_matches(&name);
                let (Some(first), Some(last)) = (nodes.first(), nodes.last()) else {
                    continue;
                };
                let start = first.start_pos();
                let end = last.end_pos();

                let text = nodes.iter().map(|n| n.text()).collect();
                let node = WasmNode {
                    text,
                    range: (
                        start.line(),
                        start.column(first),
                        end.line(),
                        end.column(last),
                    ),
                };
                map.insert(name, node);
            }
            // ignore anonymous
            _ => continue,
        }
    }
    map
}

impl From<ast_grep_core::Node<'_, WasmDoc>> for WasmNode {
    fn from(nm: ast_grep_core::Node<WasmDoc>) -> Self {
        let start = nm.start_pos();
        let end = nm.end_pos();
        Self {
            text: nm.text().to_string(),
            range: (start.line(), start.column(&nm), end.line(), end.column(&nm)),
        }
    }
}

pub fn try_get_rule_config(config: JsValue) -> Result<RuleConfig<WasmLang>, JsError> {
    let config: SerializableRuleConfig<WasmLang> = from_js_val(config)?;
    RuleConfig::try_from(config, &Default::default()).map_err(dump_error)
}

fn dump_error(err: impl Error) -> JsError {
    let mut errors = vec![err.to_string()];
    let mut err: &dyn Error = &err;
    while let Some(e) = err.source() {
        errors.push(e.to_string());
        err = e;
    }
    JsError::new(&format!("{}", errors.join("\n")))
}

pub fn convert_to_debug_node(n: ast_grep_core::Node<WasmDoc>) -> DumpNode {
    let mut cursor = n.get_inner_node().0.walk();
    let mut target = vec![];
    dump_one_node(&mut cursor, &mut target);
    target.pop().expect("found empty node")
}
