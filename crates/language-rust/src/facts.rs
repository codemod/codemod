//! Value-flow facts for one Rust file, read off rust-analyzer's syntax tree
//! with its name resolution: a read of `x` is joined to the `let` (or
//! parameter, or pattern) that binds that `x`, and every call is resolved to
//! the function it calls, so [`language_flow`]'s queries can walk them.

use crate::workspace::RaWorkspace;
use language_core::{
    ByteRange, CallEdgeKind, FlowEdgeKind, FlowNodeKind, MutationConfidence, MutationKind,
};
use language_flow::{
    Adjacency, CallFact, CalleeFact, FileFlow, FlowNode, FunctionFact, ImportFact, MutationFact,
    Name,
};
use ra_ap_hir::{
    Access, AsAssocItem, AssocItem, AssocItemContainer, CallableKind, HasSource, ModuleDef,
    PathResolution, Semantics,
};
use ra_ap_ide_db::RootDatabase;
use ra_ap_syntax::{
    ast::{self, HasArgList, HasAttrs, HasLoopBody, HasName},
    AstNode, NodeOrToken, SyntaxKind, SyntaxNode, TextRange,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A function a call resolves to: its file and range.
pub(crate) type Target = (PathBuf, ByteRange, CallEdgeKind);

/// What the facts need besides [`FileFlow`]: what rust-analyzer resolved.
#[derive(Debug, Default)]
pub(crate) struct Resolved {
    /// By call index: the functions the call calls. None when the call goes
    /// through a value (a closure in a local, a function pointer), which the
    /// value-flow queries follow instead.
    pub calls: Vec<Option<Vec<Target>>>,
    /// By function index: why something outside the workspace calls it.
    pub entry_points: FxHashMap<u32, Vec<language_core::EntryPoint>>,
}

/// Methods of std collections and strings that change their receiver, for
/// calls rust-analyzer cannot resolve.
const MUTATING_METHODS: &[&str] = &[
    "push",
    "push_str",
    "push_back",
    "push_front",
    "pop",
    "pop_back",
    "pop_front",
    "insert",
    "remove",
    "clear",
    "extend",
    "append",
    "truncate",
    "retain",
    "drain",
    "sort",
    "sort_by",
    "sort_by_key",
    "sort_unstable",
    "dedup",
    "reverse",
    "swap",
    "entry",
    "get_mut",
    "iter_mut",
    "set",
    "take",
    "replace",
];

pub(crate) fn range_of(range: TextRange) -> ByteRange {
    ByteRange::new(range.start().into(), range.end().into())
}

/// A function's range as ast-grep's tree has it: from its first keyword
/// (or visibility), leaving out attributes and doc comments, which
/// tree-sitter keeps as siblings of the function.
pub(crate) fn fn_range(function: &ast::Fn) -> TextRange {
    let syntax = function.syntax();
    let start = syntax
        .children_with_tokens()
        .find(|child| match child {
            NodeOrToken::Node(node) => node.kind() != SyntaxKind::ATTR,
            NodeOrToken::Token(token) => {
                !matches!(token.kind(), SyntaxKind::COMMENT | SyntaxKind::WHITESPACE)
            }
        })
        .map(|child| child.text_range().start())
        .unwrap_or_else(|| syntax.text_range().start());
    TextRange::new(start, syntax.text_range().end())
}

/// The attributes written on a function, as written inside `#[...]`.
pub(crate) fn attributes(function: &ast::Fn) -> Vec<String> {
    function
        .attrs()
        .filter_map(|attr| attr.meta())
        .map(|meta| meta.syntax().text().to_string())
        .collect()
}

pub(crate) struct Built {
    pub flow: Arc<FileFlow>,
    pub resolved: Resolved,
}

/// Facts for `path` (a file rust-analyzer knows).
pub(crate) fn build(workspace: &RaWorkspace, path: &Path) -> Option<Built> {
    let file_id = workspace.file_id(path)?;
    workspace.with_db(|db| build_in(db, workspace, file_id, path))
}

fn build_in(
    db: &RootDatabase,
    workspace: &RaWorkspace,
    file_id: ra_ap_ide::FileId,
    path: &Path,
) -> Option<Built> {
    let sema = Semantics::new(db);
    let source = sema.parse_guess_edition(file_id);
    let text = source.syntax().text().to_string();
    let mut builder = Builder {
        sema: &sema,
        db,
        workspace,
        path,
        source: &text,
        flow: FileFlow::default(),
        out_edges: Vec::new(),
        in_edges: Vec::new(),
        by_span: FxHashMap::default(),
        names: FxHashSet::default(),
        bindings: FxHashMap::default(),
        functions: Vec::new(),
        fn_nodes: FxHashMap::default(),
        resolved: Resolved::default(),
        pending_fn_reads: Vec::new(),
    };
    builder.items(source.syntax());
    Some(builder.finish())
}

struct Builder<'a> {
    sema: &'a Semantics<'a, RootDatabase>,
    db: &'a RootDatabase,
    workspace: &'a RaWorkspace,
    path: &'a Path,
    source: &'a str,
    flow: FileFlow,
    out_edges: Vec<Vec<(u32, FlowEdgeKind)>>,
    in_edges: Vec<Vec<(u32, FlowEdgeKind)>>,
    by_span: FxHashMap<(u32, u32), u32>,
    names: FxHashSet<Name>,
    /// Binding nodes by the range of the pattern (or `self`) that binds them.
    bindings: FxHashMap<(u32, u32), u32>,
    /// The functions being built, innermost last.
    functions: Vec<u32>,
    /// Function nodes by the function's range.
    fn_nodes: FxHashMap<(u32, u32), u32>,
    resolved: Resolved,
    /// Reads of a function of this file by name (`helper` passed as a
    /// value), joined once every function has its node.
    pending_fn_reads: Vec<(u32, (u32, u32))>,
}

impl<'a> Builder<'a> {
    fn finish(mut self) -> Built {
        for (read, function) in std::mem::take(&mut self.pending_fn_reads) {
            if let Some(&node) = self.fn_nodes.get(&function) {
                self.edge(node, read, FlowEdgeKind::Read);
            }
        }
        let mut flow = self.flow;
        flow.out_edges = Adjacency::from_lists(std::mem::take(&mut self.out_edges));
        flow.in_edges = Adjacency::from_lists(std::mem::take(&mut self.in_edges));
        flow.index_spans();
        flow.bytes = flow.estimate_bytes(&self.names);
        Built {
            flow: Arc::new(flow),
            resolved: self.resolved,
        }
    }

