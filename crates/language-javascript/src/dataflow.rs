//! Per-file value-flow facts.
//!
//! One pass over a module's OXC AST and symbol table produces an owned
//! [`FileFlow`]: a graph of value-flow edges between bindings and expressions,
//! the functions and calls in the module, and the places that mutate a
//! binding or its value. It borrows nothing from the AST, so it is cached
//! per file next to the symbols and queried without re-parsing.
//!
//! The graph is flow-insensitive: a binding with several assignments has an
//! edge from each of them. Queries therefore answer "may", not "must".
//! Nodes are keyed by byte range, which is also how the sandbox finds the
//! matching syntax node.

use crate::error::JsSemanticError;
use language_core::{ByteRange, FlowEdgeKind, FlowNodeKind, MutationConfidence, MutationKind};
use oxc::allocator::Allocator;
use oxc::ast::ast::*;
use oxc::ast::AstKind;
use oxc::parser::Parser;
use oxc::semantic::{AstNode, NodeId, Semantic, SemanticBuilder, SymbolFlags, SymbolId};
use oxc::span::{GetSpan, SourceType, Span};
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Array, Map and Set methods that change their receiver.
const MUTATING_METHODS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
    "set",
    "add",
    "delete",
    "clear",
    "append",
];

/// Builtin methods that leave their receiver alone.
const PURE_METHODS: &[&str] = &[
    "map",
    "filter",
    "slice",
    "concat",
    "includes",
    "indexOf",
    "lastIndexOf",
    "join",
    "toString",
    "get",
    "has",
    "keys",
    "values",
    "entries",
    "find",
    "findIndex",
    "findLast",
    "some",
    "every",
    "reduce",
    "reduceRight",
    "at",
    "flat",
    "flatMap",
    "startsWith",
    "endsWith",
    "trim",
    "split",
    "toSorted",
    "toReversed",
    "toSpliced",
    "with",
    "hasOwnProperty",
];

/// A node of the value-flow graph.
#[derive(Debug, Clone)]
pub struct FlowNode {
    pub range: ByteRange,
    pub kind: FlowNodeKind,
    pub name: Option<String>,
    /// Parameter position.
    pub index: Option<u32>,
    /// Innermost enclosing function (index into [`FileFlow::functions`]).
    pub function: Option<u32>,
    /// True for a binding (as opposed to an expression).
    pub binding: bool,
}

/// A function, arrow function or method in the module.
#[derive(Debug, Clone)]
pub struct FunctionFact {
    pub range: ByteRange,
    /// The function's node in the value-flow graph.
    pub node: u32,
    pub name: Option<String>,
    /// Where the name is written: the declaration's identifier, the variable
    /// or property it is assigned to, or the method key.
    pub name_range: Option<ByteRange>,
    /// Enclosing function.
    pub parent: Option<u32>,
    /// The class node, for methods.
    pub class_node: Option<u32>,
}

/// What a call calls, as far as one module can tell.
#[derive(Debug, Clone)]
pub enum CalleeFact {
    /// The callee expression's node.
    Node(u32),
    /// `object.property(…)`.
    Member { object: u32, property: String },
}

/// A call, `new` or JSX element.
#[derive(Debug, Clone)]
pub struct CallFact {
    pub range: ByteRange,
    /// Function the call is made from. `None` for module top-level code.
    pub function: Option<u32>,
    pub callee: CalleeFact,
    pub callee_text: String,
    pub args: Vec<u32>,
    pub is_new: bool,
    pub is_jsx: bool,
}

/// A place that changes a binding or the value it holds.
#[derive(Debug, Clone)]
pub struct MutationFact {
    /// The binding's node.
    pub binding: u32,
    pub range: ByteRange,
    pub kind: MutationKind,
    pub confidence: MutationConfidence,
    pub name: Option<String>,
    pub function: Option<u32>,
}

/// An import binding.
#[derive(Debug, Clone)]
pub struct ImportFact {
    pub module: String,
    /// The name asked of the module (`default` for a default import).
    pub name: String,
    pub namespace: bool,
}

/// Value-flow facts for one module.
#[derive(Debug, Default)]
pub struct FileFlow {
    pub nodes: Vec<FlowNode>,
    /// `out_edges[n]`: nodes the value of `n` flows to.
    pub out_edges: Vec<Vec<(u32, FlowEdgeKind)>>,
    /// `in_edges[n]`: nodes whose value flows into `n`.
    pub in_edges: Vec<Vec<(u32, FlowEdgeKind)>>,
    by_span: FxHashMap<(u32, u32), u32>,
    pub functions: Vec<FunctionFact>,
    pub fn_by_node: FxHashMap<u32, u32>,
    pub calls: Vec<CallFact>,
    pub call_by_node: FxHashMap<u32, u32>,
    pub mutations: Vec<MutationFact>,
    pub imports: FxHashMap<u32, ImportFact>,
    /// Object literal node -> (property name, value node).
    pub object_members: FxHashMap<u32, Vec<(String, u32)>>,
    /// Class node -> (method name, function).
    pub class_methods: FxHashMap<u32, Vec<(String, u32)>>,
    /// Class node -> (field name, the field's node). The initializer flows
    /// into the field's node with a `store` edge.
    pub class_fields: FxHashMap<u32, Vec<(String, u32)>>,
    /// `require("m")` call node -> "m".
    pub requires: FxHashMap<u32, String>,
    /// CommonJS exports: `module.exports = v` (None) and `exports.f = v`,
    /// `module.exports.f = v` (Some("f")), with the value's node.
    pub commonjs_exports: Vec<(Option<String>, u32)>,
    /// The key of a `property` edge into a destructured binding:
    /// `const { f: g } = o` gives (o, g) -> "f".
    pub property_keys: FxHashMap<(u32, u32), String>,
    pub loops: Vec<ByteRange>,
    /// Syntax the builder saw but does not model, by what it is. Values that
    /// pass through it are not connected to where they go. Used to measure
    /// coverage; queries do not read it.
    pub skipped: HashMap<String, u32>,
}

impl FileFlow {
    /// The node with exactly this range.
    pub fn exact(&self, range: ByteRange) -> Option<u32> {
        self.by_span.get(&(range.start, range.end)).copied()
    }

    /// The node for a syntax range: the exact one, else the tightest node
    /// around it, else the widest node inside it.
    pub fn locate(&self, range: ByteRange) -> Option<u32> {
        if let Some(found) = self.exact(range) {
            return Some(found);
        }
        let around = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| range.is_within(&n.range))
            .min_by_key(|(_, n)| n.range.len());
        if let Some((index, _)) = around {
            return Some(index as u32);
        }
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.range.is_within(&range))
            .max_by_key(|(_, n)| n.range.len())
            .map(|(index, _)| index as u32)
    }

    /// The innermost function around a range.
    pub fn function_at(&self, range: ByteRange) -> Option<u32> {
        self.functions
            .iter()
            .enumerate()
            .filter(|(_, f)| range.is_within(&f.range))
            .min_by_key(|(_, f)| f.range.len())
            .map(|(index, _)| index as u32)
    }

    /// The class a function is a method of, looking through nested functions
    /// (an arrow function inside a method still sees the method's `this`).
    pub fn class_of(&self, function: Option<u32>) -> Option<u32> {
        let mut current = function;
        while let Some(index) = current {
            let fact = &self.functions[index as usize];
            if fact.class_node.is_some() {
                return fact.class_node;
            }
            current = fact.parent;
        }
        None
    }
}

/// Facts per file, rebuilt when the file's content changes.
#[derive(Debug, Default)]
pub struct FlowCache {
    files: RwLock<HashMap<PathBuf, (u64, Arc<FileFlow>)>>,
}