    fn intern(&mut self, name: &str) -> Name {
        if let Some(found) = self.names.get(name) {
            return Arc::clone(found);
        }
        let name: Name = name.into();
        self.names.insert(Arc::clone(&name));
        name
    }

    fn text(&self, range: TextRange) -> &'a str {
        &self.source[usize::from(range.start())..usize::from(range.end())]
    }

    fn function(&self) -> Option<u32> {
        self.functions.last().copied()
    }

    fn node(&mut self, range: TextRange) -> u32 {
        let key = (range.start().into(), range.end().into());
        if let Some(found) = self.by_span.get(&key) {
            return *found;
        }
        let index = self.flow.nodes.len() as u32;
        self.flow.nodes.push(FlowNode {
            range: range_of(range),
            kind: FlowNodeKind::Expression,
            name: None,
            index: None,
            function: self.function(),
            binding: false,
        });
        self.out_edges.push(Vec::new());
        self.in_edges.push(Vec::new());
        self.by_span.insert(key, index);
        index
    }

    /// Describe a node; the first specific description of a range wins.
    fn describe(&mut self, range: TextRange, kind: FlowNodeKind, name: Option<&str>) -> u32 {
        let index = self.node(range);
        let name = name.map(|name| self.intern(name));
        let node = &mut self.flow.nodes[index as usize];
        if node.kind == FlowNodeKind::Expression {
            node.kind = kind;
        }
        if node.name.is_none() {
            node.name = name;
        }
        index
    }

    fn edge(&mut self, from: u32, to: u32, kind: FlowEdgeKind) {
        if from == to || self.out_edges[from as usize].contains(&(to, kind)) {
            return;
        }
        self.out_edges[from as usize].push((to, kind));
        self.in_edges[to as usize].push((from, kind));
    }

    fn skip(&mut self, what: &str) {
        *self.flow.skipped.entry(what.to_string()).or_default() += 1;
    }

    fn mutation(
        &mut self,
        binding: Option<u32>,
        range: TextRange,
        kind: MutationKind,
        confidence: MutationConfidence,
        name: Option<&str>,
    ) {
        let Some(binding) = binding else {
            return;
        };
        let name = name.map(|n| self.intern(n));
        let function = self.function();
        self.flow.mutations.push(MutationFact {
            binding,
            range: range_of(range),
            kind,
            confidence,
            name,
            function,
        });
    }

    // ---- items --------------------------------------------------------

    /// Functions, impls, traits, modules, consts and statics under `node`.
    fn items(&mut self, node: &SyntaxNode) {
        for child in node.children() {
            if let Some(item) = ast::Item::cast(child.clone()) {
                self.item(&item);
            }
        }
    }

    fn item(&mut self, item: &ast::Item) {
        match item {
            ast::Item::Fn(function) => {
                self.function_item(function);
            }
            ast::Item::Impl(implementation) => {
                if let Some(list) = implementation.assoc_item_list() {
                    for assoc in list.assoc_items() {
                        self.assoc_item(&assoc);
                    }
                }
            }
            ast::Item::Trait(tr) => {
                if let Some(list) = tr.assoc_item_list() {
                    for assoc in list.assoc_items() {
                        self.assoc_item(&assoc);
                    }
                }
            }
            ast::Item::Module(module) => {
                if let Some(list) = module.item_list() {
                    self.items(list.syntax());
                }
            }
            ast::Item::Const(constant) => {
                self.global(constant.name(), constant.body(), constant.ty());
            }
            ast::Item::Static(stat) => {
                self.global(stat.name(), stat.body(), stat.ty());
            }
            _ => {}
        }
    }

    fn assoc_item(&mut self, item: &ast::AssocItem) {
        match item {
            ast::AssocItem::Fn(function) => {
                self.function_item(function);
            }
            ast::AssocItem::Const(constant) => {
                self.global(constant.name(), constant.body(), constant.ty());
            }
            _ => {}
        }
    }

    /// A `const` or `static`: a binding at its name, assigned its value.
    fn global(&mut self, name: Option<ast::Name>, body: Option<ast::Expr>, ty: Option<ast::Type>) {
        let Some(name) = name else {
            return;
        };
        let range = name.syntax().text_range();
        let text = name.text().to_string();
        let binding = self.describe(range, FlowNodeKind::Variable, Some(&text));
        self.flow.nodes[binding as usize].binding = true;
        self.bindings
            .insert((range.start().into(), range.end().into()), binding);
        if let Some(ty) = ty {
            let declared = self.intern(&ty.syntax().text().to_string());
            self.flow.declared_types.insert(binding, declared);
        }
        if let Some(body) = body {
            let value = self.expr(&body);
            self.edge(value, binding, FlowEdgeKind::Assign);
        }
    }

    /// A `fn`, with its parameters and body. Returns its function node.
    fn function_item(&mut self, function: &ast::Fn) -> u32 {
        let range = fn_range(function);
        let name = function.name().map(|n| n.text().to_string());
        let parent = self.function();
        let node = self.describe(range, FlowNodeKind::Function, name.as_deref());
        self.flow.nodes[node as usize].kind = FlowNodeKind::Function;
        self.flow.nodes[node as usize].function = parent;
        let index = self.flow.functions.len() as u32;
        let interned = name.as_deref().map(|n| self.intern(n));
        self.flow.functions.push(FunctionFact {
            range: range_of(range),
            node,
            name: interned,
            name_range: function.name().map(|n| range_of(n.syntax().text_range())),
            parent,
            class_node: None,
            accessor: None,
        });
        self.flow.fn_by_node.insert(node, index);
        self.fn_nodes
            .insert((range.start().into(), range.end().into()), node);
        let attrs = attributes(function);
        if let Some(def) = self.sema.to_def(function) {
            let found = crate::entry::entry_points(self.db, def, &attrs);
            if !found.is_empty() {
                self.resolved.entry_points.insert(index, found);
            }
        }
        if !attrs.is_empty() {
            let attrs = attrs.iter().map(|a| self.intern(a)).collect();
            self.flow.decorators.insert(index, attrs);
        }
        if let Some(ret) = function.ret_type().and_then(|r| r.ty()) {
            let declared = self.intern(&ret.syntax().text().to_string());
            self.flow.declared_types.insert(node, declared);
        }

        self.functions.push(index);
        if let Some(params) = function.param_list() {
            self.parameters(&params);
        }
        if let Some(body) = function.body() {
            // Items nested in the body are functions of their own.
            let value = self.block(&body);
            if let Some(value) = value {
                self.edge(value, node, FlowEdgeKind::Return);
            }
        }
        self.functions.pop();
        node
    }

    fn parameters(&mut self, params: &ast::ParamList) {
        if let Some(self_param) = params.self_param() {
            let range = self_param.syntax().text_range();
            let binding = self.describe(range, FlowNodeKind::Parameter, Some("self"));
            self.flow.nodes[binding as usize].binding = true;
            self.bindings
                .insert((range.start().into(), range.end().into()), binding);
        }
        for (position, param) in params.params().enumerate() {
            let Some(pat) = param.pat() else {
                continue;
            };
            let declared = param.ty().map(|t| t.syntax().text().to_string());
            let start = self.flow.nodes.len();
            if let ast::Pat::IdentPat(ident) = &pat {
                let binding = self.ident_binding(ident, FlowNodeKind::Parameter);
                self.flow.nodes[binding as usize].index = Some(position as u32);
                if let Some(declared) = &declared {
                    let declared = self.intern(declared);
                    self.flow.declared_types.insert(binding, declared);
                }
            } else {
                // A destructured parameter: the whole value is the
                // parameter, its parts are bound from it.
                let range = pat.syntax().text_range();
                let whole = self.describe(range, FlowNodeKind::Parameter, None);
                self.flow.nodes[whole as usize].binding = true;
                self.flow.nodes[whole as usize].index = Some(position as u32);
                self.bind_pattern_parts(&pat, whole);
            }
            let _ = start;
        }
    }

    fn ident_binding(&mut self, ident: &ast::IdentPat, kind: FlowNodeKind) -> u32 {
        let range = ident.syntax().text_range();
        let name = ident.name();
        // The node is the name alone (`sum` in `let mut sum`): that is the
        // node a codemod finds, while rust-analyzer knows the local by its
        // whole pattern, so the binding is kept under that.
        let at = name.as_ref().map_or(range, |n| n.syntax().text_range());
        let name = name.map(|n| n.text().to_string());
        let binding = self.describe(at, kind, name.as_deref());
        self.flow.nodes[binding as usize].binding = true;
        self.bindings
            .insert((range.start().into(), range.end().into()), binding);
        binding
    }

    // ---- patterns -----------------------------------------------------

    /// Bind `pat` to the value at node `value`.
    fn bind_pattern(&mut self, pat: &ast::Pat, value: u32, edge: FlowEdgeKind) {
        match pat {
            ast::Pat::IdentPat(ident) => {
                let binding = self.ident_binding(ident, FlowNodeKind::Variable);
                self.edge(value, binding, edge);
                if let Some(inner) = ident.pat() {
                    // `x @ Some(_)`
                    self.bind_pattern(&inner, binding, FlowEdgeKind::Alias);
                }
            }
            ast::Pat::WildcardPat(_) | ast::Pat::LiteralPat(_) | ast::Pat::RangePat(_) => {}
            // `let (a, b) = (x, y)`: the parts come straight from the
            // value, so a part that is an element of a tuple literal is
            // followed to that element alone.
            _ => self.bind_pattern_parts(pat, value),
        }
    }

    /// The bindings inside a destructuring pattern whose whole value is at
    /// node `whole`.
    fn bind_pattern_parts(&mut self, pat: &ast::Pat, whole: u32) {
        match pat {
            ast::Pat::IdentPat(_) => self.bind_pattern(pat, whole, FlowEdgeKind::Alias),
            ast::Pat::TuplePat(tuple) => {
                for (position, field) in tuple.fields().enumerate() {
                    self.element(&field, whole, position as u32);
                }
            }
            ast::Pat::SlicePat(slice) => {
                for (position, field) in slice.pats().enumerate() {
                    self.element(&field, whole, position as u32);
                }
            }
            ast::Pat::TupleStructPat(tuple) => {
                // `Some(x)`, `Ok(v)`, `Point(a, b)`: the payload's parts.
                for (position, field) in tuple.fields().enumerate() {
                    self.element(&field, whole, position as u32);
                }
            }
            ast::Pat::RecordPat(record) => {
                let Some(list) = record.record_pat_field_list() else {
                    return;
                };
                for field in list.fields() {
                    let Some(pat) = field.pat() else {
                        continue;
                    };
                    let key = field
                        .field_name()
                        .map(|n| n.text().to_string())
                        .unwrap_or_default();
                    self.property(&pat, whole, &key);
                }
            }
            ast::Pat::RefPat(r) => {
                if let Some(inner) = r.pat() {
                    self.bind_pattern(&inner, whole, FlowEdgeKind::Alias);
                }
            }
            ast::Pat::BoxPat(b) => {
                if let Some(inner) = b.pat() {
                    self.bind_pattern(&inner, whole, FlowEdgeKind::Alias);
                }
            }
            ast::Pat::ParenPat(p) => {
                if let Some(inner) = p.pat() {
                    self.bind_pattern(&inner, whole, FlowEdgeKind::Alias);
                }
            }
            ast::Pat::OrPat(or) => {
                for alternative in or.pats() {
                    self.bind_pattern(&alternative, whole, FlowEdgeKind::Alias);
                }
            }
            _ => {}
        }
    }

    fn element(&mut self, pat: &ast::Pat, whole: u32, position: u32) {
        match pat {
            ast::Pat::RestPat(_) | ast::Pat::WildcardPat(_) => {}
            ast::Pat::IdentPat(ident) => {
                let binding = self.ident_binding(ident, FlowNodeKind::Variable);
                self.edge(whole, binding, FlowEdgeKind::Element);
                self.flow.element_index.insert((whole, binding), position);
                if let Some(inner) = ident.pat() {
                    self.bind_pattern(&inner, binding, FlowEdgeKind::Alias);
                }
            }
            _ => {
                let part = self.node(pat.syntax().text_range());
                self.edge(whole, part, FlowEdgeKind::Element);
                self.flow.element_index.insert((whole, part), position);
                self.bind_pattern_parts(pat, part);
            }
        }
    }

    fn property(&mut self, pat: &ast::Pat, whole: u32, key: &str) {
        let part = match pat {
            ast::Pat::IdentPat(ident) => {
                let binding = self.ident_binding(ident, FlowNodeKind::Variable);
                if let Some(inner) = ident.pat() {
                    self.bind_pattern(&inner, binding, FlowEdgeKind::Alias);
                }
                binding
            }
            ast::Pat::WildcardPat(_) | ast::Pat::RestPat(_) => return,
            _ => {
                let part = self.node(pat.syntax().text_range());
                self.bind_pattern_parts(pat, part);
                part
            }
        };
        self.edge(whole, part, FlowEdgeKind::Property);
        self.flow
            .property_keys
            .insert((whole, part), key.to_string());
    }

    // ---- statements and blocks ---------------------------------------

    /// A block's statements; returns the node of its value (the tail
    /// expression), if it has one.
    fn block(&mut self, block: &ast::BlockExpr) -> Option<u32> {
        let list = block.stmt_list()?;
        for statement in list.statements() {
            self.statement(&statement);
        }
        let tail = list.tail_expr()?;
        Some(self.expr(&tail))
    }

    fn statement(&mut self, statement: &ast::Stmt) {
        match statement {
            ast::Stmt::LetStmt(let_stmt) => {
                let value = let_stmt.initializer().map(|init| self.expr(&init));
                if let Some(else_branch) = let_stmt.let_else().and_then(|e| e.block_expr()) {
                    self.block(&else_branch);
                }
                let Some(pat) = let_stmt.pat() else {
                    return;
                };
                let declared = let_stmt.ty().map(|t| t.syntax().text().to_string());
                match value {
                    Some(value) => self.bind_pattern(&pat, value, FlowEdgeKind::Assign),
                    None => {
                        // `let x;`: assigned later.
                        if let ast::Pat::IdentPat(ident) = &pat {
                            self.ident_binding(ident, FlowNodeKind::Variable);
                        }
                    }
                }
                if let (Some(declared), ast::Pat::IdentPat(ident)) = (declared, &pat) {
                    let range = ident.syntax().text_range();
                    if let Some(&binding) = self
                        .bindings
                        .get(&(range.start().into(), range.end().into()))
                    {
                        let declared = self.intern(&declared);
                        self.flow.declared_types.insert(binding, declared);
                    }
                }
            }
            ast::Stmt::ExprStmt(expr_stmt) => {
                if let Some(expr) = expr_stmt.expr() {
                    self.expr(&expr);
                }
            }
            ast::Stmt::Item(item) => self.item(item),
        }
    }

    // ---- expressions --------------------------------------------------

    /// The node of an expression's value.
    fn expr(&mut self, expr: &ast::Expr) -> u32 {
        let range = expr.syntax().text_range();
        match expr {
            ast::Expr::PathExpr(path_expr) => self.path_expr(path_expr),
            ast::Expr::Literal(_) => {
                let text = self.text(range).to_string();
                self.describe(range, FlowNodeKind::Literal, Some(&text))
            }
            ast::Expr::ParenExpr(paren) => {
                let node = self.node(range);
                if let Some(inner) = paren.expr() {
                    let inner = self.expr(&inner);
                    self.edge(inner, node, FlowEdgeKind::Alias);
                }
                node
            }
            ast::Expr::RefExpr(r) => {
                let node = self.node(range);
                if let Some(inner) = r.expr() {
                    let inner = self.expr(&inner);
                    self.edge(inner, node, FlowEdgeKind::Alias);
                }
                node
            }
            ast::Expr::PrefixExpr(prefix) => {
                let node = self.node(range);
                if let Some(inner) = prefix.expr() {
                    let inner = self.expr(&inner);
                    let kind = match prefix.op_kind() {
                        Some(ast::UnaryOp::Deref) => FlowEdgeKind::Alias,
                        _ => FlowEdgeKind::Derive,
                    };
                    self.edge(inner, node, kind);
                }
                node
            }
            ast::Expr::BinExpr(bin) => self.bin_expr(bin, range),
            ast::Expr::CallExpr(call) => self.call_expr(call),
            ast::Expr::MethodCallExpr(call) => self.method_call(call),
            ast::Expr::FieldExpr(field) => {
                let name = field.name_ref().map(|n| n.text().to_string());
                let node = self.describe(range, FlowNodeKind::Member, name.as_deref());
                if let Some(object) = field.expr() {
                    let object = self.expr(&object);
                    self.edge(object, node, FlowEdgeKind::Property);
                }
                node
            }
            ast::Expr::IndexExpr(index) => {
                let position = index.index().and_then(|i| match i {
                    ast::Expr::Literal(_) => Some(i.syntax().text().to_string()),
                    _ => None,
                });
                let node = self.describe(range, FlowNodeKind::Member, position.as_deref());
                if let Some(base) = index.base() {
                    let base = self.expr(&base);
                    self.edge(base, node, FlowEdgeKind::Element);
                }
                if let Some(i) = index.index() {
                    self.expr(&i);
                }
                node
            }
            ast::Expr::RecordExpr(record) => {
                let name = record.path().map(|p| p.syntax().text().to_string());
                let node = self.describe(range, FlowNodeKind::Object, name.as_deref());
                if let Some(list) = record.record_expr_field_list() {
                    for field in list.fields() {
                        let key = field
                            .field_name()
                            .map(|n| n.text().to_string())
                            .unwrap_or_default();
                        let value = match field.expr() {
                            Some(value) => self.expr(&value),
                            None => continue,
                        };
                        self.edge(value, node, FlowEdgeKind::Capture);
                        let key = self.intern(&key);
                        self.flow
                            .object_members
                            .entry(node)
                            .or_default()
                            .push((key, value));
                    }
                    if let Some(spread) = list.spread() {
                        let spread = self.expr(&spread);
                        self.edge(spread, node, FlowEdgeKind::Capture);
                    }
                }
                node
            }
            ast::Expr::TupleExpr(tuple) => {
                let items: Vec<ast::Expr> = tuple.fields().collect();
                self.sequence(range, &items)
            }
            ast::Expr::ArrayExpr(array) => {
                let items: Vec<ast::Expr> = array.exprs().collect();
                self.sequence(range, &items)
            }
            ast::Expr::BlockExpr(block) => {
                let node = self.node(range);
                if let Some(value) = self.block(block) {
                    self.edge(value, node, FlowEdgeKind::Alias);
                }
                node
            }
            ast::Expr::IfExpr(if_expr) => {
                let node = self.node(range);
                if let Some(condition) = if_expr.condition() {
                    self.expr(&condition);
                }
                if let Some(then) = if_expr.then_branch() {
                    if let Some(value) = self.block(&then) {
                        self.edge(value, node, FlowEdgeKind::Alias);
                    }
                }
                match if_expr.else_branch() {
                    Some(ast::ElseBranch::Block(block)) => {
                        if let Some(value) = self.block(&block) {
                            self.edge(value, node, FlowEdgeKind::Alias);
                        }
                    }
                    Some(ast::ElseBranch::IfExpr(nested)) => {
                        let value = self.expr(&ast::Expr::IfExpr(nested));
                        self.edge(value, node, FlowEdgeKind::Alias);
                    }
                    None => {}
                }
                node
            }
            ast::Expr::LetExpr(let_expr) => {
                // `if let pat = value`, `while let pat = value`
                let node = self.node(range);
                if let Some(value) = let_expr.expr() {
                    let value = self.expr(&value);
                    if let Some(pat) = let_expr.pat() {
                        self.bind_pattern(&pat, value, FlowEdgeKind::Assign);
                    }
                }
                node
            }
            ast::Expr::MatchExpr(match_expr) => {
                let node = self.node(range);
                let scrutinee = match_expr.expr().map(|e| self.expr(&e));
                if let Some(list) = match_expr.match_arm_list() {
                    for arm in list.arms() {
                        if let (Some(pat), Some(scrutinee)) = (arm.pat(), scrutinee) {
                            self.bind_pattern(&pat, scrutinee, FlowEdgeKind::Assign);
                        }
                        if let Some(guard) = arm.guard().and_then(|g| g.condition()) {
                            self.expr(&guard);
                        }
                        if let Some(value) = arm.expr() {
                            let value = self.expr(&value);
                            self.edge(value, node, FlowEdgeKind::Alias);
                        }
                    }
                }
                node
            }
            ast::Expr::ForExpr(for_expr) => {
                let node = self.node(range);
                if let Some(iterable) = for_expr.iterable() {
                    let iterable = self.expr(&iterable);
                    if let Some(pat) = for_expr.pat() {
                        self.bind_iteration(&pat, iterable);
                    }
                }
                if let Some(body) = for_expr.loop_body() {
                    self.flow.loops.push(range_of(range));
                    self.block(&body);
                }
                node
            }
            ast::Expr::WhileExpr(while_expr) => {
                let node = self.node(range);
                if let Some(condition) = while_expr.condition() {
                    self.expr(&condition);
                }
                if let Some(body) = while_expr.loop_body() {
                    self.flow.loops.push(range_of(range));
                    self.block(&body);
                }
                node
            }
            ast::Expr::LoopExpr(loop_expr) => {
                let node = self.node(range);
                if let Some(body) = loop_expr.loop_body() {
                    self.flow.loops.push(range_of(range));
                    self.block(&body);
                }
                node
            }
            ast::Expr::ReturnExpr(ret) => {
                let node = self.node(range);
                if let Some(value) = ret.expr() {
                    let value = self.expr(&value);
                    if let Some(function) = self.function() {
                        let function_node = self.flow.functions[function as usize].node;
                        self.edge(value, function_node, FlowEdgeKind::Return);
                    }
                }
                node
            }
            ast::Expr::ClosureExpr(closure) => self.closure(closure),
            ast::Expr::TryExpr(t) => self.derived(range, t.expr()),
            ast::Expr::AwaitExpr(a) => self.derived(range, a.expr()),
            ast::Expr::CastExpr(c) => self.derived(range, c.expr()),
            ast::Expr::MacroExpr(m) => match m.macro_call() {
                Some(call) => self.macro_call(&call, range),
                None => self.node(range),
            },
            _ => {
                // Anything else: computed from the expressions inside it.
                let node = self.node(range);
                let children: Vec<ast::Expr> = expr
                    .syntax()
                    .children()
                    .filter_map(ast::Expr::cast)
                    .collect();
                for child in children {
                    let child = self.expr(&child);
                    self.edge(child, node, FlowEdgeKind::Derive);
                }
                node
            }
        }
    }

    fn derived(&mut self, range: TextRange, inner: Option<ast::Expr>) -> u32 {
        let node = self.node(range);
        if let Some(inner) = inner {
            let inner = self.expr(&inner);
            self.edge(inner, node, FlowEdgeKind::Derive);
        }
        node
    }

    /// `(a, b)` and `[a, b]`.
    fn sequence(&mut self, range: TextRange, items: &[ast::Expr]) -> u32 {
        let node = self.describe(range, FlowNodeKind::Array, None);
        let mut elements = Vec::with_capacity(items.len());
        for item in items {
            let value = self.expr(item);
            self.edge(value, node, FlowEdgeKind::Capture);
            elements.push(Some(value));
        }
        self.flow.array_elements.insert(node, elements);
        node
    }

    /// `for pat in iterable`: each element of the iterable.
    fn bind_iteration(&mut self, pat: &ast::Pat, iterable: u32) {
        match pat {
            ast::Pat::IdentPat(ident) => {
                let binding = self.ident_binding(ident, FlowNodeKind::Variable);
                self.edge(iterable, binding, FlowEdgeKind::Element);
            }
            ast::Pat::WildcardPat(_) => {}
            _ => {
                let item = self.node(pat.syntax().text_range());
                self.edge(iterable, item, FlowEdgeKind::Element);
                self.bind_pattern_parts(pat, item);
            }
        }
    }

    fn bin_expr(&mut self, bin: &ast::BinExpr, range: TextRange) -> u32 {
        let node = self.node(range);
        match bin.op_kind() {
            Some(ast::BinaryOp::Assignment { op }) => {
                let value = bin.rhs().map(|rhs| self.expr(&rhs));
                let Some(target) = bin.lhs() else {
                    return node;
                };
                self.assign(&target, value, op.is_some(), range);
            }
            _ => {
                for side in [bin.lhs(), bin.rhs()].into_iter().flatten() {
                    let side = self.expr(&side);
                    self.edge(side, node, FlowEdgeKind::Derive);
                }
            }
        }
        node
    }

    /// `target = value` (or `target += value` when `compound`).
    fn assign(&mut self, target: &ast::Expr, value: Option<u32>, compound: bool, whole: TextRange) {
        match target {
            ast::Expr::PathExpr(path_expr) => {
                let binding = self.local_binding(path_expr);
                let target_node = self.path_expr(path_expr);
                let to = binding.unwrap_or(target_node);
                if let Some(value) = value {
                    self.edge(value, to, FlowEdgeKind::Assign);
                }
                let kind = if compound {
                    MutationKind::Update
                } else {
                    MutationKind::Reassign
                };
                self.mutation(binding, whole, kind, MutationConfidence::Definite, None);
            }
            ast::Expr::FieldExpr(field) => {
                let name = field.name_ref().map(|n| n.text().to_string());
                let target_node = self.describe(
                    target.syntax().text_range(),
                    FlowNodeKind::Member,
                    name.as_deref(),
                );
                if let Some(object) = field.expr() {
                    let object_node = self.expr(&object);
                    self.edge(object_node, target_node, FlowEdgeKind::Property);
                    let binding = self.root_binding(&object);
                    if let Some(value) = value {
                        self.edge(value, object_node, FlowEdgeKind::Store);
                    }
                    let kind = if compound {
                        MutationKind::Update
                    } else {
                        MutationKind::PropertyWrite
                    };
                    self.mutation(
                        binding,
                        whole,
                        kind,
                        MutationConfidence::Definite,
                        name.as_deref(),
                    );
                }
                if let Some(value) = value {
                    self.edge(value, target_node, FlowEdgeKind::Assign);
                }
            }
            ast::Expr::IndexExpr(index) => {
                let target_node = self.node(target.syntax().text_range());
                if let Some(base) = index.base() {
                    let base_node = self.expr(&base);
                    self.edge(base_node, target_node, FlowEdgeKind::Element);
                    let binding = self.root_binding(&base);
                    if let Some(value) = value {
                        self.edge(value, base_node, FlowEdgeKind::Store);
                    }
                    let kind = if compound {
                        MutationKind::Update
                    } else {
                        MutationKind::PropertyWrite
                    };
                    self.mutation(binding, whole, kind, MutationConfidence::Definite, None);
                }
                if let Some(i) = index.index() {
                    self.expr(&i);
                }
            }
            ast::Expr::PrefixExpr(prefix) => {
                // `*r = v`: writes what r points to.
                if let Some(inner) = prefix.expr() {
                    let inner_node = self.expr(&inner);
                    if let Some(value) = value {
                        self.edge(value, inner_node, FlowEdgeKind::Store);
                    }
                    let binding = self.root_binding(&inner);
                    let kind = if compound {
                        MutationKind::Update
                    } else {
                        MutationKind::Reassign
                    };
                    self.mutation(binding, whole, kind, MutationConfidence::Definite, None);
                }
            }
            other => {
                let node = self.expr(other);
                if let Some(value) = value {
                    self.edge(value, node, FlowEdgeKind::Assign);
                }
            }
        }
    }

    /// The local binding an expression changes when it is written through:
    /// `v` in `v.x.y = 1`, `v[0] = 1`, `v.push(1)`.
    fn root_binding(&mut self, expr: &ast::Expr) -> Option<u32> {
        match expr {
            ast::Expr::PathExpr(path_expr) => self.local_binding(path_expr),
            ast::Expr::FieldExpr(f) => self.root_binding(&f.expr()?),
            ast::Expr::IndexExpr(i) => self.root_binding(&i.base()?),
            ast::Expr::ParenExpr(p) => self.root_binding(&p.expr()?),
            ast::Expr::RefExpr(r) => self.root_binding(&r.expr()?),
            ast::Expr::PrefixExpr(p) => self.root_binding(&p.expr()?),
            ast::Expr::MethodCallExpr(m) => {
                // `v.borrow_mut().push(1)`, `v.lock().unwrap().insert(..)`
                self.root_binding(&m.receiver()?)
            }
            _ => None,
        }
    }

    /// The binding node of the local a path names.
    fn local_binding(&mut self, path_expr: &ast::PathExpr) -> Option<u32> {
        let path = path_expr.path()?;
        let PathResolution::Local(local) = self.sema.resolve_path(&path)? else {
            return None;
        };
        local.sources(self.db).iter().find_map(|source| {
            let range = source.syntax().text_range();
            self.bindings
                .get(&(range.start().into(), range.end().into()))
                .copied()
        })
    }

    fn path_expr(&mut self, path_expr: &ast::PathExpr) -> u32 {
        let range = path_expr.syntax().text_range();
        let text = self.text(range).to_string();
        if let Some(binding) = self.local_binding(path_expr) {
            let node = self.describe(range, FlowNodeKind::Reference, Some(&text));
            self.edge(binding, node, FlowEdgeKind::Read);
            return node;
        }
        let Some(path) = path_expr.path() else {
            return self.node(range);
        };
        let resolution = self.sema.resolve_path(&path);
        let short = path
            .segment()
            .and_then(|s| s.name_ref())
            .map(|n| n.text().to_string())
            .unwrap_or_else(|| text.clone());
        match resolution {
            Some(PathResolution::Def(ModuleDef::Function(function))) => {
                // A function used as a value: `helper`, `items.map(parse)`.
                match self.function_location(function) {
                    Some((target, target_range)) if target == self.path => {
                        let node = self.describe(range, FlowNodeKind::Reference, Some(&short));
                        self.pending_fn_reads
                            .push((node, (target_range.start, target_range.end)));
                        node
                    }
                    Some((target, target_range)) => {
                        let node = self.describe(range, FlowNodeKind::Import, Some(&short));
                        self.flow.imports.insert(
                            node,
                            ImportFact {
                                module: target.to_string_lossy().into_owned(),
                                name: format!("{}-{}", target_range.start, target_range.end),
                                namespace: false,
                            },
                        );
                        node
                    }
                    None => self.describe(range, FlowNodeKind::Global, Some(&short)),
                }
            }
            Some(PathResolution::Def(ModuleDef::Const(_) | ModuleDef::Static(_))) => {
                let target = self.global_location(&resolution);
                match target {
                    Some((target, target_range)) if target == self.path => {
                        let node = self.describe(range, FlowNodeKind::Reference, Some(&short));
                        if let Some(&binding) =
                            self.bindings.get(&(target_range.start, target_range.end))
                        {
                            self.edge(binding, node, FlowEdgeKind::Read);
                        }
                        node
                    }
                    Some((target, target_range)) => {
                        let node = self.describe(range, FlowNodeKind::Import, Some(&short));
                        self.flow.imports.insert(
                            node,
                            ImportFact {
                                module: target.to_string_lossy().into_owned(),
                                name: format!("{}-{}", target_range.start, target_range.end),
                                namespace: false,
                            },
                        );
                        node
                    }
                    None => self.describe(range, FlowNodeKind::Global, Some(&short)),
                }
            }
            Some(PathResolution::Local(_)) => {
                // A local bound where this builder does not look (a macro).
                self.describe(range, FlowNodeKind::Reference, Some(&text))
            }
            Some(PathResolution::SelfType(_)) | Some(PathResolution::Def(_)) => {
                // `None`, `MyUnit`, `Self`: a value written in place.
                self.describe(range, FlowNodeKind::Literal, Some(&text))
            }
            _ => self.describe(range, FlowNodeKind::Global, Some(&short)),
        }
    }

    fn global_location(&self, resolution: &Option<PathResolution>) -> Option<(PathBuf, ByteRange)> {
        let name_range = |name: Option<ast::Name>, file: ra_ap_hir::HirFileId| {
            let file = file.file_id()?.file_id(self.db);
            let path = self.workspace.path(file)?;
            Some((path, range_of(name?.syntax().text_range())))
        };
        match resolution {
            Some(PathResolution::Def(ModuleDef::Const(c))) => {
                let source = c.source(self.db)?;
                name_range(source.value.name(), source.file_id)
            }
            Some(PathResolution::Def(ModuleDef::Static(s))) => {
                let source = s.source(self.db)?;
                name_range(source.value.name(), source.file_id)
            }
            _ => None,
        }
    }

    /// Where a function is written, when it is in the workspace.
    fn function_location(&self, function: ra_ap_hir::Function) -> Option<(PathBuf, ByteRange)> {
        let source = function.source(self.db)?;
        let file = source.file_id.file_id()?.file_id(self.db);
        let path = self.workspace.path(file)?;
        if !self.workspace.is_local(&path) {
            return None;
        }
        Some((path, range_of(fn_range(&source.value))))
    }

    /// The workspace functions `function` stands for when called: itself,
    /// and for a trait method called on a generic or `dyn` receiver, the
    /// methods of the same name in the workspace's impls of that trait.
    fn targets_of(&self, function: ra_ap_hir::Function, via: CallEdgeKind) -> Vec<Target> {
        let mut targets = Vec::new();
        let is_trait_method = matches!(
            function
                .as_assoc_item(self.db)
                .map(|a| a.container(self.db)),
            Some(AssocItemContainer::Trait(_))
        );
        if !is_trait_method || function.has_body(self.db) {
            if let Some((path, range)) = self.function_location(function) {
                targets.push((path, range, via));
            }
        }
        if let Some(AssocItemContainer::Trait(tr)) = function
            .as_assoc_item(self.db)
            .map(|a| a.container(self.db))
        {
            let name = function.name(self.db);
            for implementation in ra_ap_hir::Impl::all_for_trait(self.db, tr) {
                for item in implementation.items(self.db) {
                    if let AssocItem::Function(f) = item {
                        if f.name(self.db) == name {
                            if let Some((path, range)) = self.function_location(f) {
                                targets.push((path, range, CallEdgeKind::Type));
                            }
                        }
                    }
                }
            }
        }
        targets
    }

    fn record_call(
        &mut self,
        range: TextRange,
        node: u32,
        callee: CalleeFact,
        callee_text: &str,
        args: Vec<u32>,
        targets: Option<Vec<Target>>,
    ) {
        let callee_text = self.intern(callee_text);
        let function = self.function();
        self.flow
            .call_by_node
            .insert(node, self.flow.calls.len() as u32);
        self.flow.calls.push(CallFact {
            range: range_of(range),
            function,
            callee,
            callee_text,
            args: args.into_boxed_slice(),
            is_new: false,
            is_jsx: false,
        });
        self.resolved.calls.push(targets);
    }

    fn arguments(
        &mut self,
        list: Option<ast::ArgList>,
        call_node: u32,
        whole: TextRange,
    ) -> Vec<u32> {
        let mut args = Vec::new();
        let Some(list) = list else {
            return args;
        };
        for argument in list.args() {
            let value = self.expr(&argument);
            self.edge(value, call_node, FlowEdgeKind::Argument);
            args.push(value);
            // `f(&mut x)`: f may change x.
            if let ast::Expr::RefExpr(r) = &argument {
                if r.mut_token().is_some() {
                    if let Some(inner) = r.expr() {
                        let binding = self.root_binding(&inner);
                        self.mutation(
                            binding,
                            whole,
                            MutationKind::Escape,
                            MutationConfidence::Likely,
                            None,
                        );
                    }
                }
            }
        }
        args
    }

    fn call_expr(&mut self, call: &ast::CallExpr) -> u32 {
        let range = call.syntax().text_range();
        let node = self.node(range);
        let Some(callee_expr) = call.expr() else {
            return node;
        };
        let callee_text = self.text(callee_expr.syntax().text_range()).to_string();
        let callee = self.expr(&callee_expr);
        let args = self.arguments(call.arg_list(), node, range);
        let (targets, via) = match self
            .sema
            .resolve_expr_as_callable(&callee_expr)
            .map(|c| c.kind())
        {
            Some(CallableKind::Function(function)) => {
                let via = if function.self_param(self.db).is_some() {
                    // `Type::method(x)`: a method called as a function.
                    CallEdgeKind::Method
                } else {
                    CallEdgeKind::Direct
                };
                (Some(self.targets_of(function, via)), via)
            }
            Some(CallableKind::TupleStruct(_) | CallableKind::TupleEnumVariant(_)) => {
                (Some(Vec::new()), CallEdgeKind::Constructor)
            }
            // A closure or function pointer: follow the value.
            _ => (None, CallEdgeKind::Direct),
        };
        let name = if via == CallEdgeKind::Constructor {
            FlowNodeKind::Object
        } else {
            FlowNodeKind::Call
        };
        self.describe(range, name, Some(&callee_text));
        if via == CallEdgeKind::Constructor {
            // `Some(x)`, `Point(a, b)`: a value holding its arguments.
            for (position, arg) in args.iter().enumerate() {
                self.edge(*arg, node, FlowEdgeKind::Capture);
                let _ = position;
            }
            self.flow
                .array_elements
                .insert(node, args.iter().map(|a| Some(*a)).collect());
            return node;
        }
        self.record_call(
            range,
            node,
            CalleeFact::Node(callee),
            &callee_text,
            args,
            targets,
        );
        node
    }

    fn method_call(&mut self, call: &ast::MethodCallExpr) -> u32 {
        let range = call.syntax().text_range();
        let node = self.node(range);
        let Some(receiver) = call.receiver() else {
            return node;
        };
        let Some(name_ref) = call.name_ref() else {
            return node;
        };
        let name = name_ref.text().to_string();
        let object = self.expr(&receiver);
        // `x.m` as one node, the way ast-grep's tree has it (a field
        // expression called).
        let member_range = TextRange::new(
            receiver.syntax().text_range().start(),
            name_ref.syntax().text_range().end(),
        );
        let member = self.describe(member_range, FlowNodeKind::Member, Some(&name));
        self.edge(object, member, FlowEdgeKind::Property);
        self.edge(object, node, FlowEdgeKind::Receiver);
        let args = self.arguments(call.arg_list(), node, range);
        let resolved = self.sema.resolve_method_call(call);
        let binding = self.root_binding(&receiver);
        match resolved {
            Some(function) => {
                if let Some(self_param) = function.self_param(self.db) {
                    if self_param.access(self.db) == Access::Exclusive {
                        self.mutation(
                            binding,
                            range,
                            MutationKind::MethodCall,
                            MutationConfidence::Likely,
                            Some(&name),
                        );
                    }
                }
            }
            None => {
                let confidence = if MUTATING_METHODS.contains(&name.as_str()) {
                    MutationConfidence::Likely
                } else {
                    MutationConfidence::Maybe
                };
                self.mutation(
                    binding,
                    range,
                    MutationKind::MethodCall,
                    confidence,
                    Some(&name),
                );
            }
        }
        let targets = resolved.map(|f| self.targets_of(f, CallEdgeKind::Method));
        let callee_text = self.text(member_range).to_string();
        self.describe(range, FlowNodeKind::Call, Some(&callee_text));
        let property = self.intern(&name);
        self.record_call(
            range,
            node,
            CalleeFact::Member { object, property },
            &callee_text,
            args,
            // Unresolved methods are left to the values, which find
            // nothing: the call answers as unresolved.
            Some(targets.unwrap_or_default()),
        );
        node
    }

    fn closure(&mut self, closure: &ast::ClosureExpr) -> u32 {
        let range = closure.syntax().text_range();
        let parent = self.function();
        let node = self.describe(range, FlowNodeKind::Function, None);
        self.flow.nodes[node as usize].kind = FlowNodeKind::Function;
        let index = self.flow.functions.len() as u32;
        self.flow.functions.push(FunctionFact {
            range: range_of(range),
            node,
            name: None,
            name_range: None,
            parent,
            class_node: None,
            accessor: None,
        });
        self.flow.fn_by_node.insert(node, index);
        self.functions.push(index);
        if let Some(params) = closure.param_list() {
            for (position, param) in params.params().enumerate() {
                let Some(pat) = param.pat() else {
                    continue;
                };
                if let ast::Pat::IdentPat(ident) = &pat {
                    let binding = self.ident_binding(ident, FlowNodeKind::Parameter);
                    self.flow.nodes[binding as usize].index = Some(position as u32);
                } else {
                    let whole =
                        self.describe(pat.syntax().text_range(), FlowNodeKind::Parameter, None);
                    self.flow.nodes[whole as usize].binding = true;
                    self.flow.nodes[whole as usize].index = Some(position as u32);
                    self.bind_pattern_parts(&pat, whole);
                }
            }
        }
        if let Some(body) = closure.body() {
            let value = self.expr(&body);
            self.edge(value, node, FlowEdgeKind::Return);
        }
        self.functions.pop();
        node
    }

    /// `format!("{}", x)`, `vec![a, b]`, `println!(..)`: the macro's value
    /// is computed from the names written in its arguments. Each name that
    /// resolves to a local is read where it is written.
    fn macro_call(&mut self, call: &ast::MacroCall, range: TextRange) -> u32 {
        let macro_name = call
            .path()
            .and_then(|p| p.segment())
            .and_then(|s| s.name_ref())
            .map(|n| n.text().to_string())
            .unwrap_or_default();
        let is_vec = macro_name == "vec";
        let kind = if is_vec {
            FlowNodeKind::Array
        } else {
            FlowNodeKind::Expression
        };
        let node = self.describe(range, kind, Some(&format!("{macro_name}!")));
        let Some(tokens) = call.token_tree() else {
            return node;
        };
        let edge = if is_vec {
            FlowEdgeKind::Capture
        } else {
            FlowEdgeKind::Derive
        };
        let mut seen_any = false;
        for token in tokens
            .syntax()
            .descendants_with_tokens()
            .filter_map(|t| t.into_token())
            .filter(|t| t.kind() == SyntaxKind::IDENT)
        {
            // Skip `x` in `a.x` (a field name, not a variable).
            if token
                .prev_token()
                .is_some_and(|p| p.kind() == SyntaxKind::DOT)
            {
                continue;
            }
            let found = self
                .sema
                .descend_into_macros(token.clone())
                .into_iter()
                .find_map(|t| {
                    let path = t.parent_ancestors().find_map(ast::Path::cast)?;
                    match self.sema.resolve_path(&path)? {
                        PathResolution::Local(local) => Some(local),
                        _ => None,
                    }
                });
            let Some(local) = found else {
                continue;
            };
            let binding = local.sources(self.db).iter().find_map(|source| {
                let range = source.syntax().text_range();
                self.bindings
                    .get(&(range.start().into(), range.end().into()))
                    .copied()
            });
            let Some(binding) = binding else {
                continue;
            };
            let text = token.text().to_string();
            let read = self.describe(token.text_range(), FlowNodeKind::Reference, Some(&text));
            self.edge(binding, read, FlowEdgeKind::Read);
            self.edge(read, node, edge);
            seen_any = true;
        }
        if !seen_any && !tokens.syntax().text().is_empty() {
            self.skip("macro without locals");
        }
        node
    }
}