impl FlowCache {
    /// Facts for `path` at `content_hash`, building them from `content` when
    /// they are missing or stale. `None` when the file does not parse.
    pub fn get_or_build(
        &self,
        path: &Path,
        content_hash: u64,
        content: impl FnOnce() -> Option<Arc<str>>,
    ) -> Option<Arc<FileFlow>> {
        if let Some((hash, flow)) = self.files.read().get(path) {
            if *hash == content_hash {
                return Some(Arc::clone(flow));
            }
        }
        let flow = Arc::new(build_file_flow(path, &content()?).ok()?);
        let mut files = self.files.write();
        // Another thread may have built them meanwhile: keep one copy, so
        // every caller holds the same facts.
        if let Some((hash, built)) = files.get(path) {
            if *hash == content_hash {
                return Some(Arc::clone(built));
            }
        }
        files.insert(path.to_path_buf(), (content_hash, Arc::clone(&flow)));
        Some(flow)
    }

    /// Whether facts for `path` at `content_hash` are cached.
    pub fn has(&self, path: &Path, content_hash: u64) -> bool {
        self.files
            .read()
            .get(path)
            .is_some_and(|(hash, _)| *hash == content_hash)
    }

    /// Keep facts built elsewhere (in the symbol pass's parse).
    pub fn insert(&self, path: PathBuf, content_hash: u64, flow: FileFlow) {
        self.files
            .write()
            .insert(path, (content_hash, Arc::new(flow)));
    }

    pub fn clear(&self) {
        self.files.write().clear();
    }
}

/// Parse a module and collect its value-flow facts.
pub fn build_file_flow(file_path: &Path, content: &str) -> Result<FileFlow, JsSemanticError> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file_path).unwrap_or_default();
    let parsed = Parser::new(&allocator, content, source_type).parse();
    if !parsed.errors.is_empty() {
        let messages: Vec<String> = parsed.errors.iter().map(|e| e.to_string()).collect();
        return Err(JsSemanticError::ParseError {
            path: file_path.to_path_buf(),
            message: messages.join("; "),
        });
    }
    let program = parsed.program;
    let semantic = SemanticBuilder::new().build(&program).semantic;
    Ok(facts_from_semantic(&semantic, content))
}

/// Value-flow facts from a module's semantic model. The symbol pass and the
/// facts share one parse through this.
pub(crate) fn facts_from_semantic(semantic: &Semantic<'_>, content: &str) -> FileFlow {
    let node_count = semantic.nodes().len();
    let mut builder = Builder {
        semantic,
        source: content,
        flow: FileFlow::default(),
        fn_by_ast: vec![NONE; node_count],
        fn_ast: Vec::new(),
        enclosing: Vec::new(),
        binding_by_symbol: vec![NONE; semantic.scoping().symbols_len()],
    };
    builder.collect_functions();
    builder.collect_bindings();
    builder.collect_expressions();
    builder.flow
}

/// No entry, in the builder's id-indexed tables.
const NONE: u32 = u32::MAX;

fn some(value: u32) -> Option<u32> {
    (value != NONE).then_some(value)
}

struct Builder<'s, 'a> {
    semantic: &'s Semantic<'a>,
    source: &'s str,
    flow: FileFlow,
    /// Function index by syntax node id.
    fn_by_ast: Vec<u32>,
    /// Syntax node of each function, in function order.
    fn_ast: Vec<NodeId>,
    /// Innermost enclosing function by syntax node id, not counting the node.
    enclosing: Vec<u32>,
    binding_by_symbol: Vec<u32>,
}

/// A class member's name: `m`, or `#m` for a private member.
fn class_key(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::PrivateIdentifier(private) => Some(format!("#{}", private.name)),
        other => other.static_name().map(|name| name.to_string()),
    }
}

fn range_of(span: Span) -> ByteRange {
    ByteRange::new(span.start, span.end)
}

impl<'s, 'a> Builder<'s, 'a> {
    fn node(&mut self, span: Span) -> u32 {
        if let Some(found) = self.flow.by_span.get(&(span.start, span.end)) {
            return *found;
        }
        let index = self.flow.nodes.len() as u32;
        self.flow.nodes.push(FlowNode {
            range: range_of(span),
            kind: FlowNodeKind::Expression,
            name: None,
            index: None,
            function: None,
            binding: false,
        });
        self.flow.out_edges.push(Vec::new());
        self.flow.in_edges.push(Vec::new());
        self.flow.by_span.insert((span.start, span.end), index);
        index
    }

    /// Describe a node. Two syntax nodes can share a range (`{ x }`, `a?.b`);
    /// the more specific description wins.
    fn describe(
        &mut self,
        span: Span,
        kind: FlowNodeKind,
        name: Option<String>,
        function: Option<u32>,
    ) -> u32 {
        let index = self.node(span);
        let node = &mut self.flow.nodes[index as usize];
        if node.kind == FlowNodeKind::Expression {
            node.kind = kind;
        }
        if node.name.is_none() {
            node.name = name;
        }
        if node.function.is_none() {
            node.function = function;
        }
        index
    }

    fn skip(&mut self, what: impl Into<String>) {
        *self.flow.skipped.entry(what.into()).or_default() += 1;
    }

    /// True when `expr` is the free name `name` (no local binding).
    fn is_global(&self, expr: &Expression<'a>, name: &str) -> bool {
        matches!(expr.get_inner_expression(), Expression::Identifier(ident)
            if ident.name == name && self.binding_of(ident).is_none())
    }

    fn edge(&mut self, from: u32, to: u32, kind: FlowEdgeKind) {
        if from == to || self.flow.out_edges[from as usize].contains(&(to, kind)) {
            return;
        }
        self.flow.out_edges[from as usize].push((to, kind));
        self.flow.in_edges[to as usize].push((from, kind));
    }

    fn text(&self, span: Span) -> String {
        let text = self
            .source
            .get(span.start as usize..span.end as usize)
            .unwrap_or("");
        let mut short: String = text.chars().take(80).collect();
        if short.len() < text.len() {
            short.push('…');
        }
        short
    }

    /// Innermost function around a syntax node, not counting the node itself.
    fn enclosing_fn(&self, id: NodeId) -> Option<u32> {
        some(self.enclosing[id.index()])
    }

    /// Fill [`Self::enclosing`] in one pass: a node's enclosing function is
    /// its parent when the parent is a function, else the parent's.
    fn compute_enclosing(&mut self) {
        let nodes = self.semantic.nodes();
        let mut enclosing = vec![NONE; nodes.len()];
        for node in nodes.iter() {
            let id = node.id();
            let parent = nodes.parent_node(id).id();
            if parent == id {
                continue;
            }
            enclosing[id.index()] = if parent.index() < id.index() {
                match self.fn_by_ast[parent.index()] {
                    NONE => enclosing[parent.index()],
                    function => function,
                }
            } else {
                // Parents come before children in OXC's node order; if not,
                // walk up.
                nodes
                    .ancestors(id)
                    .filter(|ancestor| ancestor.id() != id)
                    .find_map(|ancestor| some(self.fn_by_ast[ancestor.id().index()]))
                    .unwrap_or(NONE)
            };
        }
        self.enclosing = enclosing;
    }

    fn binding_for_symbol(&self, symbol: SymbolId) -> Option<u32> {
        self.binding_by_symbol
            .get(symbol.index())
            .copied()
            .and_then(some)
    }

    fn binding_of(&self, ident: &IdentifierReference<'a>) -> Option<u32> {
        let reference = self.semantic.scoping().get_reference(ident.reference_id());
        reference
            .symbol_id()
            .and_then(|symbol| self.binding_for_symbol(symbol))
    }

    /// The binding at the root of `x`, `x.a.b`, `x[i]`, `(x)`.
    fn root_binding(&self, expr: &Expression<'a>) -> Option<u32> {
        match expr {
            Expression::Identifier(ident) => self.binding_of(ident),
            Expression::StaticMemberExpression(member) => self.root_binding(&member.object),
            Expression::ComputedMemberExpression(member) => self.root_binding(&member.object),
            Expression::PrivateFieldExpression(member) => self.root_binding(&member.object),
            Expression::ParenthesizedExpression(inner) => self.root_binding(&inner.expression),
            Expression::TSNonNullExpression(inner) => self.root_binding(&inner.expression),
            Expression::TSAsExpression(inner) => self.root_binding(&inner.expression),
            _ => None,
        }
    }

    fn mutation(
        &mut self,
        binding: Option<u32>,
        span: Span,
        kind: MutationKind,
        confidence: MutationConfidence,
        name: Option<String>,
        at: NodeId,
    ) {
        let Some(binding) = binding else {
            return;
        };
        let function = self.enclosing_fn(at);
        self.flow.mutations.push(MutationFact {
            binding,
            range: range_of(span),
            kind,
            confidence,
            name,
            function,
        });
    }

    fn collect_functions(&mut self) {
        let nodes = self.semantic.nodes();
        for node in nodes.iter() {
            let (span, own_name) = match node.kind() {
                AstKind::Function(function) => (
                    function.span,
                    function
                        .id
                        .as_ref()
                        .map(|id| (id.name.to_string(), range_of(id.span))),
                ),
                AstKind::ArrowFunctionExpression(arrow) => (arrow.span, None),
                _ => continue,
            };
            // An anonymous function takes the name of what it is assigned to.
            let named = own_name.or_else(|| match nodes.parent_kind(node.id()) {
                AstKind::VariableDeclarator(declarator) => declarator
                    .id
                    .get_binding_identifier()
                    .map(|id| (id.name.to_string(), range_of(id.span))),
                AstKind::ObjectProperty(property) => property
                    .key
                    .static_name()
                    .map(|name| (name.to_string(), range_of(property.key.span()))),
                AstKind::MethodDefinition(method) => {
                    class_key(&method.key).map(|name| (name, range_of(method.key.span())))
                }
                AstKind::PropertyDefinition(property) => {
                    class_key(&property.key).map(|name| (name, range_of(property.key.span())))
                }
                AstKind::AssignmentExpression(assignment) => assignment
                    .left
                    .get_identifier_name()
                    .map(|name| (name.to_string(), range_of(assignment.left.span()))),
                _ => None,
            });
            let index = self.flow.functions.len() as u32;
            let flow_node = self.node(span);
            self.flow.functions.push(FunctionFact {
                range: range_of(span),
                node: flow_node,
                name: named.as_ref().map(|(name, _)| name.clone()),
                name_range: named.map(|(_, range)| range),
                parent: None,
                class_node: None,
            });
            self.flow.fn_by_node.insert(flow_node, index);
            self.fn_by_ast[node.id().index()] = index;
            self.fn_ast.push(node.id());
        }
        self.compute_enclosing();

        // Parents and classes need every function to be known first.
        for (index, id) in self.fn_ast.clone().into_iter().enumerate() {
            let index = index as u32;
            let node = nodes.get_node(id);
            let parent = self.enclosing_fn(node.id());
            self.flow.functions[index as usize].parent = parent;
            let flow_node = self.flow.functions[index as usize].node;
            self.flow.nodes[flow_node as usize].kind = FlowNodeKind::Function;
            self.flow.nodes[flow_node as usize].name =
                self.flow.functions[index as usize].name.clone();
            self.flow.nodes[flow_node as usize].function = parent;

            let member_name = match nodes.parent_kind(node.id()) {
                AstKind::MethodDefinition(method) => Some(class_key(&method.key)),
                // `handle = () => {}`: an arrow-function field is a method
                // whose `this` is the instance.
                AstKind::PropertyDefinition(property) => Some(class_key(&property.key)),
                _ => None,
            };
            if let Some(member_name) = member_name {
                let class_span = nodes.ancestors(node.id()).find_map(|a| match a.kind() {
                    AstKind::Class(class) => Some(class.span),
                    _ => None,
                });
                if let (Some(class_span), Some(name)) = (class_span, member_name) {
                    let class_node = self.node(class_span);
                    self.flow.functions[index as usize].class_node = Some(class_node);
                    self.flow
                        .class_methods
                        .entry(class_node)
                        .or_default()
                        .push((name.to_string(), index));
                }
            }
        }
    }

    fn collect_bindings(&mut self) {
        let nodes = self.semantic.nodes();
        let scoping = self.semantic.scoping();
        for symbol in scoping.symbol_ids() {
            let flags = scoping.symbol_flags(symbol);
            if flags.intersects(
                SymbolFlags::TypeAlias | SymbolFlags::Interface | SymbolFlags::TypeParameter,
            ) {
                continue;
            }
            let declaration = scoping.symbol_declaration(symbol);
            let mut kind = FlowNodeKind::Variable;
            let mut index = None;
            let mut import = None;
            let mut value_span = None;
            match nodes.kind(declaration) {
                AstKind::FormalParameter(parameter) => {
                    kind = FlowNodeKind::Parameter;
                    if let AstKind::FormalParameters(parameters) = nodes.parent_kind(declaration) {
                        index = parameters
                            .items
                            .iter()
                            .position(|item| item.span == parameter.span)
                            .map(|position| position as u32);
                    }
                }
                AstKind::BindingRestElement(_) => {
                    if let AstKind::FormalParameters(parameters) = nodes.parent_kind(declaration) {
                        kind = FlowNodeKind::Parameter;
                        index = Some(parameters.items.len() as u32);
                    }
                }
                AstKind::ImportSpecifier(specifier) => {
                    kind = FlowNodeKind::Import;
                    import = Some((specifier.imported.name().to_string(), false));
                }
                AstKind::ImportDefaultSpecifier(_) => {
                    kind = FlowNodeKind::Import;
                    import = Some(("default".to_string(), false));
                }
                AstKind::ImportNamespaceSpecifier(_) => {
                    kind = FlowNodeKind::Import;
                    import = Some(("*".to_string(), true));
                }
                AstKind::Function(function) => value_span = Some(function.span),
                AstKind::Class(class) => value_span = Some(class.span),
                _ => {}
            }

            let function = self.enclosing_fn(declaration);
            let name = scoping.symbol_name(symbol).to_string();
            let binding = self.describe(scoping.symbol_span(symbol), kind, Some(name), function);
            self.flow.nodes[binding as usize].kind = kind;
            self.flow.nodes[binding as usize].index = index;
            self.flow.nodes[binding as usize].binding = true;
            self.binding_by_symbol[symbol.index()] = binding;

            if let Some((name, namespace)) = import {
                if let AstKind::ImportDeclaration(import_declaration) =
                    nodes.parent_kind(declaration)
                {
                    self.flow.imports.insert(
                        binding,
                        ImportFact {
                            module: import_declaration.source.value.to_string(),
                            name,
                            namespace,
                        },
                    );
                }
            }
            if let Some(span) = value_span {
                let value = self.node(span);
                self.edge(value, binding, FlowEdgeKind::Assign);
            }
        }
    }

    /// Edges from a value into the bindings of a pattern.
    fn bind_pattern(&mut self, pattern: &BindingPattern<'a>, from: u32, kind: FlowEdgeKind) {
        match &pattern.kind {
            BindingPatternKind::BindingIdentifier(ident) => {
                if let Some(binding) = self.binding_for_symbol(ident.symbol_id()) {
                    self.edge(from, binding, kind);
                }
            }
            BindingPatternKind::ObjectPattern(object) => {
                for property in &object.properties {
                    self.bind_pattern(&property.value, from, FlowEdgeKind::Property);
                    // `{ f: g }`, `{ f = d }`: remember that g is from.f.
                    let leaf = match &property.value.kind {
                        BindingPatternKind::AssignmentPattern(assignment) => &assignment.left.kind,
                        other => other,
                    };
                    if let (BindingPatternKind::BindingIdentifier(ident), Some(key)) =
                        (leaf, property.key.static_name())
                    {
                        if !property.computed {
                            if let Some(binding) = self.binding_for_symbol(ident.symbol_id()) {
                                self.flow
                                    .property_keys
                                    .insert((from, binding), key.to_string());
                            }
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.bind_pattern(&rest.argument, from, FlowEdgeKind::Capture);
                }
            }
            BindingPatternKind::ArrayPattern(array) => {
                for element in array.elements.iter().flatten() {
                    self.bind_pattern(element, from, FlowEdgeKind::Element);
                }
                if let Some(rest) = &array.rest {
                    self.bind_pattern(&rest.argument, from, FlowEdgeKind::Capture);
                }
            }
            // The default value is linked when it is visited.
            BindingPatternKind::AssignmentPattern(assignment) => {
                self.bind_pattern(&assignment.left, from, kind);
            }
        }
    }

    /// Edges from a value into the targets of an assignment pattern:
    /// `[a, b] = v`, `({ p, q: r = 1, ...rest } = v)`, `[o.p] = v`.
    fn bind_target(&mut self, target: &AssignmentTarget<'a>, from: u32, kind: FlowEdgeKind) {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                if let Some(binding) = self.binding_of(ident) {
                    self.edge(from, binding, kind);
                }
            }
            AssignmentTarget::StaticMemberExpression(_)
            | AssignmentTarget::ComputedMemberExpression(_)
            | AssignmentTarget::PrivateFieldExpression(_) => {
                let node = self.node(target.span());
                self.edge(from, node, FlowEdgeKind::Store);
            }
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                for element in array.elements.iter().flatten() {
                    self.bind_target_default(element, from, FlowEdgeKind::Element);
                }
                if let Some(rest) = &array.rest {
                    self.bind_target(&rest.target, from, FlowEdgeKind::Capture);
                }
            }
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                for property in &object.properties {
                    match property {
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(short) => {
                            if let Some(binding) = self.binding_of(&short.binding) {
                                self.edge(from, binding, FlowEdgeKind::Property);
                                self.flow
                                    .property_keys
                                    .insert((from, binding), short.binding.name.to_string());
                                if let Some(init) = &short.init {
                                    let value = self.node(init.span());
                                    self.edge(value, binding, FlowEdgeKind::Assign);
                                }
                            }
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(long) => {
                            self.bind_target_default(&long.binding, from, FlowEdgeKind::Property);
                            let leaf = match &long.binding {
                                AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                                    Some(&d.binding)
                                }
                                other => other.as_assignment_target(),
                            };
                            if let (
                                Some(AssignmentTarget::AssignmentTargetIdentifier(ident)),
                                Some(key),
                                false,
                            ) = (leaf, long.name.static_name(), long.computed)
                            {
                                if let Some(binding) = self.binding_of(ident) {
                                    self.flow
                                        .property_keys
                                        .insert((from, binding), key.to_string());
                                }
                            }
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.bind_target(&rest.target, from, FlowEdgeKind::Capture);
                }
            }
            // `(x as T) = v` and other TypeScript wrappers.
            _ => self.skip("assignment-target"),
        }
    }

    fn bind_target_default(
        &mut self,
        target: &AssignmentTargetMaybeDefault<'a>,
        from: u32,
        kind: FlowEdgeKind,
    ) {
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
                self.bind_target(&with_default.binding, from, kind);
                let value = self.node(with_default.init.span());
                self.bind_target(&with_default.binding, value, FlowEdgeKind::Assign);
            }
            other => {
                if let Some(target) = other.as_assignment_target() {
                    self.bind_target(target, from, kind);
                }
            }
        }
    }

    /// Mutations made by an assignment pattern: each identifier is rebound,
    /// each member target is a property write.
    fn target_mutations(&mut self, target: &AssignmentTarget<'a>, span: Span, at: NodeId) {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                let binding = self.binding_of(ident);
                self.mutation(
                    binding,
                    span,
                    MutationKind::Reassign,
                    MutationConfidence::Definite,
                    None,
                    at,
                );
            }
            AssignmentTarget::StaticMemberExpression(member) => {
                let binding = self.root_binding(&member.object);
                let name = Some(member.property.name.to_string());
                self.mutation(
                    binding,
                    span,
                    MutationKind::PropertyWrite,
                    MutationConfidence::Definite,
                    name,
                    at,
                );
            }
            AssignmentTarget::ComputedMemberExpression(member) => {
                let binding = self.root_binding(&member.object);
                let name = member.static_property_name().map(|name| name.to_string());
                self.mutation(
                    binding,
                    span,
                    MutationKind::PropertyWrite,
                    MutationConfidence::Definite,
                    name,
                    at,
                );
            }
            AssignmentTarget::PrivateFieldExpression(member) => {
                let binding = self.root_binding(&member.object);
                let name = Some(member.field.name.to_string());
                self.mutation(
                    binding,
                    span,
                    MutationKind::PropertyWrite,
                    MutationConfidence::Definite,
                    name,
                    at,
                );
            }
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                for element in array.elements.iter().flatten() {
                    let inner = match element {
                        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                            Some(&d.binding)
                        }
                        other => other.as_assignment_target(),
                    };
                    if let Some(inner) = inner {
                        self.target_mutations(inner, span, at);
                    }
                }
                if let Some(rest) = &array.rest {
                    self.target_mutations(&rest.target, span, at);
                }
            }
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                for property in &object.properties {
                    match property {
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(short) => {
                            let binding = self.binding_of(&short.binding);
                            self.mutation(
                                binding,
                                span,
                                MutationKind::Reassign,
                                MutationConfidence::Definite,
                                None,
                                at,
                            );
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(long) => {
                            let inner = match &long.binding {
                                AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                                    Some(&d.binding)
                                }
                                other => other.as_assignment_target(),
                            };
                            if let Some(inner) = inner {
                                self.target_mutations(inner, span, at);
                            }
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.target_mutations(&rest.target, span, at);
                }
            }
            _ => {}
        }
    }

    fn collect_expressions(&mut self) {
        let nodes = self.semantic.nodes();
        for node in nodes.iter() {
            let kind = node.kind();
            match kind {
                AstKind::ForStatement(_)
                | AstKind::ForInStatement(_)
                | AstKind::ForOfStatement(_)
                | AstKind::WhileStatement(_)
                | AstKind::DoWhileStatement(_) => {
                    self.flow.loops.push(range_of(kind.span()));
                    // `for (x of xs)`, `for ([k, v] of entries)`: the loop rebinds x.
                    let left = match kind {
                        AstKind::ForOfStatement(for_of) => Some(&for_of.left),
                        AstKind::ForInStatement(for_in) => Some(&for_in.left),
                        _ => None,
                    };
                    if let Some(target) = left.and_then(|left| left.as_assignment_target()) {
                        self.target_mutations(target, target.span(), node.id());
                    }
                    continue;
                }
                _ => {}
            }
            let Some((flow_kind, name)) = self.classify(node) else {
                continue;
            };
            let function = self.enclosing_fn(node.id());
            let index = self.describe(kind.span(), flow_kind, name, function);
            self.link_to_parent(node, index);
        }
    }

    /// What a syntax node is in the value-flow graph, recording its calls
    /// and mutations on the way. `None` for nodes that are not values.
    fn classify(&mut self, node: &AstNode<'a>) -> Option<(FlowNodeKind, Option<String>)> {
        let id = node.id();
        Some(match node.kind() {
            AstKind::IdentifierReference(ident) => {
                let reference = self.semantic.scoping().get_reference(ident.reference_id());
                if reference.is_type() && !reference.is_value() {
                    return None;
                }
                let index = self.node(ident.span);
                match self.binding_of(ident) {
                    Some(binding) => {
                        if reference.is_read() {
                            self.edge(binding, index, FlowEdgeKind::Read);
                        }
                        (FlowNodeKind::Reference, Some(ident.name.to_string()))
                    }
                    None => (FlowNodeKind::Global, Some(ident.name.to_string())),
                }
            }
            AstKind::BooleanLiteral(_)
            | AstKind::NullLiteral(_)
            | AstKind::NumericLiteral(_)
            | AstKind::StringLiteral(_)
            | AstKind::BigIntLiteral(_)
            | AstKind::RegExpLiteral(_) => (FlowNodeKind::Literal, None),
            AstKind::TemplateLiteral(template) if template.expressions.is_empty() => {
                (FlowNodeKind::Literal, None)
            }
            AstKind::ThisExpression(_) => (FlowNodeKind::This, None),
            AstKind::ArrayExpression(_) => (FlowNodeKind::Array, None),
            AstKind::ObjectExpression(_) => (FlowNodeKind::Object, None),
            AstKind::ObjectProperty(property) => (
                FlowNodeKind::Expression,
                property.key.static_name().map(|name| name.to_string()),
            ),
            AstKind::StaticMemberExpression(member) => {
                (FlowNodeKind::Member, Some(member.property.name.to_string()))
            }
            AstKind::ComputedMemberExpression(member) => (
                FlowNodeKind::Member,
                member.static_property_name().map(|name| name.to_string()),
            ),
            AstKind::PrivateFieldExpression(member) => {
                (FlowNodeKind::Member, Some(member.field.name.to_string()))
            }
            AstKind::CallExpression(call) => {
                let callee = self.record_call(id, call.span, &call.callee, &call.arguments, false);
                (FlowNodeKind::Call, Some(callee))
            }
            AstKind::NewExpression(call) => {
                let callee = self.record_call(id, call.span, &call.callee, &call.arguments, true);
                (FlowNodeKind::Call, Some(callee))
            }
            AstKind::Function(function) => (
                FlowNodeKind::Function,
                function.id.as_ref().map(|id| id.name.to_string()),
            ),
            AstKind::ArrowFunctionExpression(_) => (FlowNodeKind::Function, None),
            AstKind::Class(class) => (
                FlowNodeKind::Class,
                class.id.as_ref().map(|id| id.name.to_string()),
            ),
            AstKind::JSXElement(element) => {
                self.record_jsx(id, element);
                (FlowNodeKind::Jsx, None)
            }
            AstKind::JSXFragment(_) => (FlowNodeKind::Jsx, None),
            AstKind::JSXAttribute(attribute) => {
                let name = match &attribute.name {
                    JSXAttributeName::Identifier(ident) => ident.name.to_string(),
                    JSXAttributeName::NamespacedName(name) => self.text(name.span),
                };
                (FlowNodeKind::JsxAttribute, Some(name))
            }
            AstKind::AssignmentExpression(assignment) => {
                self.record_assignment(id, assignment);
                (FlowNodeKind::Expression, None)
            }
            AstKind::UpdateExpression(update) => {
                let (binding, name) = match &update.argument {
                    SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                        (self.binding_of(ident), None)
                    }
                    SimpleAssignmentTarget::StaticMemberExpression(member) => (
                        self.root_binding(&member.object),
                        Some(member.property.name.to_string()),
                    ),
                    SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                        (self.root_binding(&member.object), Some("[]".to_string()))
                    }
                    _ => (None, None),
                };
                self.mutation(
                    binding,
                    update.span,
                    MutationKind::Update,
                    MutationConfidence::Definite,
                    name,
                    id,
                );
                (FlowNodeKind::Expression, None)
            }
            AstKind::UnaryExpression(unary) => {
                if unary.operator == UnaryOperator::Delete {
                    let (binding, name) = match unary.argument.get_inner_expression() {
                        Expression::StaticMemberExpression(member) => (
                            self.root_binding(&member.object),
                            Some(member.property.name.to_string()),
                        ),
                        Expression::ComputedMemberExpression(member) => {
                            (self.root_binding(&member.object), None)
                        }
                        _ => (None, None),
                    };
                    self.mutation(
                        binding,
                        unary.span,
                        MutationKind::Delete,
                        MutationConfidence::Definite,
                        name,
                        id,
                    );
                }
                (FlowNodeKind::Expression, None)
            }
            AstKind::SpreadElement(_)
            | AstKind::TaggedTemplateExpression(_)
            | AstKind::TemplateLiteral(_)
            | AstKind::BinaryExpression(_)
            | AstKind::LogicalExpression(_)
            | AstKind::ConditionalExpression(_)
            | AstKind::SequenceExpression(_)
            | AstKind::AwaitExpression(_)
            | AstKind::YieldExpression(_)
            | AstKind::ChainExpression(_)
            | AstKind::ParenthesizedExpression(_)
            | AstKind::TSAsExpression(_)
            | AstKind::TSSatisfiesExpression(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::TSTypeAssertion(_)
            | AstKind::ImportExpression(_)
            | AstKind::JSXExpressionContainer(_)
            | AstKind::JSXSpreadAttribute(_) => (FlowNodeKind::Expression, None),
            _ => return None,
        })
    }

    fn record_call(
        &mut self,
        id: NodeId,
        span: Span,
        callee: &Expression<'a>,
        arguments: &[Argument<'a>],
        is_new: bool,
    ) -> String {
        let call_node = self.node(span);
        if self.is_global(callee, "require") && !is_new {
            let specifier = match arguments.first() {
                Some(Argument::StringLiteral(literal)) => Some(literal.value.to_string()),
                Some(Argument::TemplateLiteral(template)) if template.expressions.is_empty() => {
                    template.quasis.first().map(|q| q.value.raw.to_string())
                }
                _ => None,
            };
            match specifier {
                Some(specifier) => {
                    self.flow.requires.insert(call_node, specifier);
                }
                None => self.skip("commonjs-require-dynamic"),
            }
        }
        if matches!(callee.get_inner_expression(), Expression::Super(_)) {
            self.skip("super-call");
        }
        let callee_fact = match callee.get_inner_expression() {
            Expression::StaticMemberExpression(member) if !is_new => {
                let object = self.node(member.object.span());
                self.edge(object, call_node, FlowEdgeKind::Receiver);

                let name = member.property.name.as_str();
                if !PURE_METHODS.contains(&name) {
                    let confidence = if MUTATING_METHODS.contains(&name) {
                        MutationConfidence::Likely
                    } else {
                        MutationConfidence::Maybe
                    };
                    let binding = self.root_binding(&member.object);
                    self.mutation(
                        binding,
                        span,
                        MutationKind::MethodCall,
                        confidence,
                        Some(name.to_string()),
                        id,
                    );
                }
                CalleeFact::Member {
                    object,
                    property: name.to_string(),
                }
            }
            // `this.#method()`.
            Expression::PrivateFieldExpression(member) if !is_new => {
                let object = self.node(member.object.span());
                self.edge(object, call_node, FlowEdgeKind::Receiver);
                CalleeFact::Member {
                    object,
                    property: format!("#{}", member.field.name),
                }
            }
            other => CalleeFact::Node(self.node(other.span())),
        };

        let mut args = Vec::with_capacity(arguments.len());
        for argument in arguments {
            args.push(self.node(argument.span()));
            let expression = match argument {
                Argument::SpreadElement(spread) => Some(&spread.argument),
                other => other.as_expression(),
            };
            // A callee with no summary may change any object it is handed.
            if let Some(Expression::Identifier(ident)) =
                expression.map(|e| e.get_inner_expression())
            {
                let binding = self.binding_of(ident);
                self.mutation(
                    binding,
                    span,
                    MutationKind::Escape,
                    MutationConfidence::Maybe,
                    None,
                    id,
                );
            }
        }

        let function = self.enclosing_fn(id);
        let callee_text = self.text(callee.span());
        self.flow
            .call_by_node
            .insert(call_node, self.flow.calls.len() as u32);
        self.flow.calls.push(CallFact {
            range: range_of(span),
            function,
            callee: callee_fact,
            callee_text: callee_text.clone(),
            args,
            is_new,
            is_jsx: false,
        });
        callee_text
    }

    /// `<Component />` calls `Component`.
    fn record_jsx(&mut self, id: NodeId, element: &JSXElement<'a>) {
        let (callee, callee_text) = match &element.opening_element.name {
            JSXElementName::IdentifierReference(ident) => (
                CalleeFact::Node(self.node(ident.span)),
                ident.name.to_string(),
            ),
            // `<Tabs.Trigger>`: a method-style lookup on the object.
            JSXElementName::MemberExpression(member) => {
                let text = self.text(member.span);
                match &member.object {
                    JSXMemberExpressionObject::IdentifierReference(object) => (
                        CalleeFact::Member {
                            object: self.node(object.span),
                            property: member.property.name.to_string(),
                        },
                        text,
                    ),
                    // `<A.B.C>`: reported, not resolved.
                    _ => (CalleeFact::Node(self.node(member.span)), text),
                }
            }
            // `<div>`, `<svg:path>`, `<this.X>`: not a component call.
            _ => return,
        };
        let call_node = self.node(element.span);
        let function = self.enclosing_fn(id);
        self.flow
            .call_by_node
            .insert(call_node, self.flow.calls.len() as u32);
        self.flow.calls.push(CallFact {
            range: range_of(element.span),
            function,
            callee,
            callee_text,
            args: Vec::new(),
            is_new: false,
            is_jsx: true,
        });
    }

    fn record_assignment(&mut self, id: NodeId, assignment: &AssignmentExpression<'a>) {
        let (binding, kind, name) = match &assignment.left {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                (self.binding_of(ident), MutationKind::Reassign, None)
            }
            AssignmentTarget::StaticMemberExpression(member) => (
                self.root_binding(&member.object),
                MutationKind::PropertyWrite,
                Some(member.property.name.to_string()),
            ),
            AssignmentTarget::ComputedMemberExpression(member) => (
                self.root_binding(&member.object),
                MutationKind::PropertyWrite,
                member.static_property_name().map(|name| name.to_string()),
            ),
            AssignmentTarget::PrivateFieldExpression(member) => (
                self.root_binding(&member.object),
                MutationKind::PropertyWrite,
                Some(member.field.name.to_string()),
            ),
            AssignmentTarget::ArrayAssignmentTarget(_)
            | AssignmentTarget::ObjectAssignmentTarget(_) => {
                self.target_mutations(&assignment.left, assignment.span, id);
                return;
            }
            _ => (None, MutationKind::Reassign, None),
        };
        self.mutation(
            binding,
            assignment.span,
            kind,
            MutationConfidence::Definite,
            name,
            id,
        );
    }

    /// The edge from a value to the expression, binding or sink that takes it.
    fn link_to_parent(&mut self, node: &AstNode<'a>, index: u32) {
        let nodes = self.semantic.nodes();
        let id = node.id();
        let span = node.kind().span();
        let parent = nodes.parent_node(id);
        if parent.id() == id {
            return;
        }
        match parent.kind() {
            AstKind::VariableDeclarator(declarator) => {
                if declarator.init.as_ref().is_some_and(|i| i.span() == span) {
                    self.bind_pattern(&declarator.id, index, FlowEdgeKind::Assign);
                }
            }
            AstKind::AssignmentPattern(pattern) => {
                if pattern.right.span() == span {
                    self.bind_pattern(&pattern.left, index, FlowEdgeKind::Assign);
                }
            }
            AstKind::AssignmentExpression(assignment) => {
                if assignment.right.span() != span {
                    return;
                }
                let whole = self.node(assignment.span);
                self.edge(index, whole, FlowEdgeKind::Alias);
                self.bind_target(&assignment.left, index, FlowEdgeKind::Assign);
                if assignment.operator == AssignmentOperator::Assign {
                    if let Some(export) = self.commonjs_export(&assignment.left) {
                        self.flow.commonjs_exports.push((export, index));
                    }
                }
            }
            AstKind::CallExpression(call) => {
                if call.callee.span() != span {
                    let whole = self.node(call.span);
                    self.edge(index, whole, FlowEdgeKind::Argument);
                }
            }
            AstKind::NewExpression(call) => {
                if call.callee.span() != span {
                    let whole = self.node(call.span);
                    self.edge(index, whole, FlowEdgeKind::Argument);
                }
            }
            AstKind::TaggedTemplateExpression(tagged) => {
                if tagged.quasi.span == span {
                    let whole = self.node(tagged.span);
                    self.edge(index, whole, FlowEdgeKind::Argument);
                }
            }
            AstKind::StaticMemberExpression(member) => {
                if member.object.span() == span {
                    let whole = self.node(member.span);
                    self.edge(index, whole, FlowEdgeKind::Property);
                }
            }
            AstKind::PrivateFieldExpression(member) => {
                if member.object.span() == span {
                    let whole = self.node(member.span);
                    self.edge(index, whole, FlowEdgeKind::Property);
                }
            }
            AstKind::ComputedMemberExpression(member) => {
                if member.object.span() == span {
                    let whole = self.node(member.span);
                    self.edge(index, whole, FlowEdgeKind::Element);
                }
            }
            AstKind::SpreadElement(_)
            | AstKind::ParenthesizedExpression(_)
            | AstKind::ChainExpression(_)
            | AstKind::TSAsExpression(_)
            | AstKind::TSSatisfiesExpression(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::TSTypeAssertion(_)
            | AstKind::LogicalExpression(_)
            | AstKind::JSXExpressionContainer(_)
            | AstKind::JSXSpreadAttribute(_) => {
                let whole = self.node(parent.kind().span());
                self.edge(index, whole, FlowEdgeKind::Alias);
            }
            AstKind::ConditionalExpression(conditional) => {
                if conditional.test.span() != span {
                    let whole = self.node(conditional.span);
                    self.edge(index, whole, FlowEdgeKind::Alias);
                }
            }
            AstKind::SequenceExpression(sequence) => {
                if sequence
                    .expressions
                    .last()
                    .is_some_and(|e| e.span() == span)
                {
                    let whole = self.node(sequence.span);
                    self.edge(index, whole, FlowEdgeKind::Alias);
                }
            }
            AstKind::BinaryExpression(_)
            | AstKind::UnaryExpression(_)
            | AstKind::TemplateLiteral(_)
            | AstKind::AwaitExpression(_)
            | AstKind::YieldExpression(_) => {
                let whole = self.node(parent.kind().span());
                self.edge(index, whole, FlowEdgeKind::Derive);
            }
            AstKind::ArrayExpression(array) => {
                let whole = self.node(array.span);
                self.edge(index, whole, FlowEdgeKind::Capture);
            }
            AstKind::ObjectProperty(property) => {
                if property.value.span() == span {
                    let whole = self.node(property.span);
                    self.edge(index, whole, FlowEdgeKind::Alias);
                }
            }
            AstKind::ObjectExpression(object) => {
                let whole = self.node(object.span);
                self.edge(index, whole, FlowEdgeKind::Capture);
                if let AstKind::ObjectProperty(property) = node.kind() {
                    if let Some(name) = property.key.static_name() {
                        let value = self.node(property.value.span());
                        self.flow
                            .object_members
                            .entry(whole)
                            .or_default()
                            .push((name.to_string(), value));
                    }
                }
            }
            AstKind::ReturnStatement(_) => self.link_return(id, index),
            AstKind::ExpressionStatement(_) => {
                // `(x) => x + 1`: the body expression is the return value.
                let statement = parent.id();
                let body = nodes.parent_node(statement);
                if let (AstKind::FunctionBody(_), AstKind::ArrowFunctionExpression(arrow)) =
                    (body.kind(), nodes.parent_kind(body.id()))
                {
                    if arrow.expression {
                        self.link_return(statement, index);
                    }
                }
            }
            AstKind::JSXAttribute(attribute) => {
                let whole = self.node(attribute.span);
                self.edge(index, whole, FlowEdgeKind::JsxAttribute);
            }
            // `class C { items = []; }`: the value is stored in the field.
            AstKind::PropertyDefinition(property)
                if property.value.as_ref().is_some_and(|v| v.span() == span) =>
            {
                let field = self.class_field(parent.id(), property);
                self.edge(index, field, FlowEdgeKind::Store);
            }
            AstKind::JSXOpeningElement(_) => {
                if matches!(
                    node.kind(),
                    AstKind::JSXAttribute(_) | AstKind::JSXSpreadAttribute(_)
                ) {
                    let element = nodes.parent_kind(parent.id()).span();
                    let whole = self.node(element);
                    self.edge(index, whole, FlowEdgeKind::JsxAttribute);
                }
            }
            AstKind::JSXElement(_) | AstKind::JSXFragment(_) => {
                let whole = self.node(parent.kind().span());
                self.edge(index, whole, FlowEdgeKind::JsxChild);
            }
            AstKind::ForOfStatement(for_of) if for_of.right.span() == span => {
                if let ForStatementLeft::VariableDeclaration(declaration) = &for_of.left {
                    for declarator in &declaration.declarations {
                        self.bind_pattern(&declarator.id, index, FlowEdgeKind::Element);
                    }
                } else if let Some(target) = for_of.left.as_assignment_target() {
                    self.bind_target(target, index, FlowEdgeKind::Element);
                }
            }
            // Positions where a value is used for control, or that are a
            // name rather than a value: nothing flows on.
            AstKind::IfStatement(_)
            | AstKind::WhileStatement(_)
            | AstKind::DoWhileStatement(_)
            | AstKind::ForStatement(_)
            | AstKind::ForInStatement(_)
            | AstKind::ForOfStatement(_)
            | AstKind::SwitchStatement(_)
            | AstKind::SwitchCase(_)
            | AstKind::ThrowStatement(_)
            | AstKind::FormalParameters(_)
            | AstKind::FormalParameter(_)
            | AstKind::Decorator(_)
            | AstKind::ImportExpression(_)
            | AstKind::Program(_)
            | AstKind::FunctionBody(_)
            | AstKind::BlockStatement(_)
            | AstKind::UpdateExpression(_)
            | AstKind::MethodDefinition(_)
            | AstKind::ImportDeclaration(_)
            | AstKind::ImportAttribute(_)
            | AstKind::ExportNamedDeclaration(_)
            | AstKind::ExportAllDeclaration(_)
            | AstKind::ExportSpecifier(_)
            | AstKind::ExportDefaultDeclaration(_)
            | AstKind::Directive(_)
            | AstKind::JSXMemberExpression(_)
            | AstKind::JSXClosingElement(_)
            // Assignment patterns are linked from the assignment.
            | AstKind::ArrayAssignmentTarget(_)
            | AstKind::ObjectAssignmentTarget(_)
            | AstKind::AssignmentTargetWithDefault(_)
            | AstKind::AssignmentTargetPropertyIdentifier(_)
            | AstKind::AssignmentTargetPropertyProperty(_)
            | AstKind::AssignmentTargetRest(_) => {}
            AstKind::Class(class)
                if class.super_class.as_ref().is_some_and(|s| s.span() == span) =>
            {
                self.skip("class-extends")
            }
            other => {
                let what = format!("{:?}", other.ty());
                // Type-level positions carry no values.
                if !what.starts_with("TS") {
                    self.skip(format!("value-in:{what}"));
                }
            }
        }
    }

    /// What an assignment target exports, for CommonJS: `module.exports`
    /// (Some(None)), `exports.f` or `module.exports.f` (Some(Some("f"))).
    fn commonjs_export(&self, target: &AssignmentTarget<'a>) -> Option<Option<String>> {
        let AssignmentTarget::StaticMemberExpression(member) = target else {
            return None;
        };
        let is_module_exports = |expr: &Expression<'a>| {
            matches!(expr.get_inner_expression(), Expression::StaticMemberExpression(inner)
                if inner.property.name == "exports" && self.is_global(&inner.object, "module"))
        };
        if member.property.name == "exports" && self.is_global(&member.object, "module") {
            return Some(None);
        }
        if self.is_global(&member.object, "exports") || is_module_exports(&member.object) {
            return Some(Some(member.property.name.to_string()));
        }
        None
    }

    /// The node for a class field, registered on its class.
    fn class_field(&mut self, id: NodeId, property: &PropertyDefinition<'a>) -> u32 {
        let name = class_key(&property.key);
        let function = self.enclosing_fn(id);
        let field = self.describe(property.span, FlowNodeKind::Member, name.clone(), function);
        let class_span = self
            .semantic
            .nodes()
            .ancestors(id)
            .find_map(|a| match a.kind() {
                AstKind::Class(class) => Some(class.span),
                _ => None,
            });
        if let (Some(class_span), Some(name)) = (class_span, name) {
            let class_node = self.node(class_span);
            let fields = self.flow.class_fields.entry(class_node).or_default();
            if !fields.iter().any(|(_, node)| *node == field) {
                fields.push((name, field));
            }
        }
        field
    }

    fn link_return(&mut self, from: NodeId, index: u32) {
        if let Some(function) = self.enclosing_fn(from) {
            let function_node = self.flow.functions[function as usize].node;
            self.edge(index, function_node, FlowEdgeKind::Return);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(source: &str) -> FileFlow {
        build_file_flow(Path::new("test.jsx"), source).unwrap()
    }

    fn node_at(flow: &FileFlow, source: &str, needle: &str) -> u32 {
        let start = source.find(needle).unwrap() as u32;
        flow.exact(ByteRange::new(start, start + needle.len() as u32))
            .unwrap_or_else(|| panic!("no flow node for {needle}"))
    }

    #[test]
    fn initializer_flows_into_binding_and_reads() {
        let source = "const a = input.value;\nsend(a);";
        let flow = flow(source);
        let member = node_at(&flow, source, "input.value");
        let binding = flow.out_edges[member as usize][0];
        assert_eq!(binding.1, FlowEdgeKind::Assign);
        assert!(flow.nodes[binding.0 as usize].binding);
        let read = flow.out_edges[binding.0 as usize][0];
        assert_eq!(read.1, FlowEdgeKind::Read);
        let call = flow.out_edges[read.0 as usize][0];
        assert_eq!(call.1, FlowEdgeKind::Argument);
        assert_eq!(flow.nodes[call.0 as usize].kind, FlowNodeKind::Call);
    }

    #[test]
    fn parameters_and_returns_are_recorded() {
        let source = "function pick(first, second) { return second; }";
        let flow = flow(source);
        let second = flow
            .nodes
            .iter()
            .find(|n| n.binding && n.name.as_deref() == Some("second"))
            .unwrap();
        assert_eq!(second.kind, FlowNodeKind::Parameter);
        assert_eq!(second.index, Some(1));
        let function = &flow.functions[0];
        assert_eq!(function.name.as_deref(), Some("pick"));
        assert!(flow.in_edges[function.node as usize]
            .iter()
            .any(|(_, kind)| *kind == FlowEdgeKind::Return));
    }

    #[test]
    fn mutations_are_classified() {
        let source = "let list = [];\nlist.push(1);\nlist = [];\nlist.length = 0;\nlist.map(x => x);\nsave(list);";
        let flow = flow(source);
        let kinds: Vec<_> = flow
            .mutations
            .iter()
            .map(|m| (m.kind, m.confidence))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (MutationKind::MethodCall, MutationConfidence::Likely),
                (MutationKind::Reassign, MutationConfidence::Definite),
                (MutationKind::PropertyWrite, MutationConfidence::Definite),
                (MutationKind::Escape, MutationConfidence::Maybe),
            ]
        );
    }

    /// Cost of the facts on a real tree. Run with
    /// `DATAFLOW_BENCH_DIR=/path/to/repo cargo test --release -p language-javascript bench_flow_facts -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_flow_facts() {
        let Ok(dir) = std::env::var("DATAFLOW_BENCH_DIR") else {
            return;
        };
        let mut files = Vec::new();
        for entry in ignore::WalkBuilder::new(&dir).build().flatten() {
            let path = entry.path();
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if path.is_file() && matches!(extension, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs") {
                if let Ok(content) = std::fs::read_to_string(path) {
                    files.push((path.to_path_buf(), content));
                }
            }
        }
        let bytes: usize = files.iter().map(|(_, content)| content.len()).sum();

        let started = std::time::Instant::now();
        let symbols = files
            .iter()
            .filter(|(path, content)| crate::oxc_adapter::parse_and_analyze(path, content).is_ok())
            .count();
        let symbols_time = started.elapsed();

        let started = std::time::Instant::now();
        let (mut parsed, mut nodes, mut edges, mut functions, mut calls) = (0, 0, 0, 0, 0);
        for (path, content) in &files {
            if let Ok(flow) = build_file_flow(path, content) {
                parsed += 1;
                nodes += flow.nodes.len();
                edges += flow.out_edges.iter().map(Vec::len).sum::<usize>();
                functions += flow.functions.len();
                calls += flow.calls.len();
            }
        }
        let flow_time = started.elapsed();

        println!(
            "{} files, {:.1} MB: symbols {symbols} files in {symbols_time:?}; flow facts {parsed} files in {flow_time:?}; {nodes} nodes, {edges} edges, {functions} functions, {calls} calls",
            files.len(),
            bytes as f64 / 1e6,
        );
    }

    fn sources(flow: &FileFlow, node: u32) -> Vec<(FlowEdgeKind, String)> {
        flow.in_edges[node as usize]
            .iter()
            .map(|(from, kind)| {
                let n = &flow.nodes[*from as usize];
                (
                    *kind,
                    n.name
                        .clone()
                        .unwrap_or_else(|| n.kind.as_str().to_string()),
                )
            })
            .collect()
    }

    fn binding(flow: &FileFlow, name: &str) -> u32 {
        flow.nodes
            .iter()
            .position(|n| n.binding && n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no binding {name}")) as u32
    }

    #[test]
    fn destructuring_assignment_targets() {
        let source = "let a, b, c, d, rest;\n[a, [b] = fallback] = pair();\n({ c, x: d = 5, ...rest } = obj);\n[o.p, this.q] = vals;\nfor ([a] of entries) {}";
        let flow = flow(source);
        let a = sources(&flow, binding(&flow, "a"));
        assert!(
            a.contains(&(FlowEdgeKind::Element, "pair".to_string())),
            "{a:?}"
        );
        assert!(
            a.contains(&(FlowEdgeKind::Element, "entries".to_string())),
            "{a:?}"
        );
        let b = sources(&flow, binding(&flow, "b"));
        assert!(
            b.contains(&(FlowEdgeKind::Element, "pair".to_string())),
            "{b:?}"
        );
        assert!(
            b.contains(&(FlowEdgeKind::Element, "fallback".to_string())),
            "{b:?}"
        );
        let c = sources(&flow, binding(&flow, "c"));
        assert_eq!(c, [(FlowEdgeKind::Property, "obj".to_string())]);
        let d = sources(&flow, binding(&flow, "d"));
        assert!(
            d.contains(&(FlowEdgeKind::Property, "obj".to_string())),
            "{d:?}"
        );
        assert!(
            d.contains(&(FlowEdgeKind::Assign, "literal".to_string())),
            "{d:?}"
        );
        let rest = sources(&flow, binding(&flow, "rest"));
        assert_eq!(rest, [(FlowEdgeKind::Capture, "obj".to_string())]);
        let vals = node_at(&flow, source, "vals");
        let stores: Vec<_> = flow.out_edges[vals as usize]
            .iter()
            .filter(|(_, kind)| *kind == FlowEdgeKind::Store)
            .map(|(to, _)| flow.nodes[*to as usize].name.clone().unwrap())
            .collect();
        assert_eq!(stores, ["p", "q"]);
        let rebinds = flow
            .mutations
            .iter()
            .filter(|m| m.kind == MutationKind::Reassign)
            .count();
        // a, b, c, d, rest, and a again in the loop.
        assert_eq!(rebinds, 6);
        assert!(flow.skipped.is_empty(), "{:?}", flow.skipped);
    }

    #[test]
    fn class_fields_store_their_initializer() {
        let source = "class Store {\n  items = [];\n  #count = start;\n  static shared = make();\n  onClick = () => this.items.push(1);\n}";
        let flow = flow(source);
        let class = node_at(&flow, source, source);
        let fields: Vec<&str> = flow.class_fields[&class]
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(fields, ["items", "#count", "shared", "onClick"]);
        let items = flow.class_fields[&class][0].1;
        assert_eq!(
            sources(&flow, items),
            [(FlowEdgeKind::Store, "array".to_string())]
        );
        // An arrow-function field is a method.
        let methods: Vec<&str> = flow.class_methods[&class]
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(methods, ["onClick"]);
        assert!(flow.skipped.is_empty(), "{:?}", flow.skipped);
    }

    #[test]
    fn jsx_attributes_and_component_calls() {
        let source = "const App = ({ user }) => <Card title={user.name} />;";
        let flow = flow(source);
        let member = node_at(&flow, source, "user.name");
        let container = flow.out_edges[member as usize][0].0;
        let attribute = flow.out_edges[container as usize][0];
        assert_eq!(attribute.1, FlowEdgeKind::JsxAttribute);
        assert_eq!(
            flow.nodes[attribute.0 as usize].name.as_deref(),
            Some("title")
        );
        assert!(flow
            .calls
            .iter()
            .any(|c| c.is_jsx && c.callee_text == "Card"));
    }
}
