//! Value-flow facts for a Python module, built from Ruff's AST.
//!
//! The facts are the language-neutral [`FileFlow`] the queries in
//! `language-flow` walk. Names are resolved here with Python's scoping rules
//! (module, function, class and comprehension scopes, `global` and
//! `nonlocal`), not with ty's semantic model: the builder is fast and needs
//! no project setup, at the cost of precision. Types come from annotations
//! only; there is no checker behind them.

use language_core::{ByteRange, FlowEdgeKind, FlowNodeKind, MutationConfidence, MutationKind};
use language_flow::{
    Adjacency, CallFact, CalleeFact, FileFlow, FlowNode, FunctionFact, ImportFact, MutationFact,
    Name,
};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::HashMap;
use std::sync::Arc;

/// Methods that change their receiver (list, dict, set and friends).
const MUTATING_METHODS: &[&str] = &[
    "append",
    "extend",
    "insert",
    "remove",
    "pop",
    "popitem",
    "clear",
    "sort",
    "reverse",
    "update",
    "setdefault",
    "add",
    "discard",
    "difference_update",
    "intersection_update",
    "symmetric_difference_update",
    "__setitem__",
    "__delitem__",
    "appendleft",
    "extendleft",
    "popleft",
];

/// Methods that leave their receiver alone.
const PURE_METHODS: &[&str] = &[
    "get",
    "keys",
    "values",
    "items",
    "copy",
    "index",
    "count",
    "join",
    "split",
    "rsplit",
    "splitlines",
    "strip",
    "lstrip",
    "rstrip",
    "format",
    "format_map",
    "startswith",
    "endswith",
    "lower",
    "upper",
    "title",
    "capitalize",
    "replace",
    "encode",
    "decode",
    "find",
    "rfind",
    "isdigit",
    "isalpha",
    "isalnum",
    "__getitem__",
    // Django querysets return new querysets.
    "filter",
    "exclude",
    "all",
    "order_by",
    "values_list",
    "select_related",
    "prefetch_related",
    "annotate",
    "first",
    "last",
    "exists",
    "distinct",
    // pydantic v1
    "dict",
    "json",
];

/// Celery task methods that run the task itself: `task.delay(x)` calls task.
const TASK_CALLS: &[&str] = &["delay", "apply_async", "apply"];

/// Parse a module and collect its value-flow facts.
pub fn build_file_flow(content: &str) -> Result<FileFlow, String> {
    let parsed = ruff_python_parser::parse_module(content).map_err(|e| e.to_string())?;
    let mut builder = Builder::new(content);
    builder.module(&parsed.syntax().body);
    Ok(builder.finish())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Module,
    Function,
    Class,
    Comprehension,
}

struct Scope {
    kind: ScopeKind,
    /// The innermost function (its index) code in this scope runs in.
    function: Option<u32>,
    names: HashMap<String, u32>,
    globals: FxHashSet<String>,
    /// The class node, for a class body.
    class_node: Option<u32>,
}

/// A name a scope binds, from its first binding site.
struct Declaration {
    name: String,
    range: TextRange,
    kind: FlowNodeKind,
    import: Option<ImportFact>,
}

fn range_of(range: TextRange) -> ByteRange {
    ByteRange::new(range.start().to_u32(), range.end().to_u32())
}

struct Builder<'s> {
    source: &'s str,
    flow: FileFlow,
    out_edges: Vec<Vec<(u32, FlowEdgeKind)>>,
    in_edges: Vec<Vec<(u32, FlowEdgeKind)>>,
    by_span: FxHashMap<(u32, u32), u32>,
    names: FxHashSet<Name>,
    scopes: Vec<Scope>,
    /// `self` and `cls` parameters: their reads are `this`.
    self_bindings: FxHashSet<u32>,
}

impl<'s> Builder<'s> {
    fn new(source: &'s str) -> Self {
        Builder {
            source,
            flow: FileFlow::default(),
            out_edges: Vec::new(),
            in_edges: Vec::new(),
            by_span: FxHashMap::default(),
            names: FxHashSet::default(),
            scopes: Vec::new(),
            self_bindings: FxHashSet::default(),
        }
    }

    fn finish(mut self) -> FileFlow {
        let mut flow = self.flow;
        flow.out_edges = Adjacency::from_lists(std::mem::take(&mut self.out_edges));
        flow.in_edges = Adjacency::from_lists(std::mem::take(&mut self.in_edges));
        flow.index_spans();
        flow.bytes = flow.estimate_bytes(&self.names);
        flow
    }

    fn intern(&mut self, name: &str) -> Name {
        if let Some(found) = self.names.get(name) {
            return Arc::clone(found);
        }
        let name: Name = name.into();
        self.names.insert(Arc::clone(&name));
        name
    }

    fn text(&self, range: TextRange) -> &'s str {
        &self.source[range.start().to_usize()..range.end().to_usize()]
    }

    fn node(&mut self, range: TextRange) -> u32 {
        let key = (range.start().to_u32(), range.end().to_u32());
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

    /// Describe a node. Two syntax nodes can share a range; the first
    /// specific description wins.
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
        if from == to {
            return;
        }
        if self.out_edges[from as usize].contains(&(to, kind)) {
            return;
        }
        self.out_edges[from as usize].push((to, kind));
        self.in_edges[to as usize].push((from, kind));
    }

    fn skip(&mut self, what: &str) {
        *self.flow.skipped.entry(what.to_string()).or_default() += 1;
    }

    fn function(&self) -> Option<u32> {
        self.scopes.last().and_then(|scope| scope.function)
    }

    /// The class whose body is the innermost scope.
    fn class_scope(&self) -> Option<u32> {
        self.scopes
            .last()
            .filter(|scope| scope.kind == ScopeKind::Class)
            .and_then(|scope| scope.class_node)
    }

    /// The binding a name refers to here.
    fn resolve(&self, name: &str) -> Option<u32> {
        let top = self.scopes.len().checked_sub(1)?;
        for (index, scope) in self.scopes.iter().enumerate().rev() {
            // A class body's names are not visible in the functions inside it.
            if scope.kind == ScopeKind::Class && index != top {
                continue;
            }
            if scope.globals.contains(name) {
                return self.scopes[0].names.get(name).copied();
            }
            if let Some(binding) = scope.names.get(name) {
                return Some(*binding);
            }
        }
        None
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
        let name = name.map(|name| self.intern(name));
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

    /// The binding at the root of `a.b[c].d`.
    fn root_binding(&self, expr: &Expr) -> Option<u32> {
        match expr {
            Expr::Name(name) => self.resolve(name.id.as_str()),
            Expr::Attribute(attribute) => self.root_binding(&attribute.value),
            Expr::Subscript(subscript) => self.root_binding(&subscript.value),
            _ => None,
        }
    }

    // ---- scopes and declarations ----

    fn push_scope(&mut self, kind: ScopeKind, function: Option<u32>, class_node: Option<u32>) {
        self.scopes.push(Scope {
            kind,
            function,
            names: HashMap::new(),
            globals: FxHashSet::default(),
            class_node,
        });
    }

    /// Bind a name in the innermost scope (if it has no binding yet).
    fn declare_one(&mut self, declaration: Declaration) -> u32 {
        if let Some(existing) = self
            .scopes
            .last()
            .and_then(|s| s.names.get(&declaration.name))
        {
            return *existing;
        }
        let binding = self.describe(declaration.range, declaration.kind, Some(&declaration.name));
        let node = &mut self.flow.nodes[binding as usize];
        node.kind = declaration.kind;
        node.binding = true;
        if let Some(import) = declaration.import {
            self.flow.imports.insert(binding, import);
        }
        let scope = self.scopes.last_mut().expect("a scope");
        scope.names.insert(declaration.name.clone(), binding);
        let kind = scope.kind;
        let class_node = scope.class_node;
        if kind == ScopeKind::Module {
            let name = self.intern(&declaration.name);
            self.flow.module_bindings.insert(name, binding);
        }
        // `objects = Manager()`, `name: str`: class attributes are fields.
        if let (ScopeKind::Class, Some(class_node)) = (kind, class_node) {
            let name = self.intern(&declaration.name);
            let fields = self.flow.class_fields.entry(class_node).or_default();
            if !fields.iter().any(|(_, node)| *node == binding) {
                fields.push((name, binding));
            }
        }
        binding
    }

    /// Declare the names a block binds, before any of it is visited: in
    /// Python a name assigned anywhere in a function is local to all of it.
    fn declare_block(&mut self, body: &[Stmt]) {
        let mut declarations = Vec::new();
        let mut globals = FxHashSet::default();
        let in_class = self
            .scopes
            .last()
            .is_some_and(|scope| scope.kind == ScopeKind::Class);
        collect_declarations(body, in_class, &mut declarations, &mut globals);
        if let Some(scope) = self.scopes.last_mut() {
            scope.globals.extend(globals.iter().cloned());
        }
        for declaration in declarations {
            if globals.contains(&declaration.name) {
                continue;
            }
            self.declare_one(declaration);
        }
    }

    // ---- statements ----

    fn module(&mut self, body: &[Stmt]) {
        self.push_scope(ScopeKind::Module, None, None);
        self.declare_block(body);
        self.statements(body);
        self.scopes.pop();
    }

    fn statements(&mut self, body: &[Stmt]) {
        for statement in body {
            self.statement(statement);
        }
    }

    fn statement(&mut self, statement: &Stmt) {
        match statement {
            Stmt::FunctionDef(function) => self.function_def(function),
            Stmt::ClassDef(class) => self.class_def(class),
            Stmt::Return(ret) => {
                if let Some(value) = &ret.value {
                    let value = self.expr(value);
                    if let Some(function) = self.function() {
                        let node = self.flow.functions[function as usize].node;
                        self.edge(value, node, FlowEdgeKind::Return);
                    }
                }
            }
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    match target {
                        Expr::Name(name) => {
                            let binding = self.resolve(name.id.as_str());
                            self.mutation(
                                binding,
                                delete.range,
                                MutationKind::Delete,
                                MutationConfidence::Definite,
                                None,
                            );
                        }
                        Expr::Attribute(attribute) => {
                            self.expr(&attribute.value);
                            let binding = self.root_binding(&attribute.value);
                            self.mutation(
                                binding,
                                delete.range,
                                MutationKind::Delete,
                                MutationConfidence::Definite,
                                Some(attribute.attr.as_str()),
                            );
                        }
                        Expr::Subscript(subscript) => {
                            self.expr(&subscript.value);
                            self.expr(&subscript.slice);
                            let binding = self.root_binding(&subscript.value);
                            let key = string_key(&subscript.slice);
                            self.mutation(
                                binding,
                                delete.range,
                                MutationKind::Delete,
                                MutationConfidence::Definite,
                                key.as_deref(),
                            );
                        }
                        _ => {}
                    }
                }
            }
            Stmt::Assign(assign) => {
                let value = self.expr(&assign.value);
                for target in &assign.targets {
                    self.bind_target(target, value, FlowEdgeKind::Assign, assign.range);
                }
            }
            Stmt::AugAssign(assign) => {
                let value = self.expr(&assign.value);
                let current = self.expr(&assign.target);
                match &*assign.target {
                    Expr::Name(name) => {
                        if let Some(binding) = self.resolve(name.id.as_str()) {
                            self.edge(value, binding, FlowEdgeKind::Assign);
                            self.mutation(
                                Some(binding),
                                assign.range,
                                MutationKind::Update,
                                MutationConfidence::Definite,
                                None,
                            );
                        }
                    }
                    Expr::Attribute(attribute) => {
                        self.edge(value, current, FlowEdgeKind::Store);
                        let binding = self.root_binding(&attribute.value);
                        self.mutation(
                            binding,
                            assign.range,
                            MutationKind::Update,
                            MutationConfidence::Definite,
                            Some(attribute.attr.as_str()),
                        );
                    }
                    Expr::Subscript(subscript) => {
                        self.edge(value, current, FlowEdgeKind::Store);
                        let binding = self.root_binding(&subscript.value);
                        let key = string_key(&subscript.slice);
                        self.mutation(
                            binding,
                            assign.range,
                            MutationKind::Update,
                            MutationConfidence::Definite,
                            Some(key.as_deref().unwrap_or("[]")),
                        );
                    }
                    _ => {}
                }
            }
            Stmt::AnnAssign(assign) => {
                let declared = self.annotation(&assign.annotation);
                let text = self.text(assign.annotation.range());
                match &*assign.target {
                    Expr::Name(name) => {
                        if let Some(binding) = self.resolve(name.id.as_str()) {
                            self.declare_type(binding, declared, text);
                        }
                    }
                    Expr::Attribute(attribute) => {
                        let target = self.attribute_target(attribute);
                        self.declare_type(target, declared, text);
                    }
                    _ => {}
                }
                if let Some(value) = &assign.value {
                    let value = self.expr(value);
                    self.bind_target(&assign.target, value, FlowEdgeKind::Assign, assign.range);
                }
            }
            Stmt::TypeAlias(_) => {}
            Stmt::For(for_stmt) => {
                let iter = self.expr(&for_stmt.iter);
                self.bind_target(
                    &for_stmt.target,
                    iter,
                    FlowEdgeKind::Element,
                    for_stmt.target.range(),
                );
                self.flow.loops.push(range_of(for_stmt.range));
                self.statements(&for_stmt.body);
                self.statements(&for_stmt.orelse);
            }
            Stmt::While(while_stmt) => {
                self.expr(&while_stmt.test);
                self.flow.loops.push(range_of(while_stmt.range));
                self.statements(&while_stmt.body);
                self.statements(&while_stmt.orelse);
            }
            Stmt::If(if_stmt) => {
                self.expr(&if_stmt.test);
                self.statements(&if_stmt.body);
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.expr(test);
                    }
                    self.statements(&clause.body);
                }
            }
            Stmt::With(with) => {
                for item in &with.items {
                    let context = self.expr(&item.context_expr);
                    if let Some(vars) = &item.optional_vars {
                        self.bind_target(vars, context, FlowEdgeKind::Assign, vars.range());
                    }
                }
                self.statements(&with.body);
            }
            Stmt::Match(match_stmt) => {
                self.expr(&match_stmt.subject);
                for case in &match_stmt.cases {
                    if let Some(guard) = &case.guard {
                        self.expr(guard);
                    }
                    self.statements(&case.body);
                }
                self.skip("match-pattern");
            }
            Stmt::Raise(raise) => {
                if let Some(exc) = &raise.exc {
                    self.expr(exc);
                }
                if let Some(cause) = &raise.cause {
                    self.expr(cause);
                }
            }
            Stmt::Try(try_stmt) => {
                self.statements(&try_stmt.body);
                for handler in &try_stmt.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    if let Some(type_) = &handler.type_ {
                        self.expr(type_);
                    }
                    self.statements(&handler.body);
                }
                self.statements(&try_stmt.orelse);
                self.statements(&try_stmt.finalbody);
            }
            Stmt::Assert(assert) => {
                self.expr(&assert.test);
                if let Some(msg) = &assert.msg {
                    self.expr(msg);
                }
            }
            Stmt::Import(_) | Stmt::ImportFrom(_) => {}
            Stmt::Global(_) | Stmt::Nonlocal(_) => {}
            Stmt::Expr(expr) => {
                self.expr(&expr.value);
            }
            Stmt::Pass(_) | Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::IpyEscapeCommand(_) => {}
        }
    }

    fn declare_type(&mut self, node: u32, declared: Option<u32>, text: &str) {
        if let Some(declared) = declared {
            self.flow.annotations.entry(node).or_insert(declared);
        }
        let text = self.intern(text);
        self.flow.declared_types.entry(node).or_insert(text);
    }

    /// The node a type annotation names, for call resolution: `Repo`,
    /// `models.Repo`, the `X` of `Optional[X]` and `X | None`, or a string
    /// forward reference to a name in scope.
    fn annotation(&mut self, annotation: &Expr) -> Option<u32> {
        match annotation {
            Expr::Name(_) | Expr::Attribute(_) => Some(self.expr(annotation)),
            Expr::StringLiteral(literal) => {
                let text = literal.value.to_str();
                let first = text.split('.').next().unwrap_or(text).trim();
                self.resolve(first)
            }
            Expr::Subscript(subscript) => {
                let outer = self.text(subscript.value.range());
                let generic = outer.rsplit('.').next().unwrap_or(outer);
                match generic {
                    "Optional" | "Annotated" | "Type" | "type" => match &*subscript.slice {
                        Expr::Tuple(tuple) => tuple.elts.first().and_then(|e| self.annotation(e)),
                        other => self.annotation(other),
                    },
                    "Union" => match &*subscript.slice {
                        Expr::Tuple(tuple) => tuple
                            .elts
                            .iter()
                            .find(|e| !matches!(e, Expr::NoneLiteral(_)))
                            .and_then(|e| self.annotation(e)),
                        other => self.annotation(other),
                    },
                    _ => None,
                }
            }
            Expr::BinOp(binop) => {
                if matches!(&*binop.left, Expr::NoneLiteral(_)) {
                    self.annotation(&binop.right)
                } else {
                    self.annotation(&binop.left)
                }
            }
            _ => None,
        }
    }

    /// The node for `obj.attr` as an assignment target, with `obj` linked to
    /// it. Assigning `self.attr` in a method makes `attr` a field of the class.
    fn attribute_target(&mut self, attribute: &ast::ExprAttribute) -> u32 {
        let object = self.expr(&attribute.value);
        let target = self.describe(
            attribute.range,
            FlowNodeKind::Member,
            Some(attribute.attr.as_str()),
        );
        self.edge(object, target, FlowEdgeKind::Property);
        if self.flow.nodes[object as usize].kind == FlowNodeKind::This {
            if let Some(class_node) = self.flow.class_of(self.function()) {
                let name = self.intern(attribute.attr.as_str());
                let fields = self.flow.class_fields.entry(class_node).or_default();
                if !fields.iter().any(|(_, node)| *node == target) {
                    fields.push((name, target));
                }
            }
        }
        target
    }

    /// Edges from a value into an assignment target.
    fn bind_target(&mut self, target: &Expr, value: u32, kind: FlowEdgeKind, whole: TextRange) {
        match target {
            Expr::Name(name) => {
                let Some(binding) = self.resolve(name.id.as_str()) else {
                    return;
                };
                // A class attribute's value is stored in the class.
                let kind = if self.class_scope().is_some() {
                    FlowEdgeKind::Store
                } else {
                    kind
                };
                self.edge(value, binding, kind);
                // The first assignment declares the name; later ones rebind it.
                if self.flow.nodes[binding as usize].range != range_of(name.range) {
                    self.mutation(
                        Some(binding),
                        whole,
                        MutationKind::Reassign,
                        MutationConfidence::Definite,
                        None,
                    );
                }
            }
            Expr::Attribute(attribute) => {
                let node = self.attribute_target(attribute);
                self.edge(value, node, FlowEdgeKind::Store);
                let binding = self.root_binding(&attribute.value);
                self.mutation(
                    binding,
                    whole,
                    MutationKind::PropertyWrite,
                    MutationConfidence::Definite,
                    Some(attribute.attr.as_str()),
                );
            }
            Expr::Subscript(subscript) => {
                let object = self.expr(&subscript.value);
                self.expr(&subscript.slice);
                let key = string_key(&subscript.slice);
                let node = self.describe(subscript.range, FlowNodeKind::Member, key.as_deref());
                self.edge(object, node, FlowEdgeKind::Element);
                self.edge(value, node, FlowEdgeKind::Store);
                let binding = self.root_binding(&subscript.value);
                self.mutation(
                    binding,
                    whole,
                    MutationKind::PropertyWrite,
                    MutationConfidence::Definite,
                    key.as_deref(),
                );
            }
            Expr::Tuple(ast::ExprTuple { elts, .. }) | Expr::List(ast::ExprList { elts, .. }) => {
                for (position, element) in elts.iter().enumerate() {
                    if let Expr::Starred(starred) = element {
                        self.bind_target(&starred.value, value, FlowEdgeKind::Capture, whole);
                        continue;
                    }
                    self.bind_target(element, value, FlowEdgeKind::Element, whole);
                    if let Expr::Name(name) = element {
                        if let Some(binding) = self.resolve(name.id.as_str()) {
                            self.flow
                                .element_index
                                .insert((value, binding), position as u32);
                        }
                    }
                }
            }
            Expr::Starred(starred) => {
                self.bind_target(&starred.value, value, FlowEdgeKind::Capture, whole)
            }
            _ => self.skip("assignment-target"),
        }
    }

    /// Where `def`/`class` starts, past the decorators (the syntax tree's
    /// function_definition and class_definition nodes start there).
    fn definition_range(&self, range: TextRange, decorators: &[ast::Decorator]) -> TextRange {
        let Some(last) = decorators.last() else {
            return range;
        };
        let bytes = self.source.as_bytes();
        let mut at = last.range.end().to_usize();
        while at < range.end().to_usize() {
            match bytes[at] {
                b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' => at += 1,
                b'#' => {
                    while at < bytes.len() && bytes[at] != b'\n' {
                        at += 1;
                    }
                }
                _ => break,
            }
        }
        TextRange::new((at as u32).into(), range.end())
    }

    fn parameters(parameters: &ast::Parameters) -> Vec<(&ast::Parameter, Option<&Expr>)> {
        let mut all = Vec::new();
        for p in parameters.posonlyargs.iter().chain(parameters.args.iter()) {
            all.push((&p.parameter, p.default.as_deref()));
        }
        if let Some(vararg) = &parameters.vararg {
            all.push((&**vararg, None));
        }
        for p in &parameters.kwonlyargs {
            all.push((&p.parameter, p.default.as_deref()));
        }
        if let Some(kwarg) = &parameters.kwarg {
            all.push((&**kwarg, None));
        }
        all
    }

    fn function_def(&mut self, function: &ast::StmtFunctionDef) {
        let class_node = self.class_scope();
        let parent = self.function();
        let range = self.definition_range(function.range, &function.decorator_list);
        let name = function.name.as_str();

        let index = self.flow.functions.len() as u32;
        let node = self.describe(range, FlowNodeKind::Function, Some(name));
        self.flow.nodes[node as usize].kind = FlowNodeKind::Function;
        self.flow.nodes[node as usize].function = parent;
        let interned = self.intern(name);
        let mut decorator_names = Vec::new();
        let mut is_static = false;
        let mut accessor = None;
        for decorator in &function.decorator_list {
            let text = self.text(decorator.expression.range());
            match text {
                "staticmethod" => is_static = true,
                "property" | "cached_property" | "functools.cached_property" => {
                    accessor = Some(language_flow::Accessor::Get)
                }
                _ if text.ends_with(".setter") => accessor = Some(language_flow::Accessor::Set),
                _ => {}
            }
            decorator_names.push(self.intern(text));
        }
        self.flow.functions.push(FunctionFact {
            range: range_of(range),
            node,
            name: Some(interned.clone()),
            name_range: Some(range_of(function.name.range)),
            parent,
            class_node,
            accessor,
        });
        self.flow.fn_by_node.insert(node, index);
        match class_node {
            Some(class_node) => {
                self.flow
                    .class_methods
                    .entry(class_node)
                    .or_default()
                    .push((interned, index));
            }
            None => {
                if let Some(binding) = self.resolve(name) {
                    self.edge(node, binding, FlowEdgeKind::Assign);
                }
            }
        }

        // Decorators run where the function is defined, and are handed it:
        // `@app.get("/")` and `@shared_task` are calls of the decorator.
        for decorator in &function.decorator_list {
            let callee = self.expr(&decorator.expression);
            let text = self.text(decorator.expression.range());
            let call = self.describe(decorator.range, FlowNodeKind::Call, Some(text));
            self.edge(node, call, FlowEdgeKind::Argument);
            let callee_text = self.intern(text);
            self.flow
                .call_by_node
                .insert(call, self.flow.calls.len() as u32);
            self.flow.calls.push(CallFact {
                range: range_of(decorator.range),
                function: parent,
                callee: CalleeFact::Node(callee),
                callee_text,
                args: vec![node].into_boxed_slice(),
                is_new: false,
                is_jsx: false,
            });
        }
        if !decorator_names.is_empty() {
            self.flow.decorators.insert(index, decorator_names);
        }

        // Defaults and annotations are evaluated where the function is defined.
        let parameters = Self::parameters(&function.parameters);
        let mut evaluated = Vec::with_capacity(parameters.len());
        for (parameter, default) in &parameters {
            let default = default.map(|d| self.expr(d));
            let declared = parameter.annotation.as_deref().map(|a| {
                let text = self.text(a.range());
                (self.annotation(a), text)
            });
            evaluated.push((default, declared));
        }
        if let Some(returns) = &function.returns {
            let declared = self.annotation(returns);
            let text = self.text(returns.range());
            self.declare_type(node, declared, text);
        }

        self.push_scope(ScopeKind::Function, Some(index), None);
        for (position, ((parameter, _), (default, declared))) in
            parameters.iter().zip(evaluated).enumerate()
        {
            let binding = self.declare_one(Declaration {
                name: parameter.name.to_string(),
                range: parameter.name.range,
                kind: FlowNodeKind::Parameter,
                import: None,
            });
            self.flow.nodes[binding as usize].index = Some(position as u32);
            if let Some(default) = default {
                self.edge(default, binding, FlowEdgeKind::Assign);
            }
            if let Some((declared, text)) = declared {
                self.declare_type(binding, declared, text);
            }
            // `self` (or `cls`): the instance of the class.
            if position == 0 && !is_static {
                if let Some(class_node) = class_node {
                    self.self_bindings.insert(binding);
                    self.flow.annotations.entry(binding).or_insert(class_node);
                }
            }
        }
        self.declare_block(&function.body);
        self.statements(&function.body);
        self.scopes.pop();
    }

    fn class_def(&mut self, class: &ast::StmtClassDef) {
        let range = self.definition_range(class.range, &class.decorator_list);
        let name = class.name.as_str();
        let node = self.describe(range, FlowNodeKind::Class, Some(name));
        self.flow.nodes[node as usize].kind = FlowNodeKind::Class;
        if self.class_scope().is_none() {
            if let Some(binding) = self.resolve(name) {
                self.edge(node, binding, FlowEdgeKind::Assign);
            }
        }
        for decorator in &class.decorator_list {
            self.expr(&decorator.expression);
        }
        if let Some(arguments) = &class.arguments {
            let mut first = true;
            for base in arguments.args.iter() {
                let base_node = self.expr(base);
                // Multiple inheritance: the first base is the one followed.
                if first {
                    self.flow.class_extends.insert(node, base_node);
                    first = false;
                }
            }
            for keyword in arguments.keywords.iter() {
                self.expr(&keyword.value);
            }
        }
        let function = self.function();
        self.push_scope(ScopeKind::Class, function, Some(node));
        self.declare_block(&class.body);
        self.statements(&class.body);
        self.scopes.pop();
    }

    // ---- expressions ----

    fn expr(&mut self, expr: &Expr) -> u32 {
        let range = expr.range();
        match expr {
            Expr::Name(name) => {
                let index = self.node(range);
                match self.resolve(name.id.as_str()) {
                    Some(binding) if binding != index => {
                        self.edge(binding, index, FlowEdgeKind::Read);
                        let kind = if self.self_bindings.contains(&binding) {
                            FlowNodeKind::This
                        } else {
                            FlowNodeKind::Reference
                        };
                        self.describe(range, kind, Some(name.id.as_str()))
                    }
                    Some(binding) => binding,
                    None => self.describe(range, FlowNodeKind::Global, Some(name.id.as_str())),
                }
            }
            Expr::Attribute(attribute) => {
                let object = self.expr(&attribute.value);
                let index =
                    self.describe(range, FlowNodeKind::Member, Some(attribute.attr.as_str()));
                self.edge(object, index, FlowEdgeKind::Property);
                index
            }
            Expr::Subscript(subscript) => {
                let object = self.expr(&subscript.value);
                self.expr(&subscript.slice);
                let key = string_key(&subscript.slice);
                let index = self.describe(range, FlowNodeKind::Member, key.as_deref());
                self.edge(object, index, FlowEdgeKind::Element);
                index
            }
            Expr::Call(call) => self.call(call),
            Expr::FString(fstring) => {
                let mut parts = Vec::new();
                for element in fstring.value.elements() {
                    if let ast::InterpolatedStringElement::Interpolation(interpolation) = element {
                        parts.push(&*interpolation.expression);
                    }
                }
                let kind = if parts.is_empty() {
                    FlowNodeKind::Literal
                } else {
                    FlowNodeKind::Expression
                };
                let index = self.describe(range, kind, None);
                for part in parts {
                    let part = self.expr(part);
                    self.edge(part, index, FlowEdgeKind::Derive);
                }
                index
            }
            Expr::TString(_) => self.describe(range, FlowNodeKind::Expression, None),
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_) => self.describe(range, FlowNodeKind::Literal, None),
            Expr::BinOp(binop) => {
                let left = self.expr(&binop.left);
                let right = self.expr(&binop.right);
                let index = self.describe(range, FlowNodeKind::Expression, None);
                self.edge(left, index, FlowEdgeKind::Derive);
                self.edge(right, index, FlowEdgeKind::Derive);
                index
            }
            Expr::UnaryOp(unary) => {
                let operand = self.expr(&unary.operand);
                let index = self.describe(range, FlowNodeKind::Expression, None);
                self.edge(operand, index, FlowEdgeKind::Derive);
                index
            }
            Expr::BoolOp(boolop) => {
                let values: Vec<u32> = boolop.values.iter().map(|v| self.expr(v)).collect();
                let index = self.describe(range, FlowNodeKind::Expression, None);
                for value in values {
                    self.edge(value, index, FlowEdgeKind::Alias);
                }
                index
            }
            Expr::Compare(compare) => {
                let mut values = vec![self.expr(&compare.left)];
                for comparator in compare.comparators.iter() {
                    values.push(self.expr(comparator));
                }
                let index = self.describe(range, FlowNodeKind::Expression, None);
                for value in values {
                    self.edge(value, index, FlowEdgeKind::Derive);
                }
                index
            }
            Expr::If(if_expr) => {
                self.expr(&if_expr.test);
                let body = self.expr(&if_expr.body);
                let orelse = self.expr(&if_expr.orelse);
                let index = self.describe(range, FlowNodeKind::Expression, None);
                self.edge(body, index, FlowEdgeKind::Alias);
                self.edge(orelse, index, FlowEdgeKind::Alias);
                index
            }
            Expr::Await(await_expr) => {
                let value = self.expr(&await_expr.value);
                let index = self.describe(range, FlowNodeKind::Expression, None);
                self.edge(value, index, FlowEdgeKind::Derive);
                index
            }
            Expr::Yield(yield_expr) => {
                let index = self.describe(range, FlowNodeKind::Expression, None);
                if let Some(value) = &yield_expr.value {
                    let value = self.expr(value);
                    self.edge(value, index, FlowEdgeKind::Derive);
                }
                index
            }
            Expr::YieldFrom(yield_from) => {
                let value = self.expr(&yield_from.value);
                let index = self.describe(range, FlowNodeKind::Expression, None);
                self.edge(value, index, FlowEdgeKind::Derive);
                index
            }
            Expr::Named(named) => {
                let value = self.expr(&named.value);
                let index = self.describe(range, FlowNodeKind::Expression, None);
                self.edge(value, index, FlowEdgeKind::Alias);
                if let Expr::Name(name) = &*named.target {
                    // `(x := v)` binds in the enclosing function.
                    let binding = match self.resolve(name.id.as_str()) {
                        Some(binding) => binding,
                        None => self.declare_one(Declaration {
                            name: name.id.to_string(),
                            range: name.range,
                            kind: FlowNodeKind::Variable,
                            import: None,
                        }),
                    };
                    self.edge(value, binding, FlowEdgeKind::Assign);
                }
                index
            }
            Expr::Lambda(lambda) => self.lambda(lambda),
            Expr::List(ast::ExprList { elts, .. })
            | Expr::Tuple(ast::ExprTuple { elts, .. })
            | Expr::Set(ast::ExprSet { elts, .. }) => {
                let values: Vec<u32> = elts.iter().map(|e| self.expr(e)).collect();
                let index = self.describe(range, FlowNodeKind::Array, None);
                for &value in &values {
                    self.edge(value, index, FlowEdgeKind::Capture);
                }
                // Positions up to the first starred element.
                let cut = elts
                    .iter()
                    .position(|e| matches!(e, Expr::Starred(_)))
                    .unwrap_or(elts.len());
                let positions: Vec<Option<u32>> = values[..cut].iter().map(|v| Some(*v)).collect();
                if !positions.is_empty() {
                    self.flow.array_elements.insert(index, positions);
                }
                index
            }
            Expr::Dict(dict) => {
                let mut members = Vec::new();
                let mut values = Vec::new();
                for item in &dict.items {
                    if let Some(key) = &item.key {
                        self.expr(key);
                    }
                    let value = self.expr(&item.value);
                    values.push(value);
                    if let Some(name) = item.key.as_ref().and_then(string_key) {
                        members.push((name, value));
                    }
                }
                let index = self.describe(range, FlowNodeKind::Object, None);
                for value in values {
                    self.edge(value, index, FlowEdgeKind::Capture);
                }
                for (name, value) in members {
                    let name = self.intern(&name);
                    self.flow
                        .object_members
                        .entry(index)
                        .or_default()
                        .push((name, value));
                }
                index
            }
            Expr::ListComp(comp) => {
                self.comprehension(range, &comp.generators, &[&comp.elt], FlowNodeKind::Array)
            }
            Expr::SetComp(comp) => {
                self.comprehension(range, &comp.generators, &[&comp.elt], FlowNodeKind::Array)
            }
            Expr::Generator(comp) => self.comprehension(
                range,
                &comp.generators,
                &[&comp.elt],
                FlowNodeKind::Expression,
            ),
            Expr::DictComp(comp) => self.comprehension(
                range,
                &comp.generators,
                &[&comp.key, &comp.value],
                FlowNodeKind::Object,
            ),
            Expr::Starred(starred) => {
                let value = self.expr(&starred.value);
                let index = self.describe(range, FlowNodeKind::Expression, None);
                self.edge(value, index, FlowEdgeKind::Alias);
                index
            }
            Expr::Slice(slice) => {
                for part in [&slice.lower, &slice.upper, &slice.step]
                    .into_iter()
                    .flatten()
                {
                    self.expr(part);
                }
                self.describe(range, FlowNodeKind::Expression, None)
            }
            Expr::IpyEscapeCommand(_) => self.describe(range, FlowNodeKind::Expression, None),
        }
    }

    fn comprehension(
        &mut self,
        range: TextRange,
        generators: &[ast::Comprehension],
        elements: &[&Expr],
        kind: FlowNodeKind,
    ) -> u32 {
        let index = self.describe(range, kind, None);
        self.flow.loops.push(range_of(range));
        let function = self.function();
        self.push_scope(ScopeKind::Comprehension, function, None);
        for generator in generators {
            let iter = self.expr(&generator.iter);
            let mut declarations = Vec::new();
            target_declarations(&generator.target, &mut declarations);
            for declaration in declarations {
                self.declare_one(declaration);
            }
            self.bind_target(
                &generator.target,
                iter,
                FlowEdgeKind::Element,
                generator.target.range(),
            );
            for condition in &generator.ifs {
                self.expr(condition);
            }
        }
        for element in elements {
            let value = self.expr(element);
            self.edge(value, index, FlowEdgeKind::Capture);
        }
        self.scopes.pop();
        index
    }

    fn lambda(&mut self, lambda: &ast::ExprLambda) -> u32 {
        let parent = self.function();
        let index = self.flow.functions.len() as u32;
        let node = self.describe(lambda.range, FlowNodeKind::Function, None);
        self.flow.nodes[node as usize].kind = FlowNodeKind::Function;
        self.flow.functions.push(FunctionFact {
            range: range_of(lambda.range),
            node,
            name: None,
            name_range: None,
            parent,
            class_node: None,
            accessor: None,
        });
        self.flow.fn_by_node.insert(node, index);
        let parameters = lambda
            .parameters
            .as_deref()
            .map(Self::parameters)
            .unwrap_or_default();
        let defaults: Vec<Option<u32>> = parameters
            .iter()
            .map(|(_, default)| default.map(|d| self.expr(d)))
            .collect();
        self.push_scope(ScopeKind::Function, Some(index), None);
        for (position, ((parameter, _), default)) in parameters.iter().zip(defaults).enumerate() {
            let binding = self.declare_one(Declaration {
                name: parameter.name.to_string(),
                range: parameter.name.range,
                kind: FlowNodeKind::Parameter,
                import: None,
            });
            self.flow.nodes[binding as usize].index = Some(position as u32);
            if let Some(default) = default {
                self.edge(default, binding, FlowEdgeKind::Assign);
            }
        }
        let body = self.expr(&lambda.body);
        self.edge(body, node, FlowEdgeKind::Return);
        self.scopes.pop();
        node
    }

    fn call(&mut self, call: &ast::ExprCall) -> u32 {
        let range = call.range;
        let call_node = self.node(range);
        let callee_text = self.text(call.func.range());
        let callee = match &*call.func {
            Expr::Attribute(attribute) => {
                let object = self.expr(&attribute.value);
                let member = self.describe(
                    attribute.range,
                    FlowNodeKind::Member,
                    Some(attribute.attr.as_str()),
                );
                self.edge(object, member, FlowEdgeKind::Property);
                self.edge(object, call_node, FlowEdgeKind::Receiver);
                let name = attribute.attr.as_str();
                if !PURE_METHODS.contains(&name) && !TASK_CALLS.contains(&name) {
                    let confidence = if MUTATING_METHODS.contains(&name) {
                        MutationConfidence::Likely
                    } else {
                        MutationConfidence::Maybe
                    };
                    let binding = self.root_binding(&attribute.value);
                    self.mutation(
                        binding,
                        range,
                        MutationKind::MethodCall,
                        confidence,
                        Some(name),
                    );
                }
                // `super().m()`: m of the parent classes.
                if let Expr::Call(inner) = &*attribute.value {
                    if matches!(&*inner.func, Expr::Name(n) if n.id.as_str() == "super") {
                        self.flow.super_nodes.insert(object);
                    }
                }
                if TASK_CALLS.contains(&name) {
                    // `send_email.delay(x)` runs send_email.
                    CalleeFact::Node(object)
                } else {
                    CalleeFact::Member {
                        object,
                        property: self.intern(name),
                    }
                }
            }
            other => CalleeFact::Node(self.expr(other)),
        };

        // setattr(obj, "name", v) and delattr(obj, "name") write obj.
        if let Expr::Name(name) = &*call.func {
            let builtin = self.resolve(name.id.as_str()).is_none();
            let kind = match name.id.as_str() {
                "setattr" if builtin => Some(MutationKind::PropertyWrite),
                "delattr" if builtin => Some(MutationKind::Delete),
                _ => None,
            };
            if let (Some(kind), Some(target)) = (kind, call.arguments.args.first()) {
                let binding = self.root_binding(target);
                let key = call.arguments.args.get(1).and_then(string_key);
                self.mutation(
                    binding,
                    range,
                    kind,
                    MutationConfidence::Definite,
                    key.as_deref(),
                );
            }
        }

        let mut args = Vec::new();
        let argument_exprs: Vec<&Expr> = call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|k| &k.value))
            .collect();
        for argument in argument_exprs {
            let value = self.expr(argument);
            self.edge(value, call_node, FlowEdgeKind::Argument);
            args.push(value);
            // A callee with no summary may change any object it is handed.
            let inner = match argument {
                Expr::Starred(starred) => &*starred.value,
                other => other,
            };
            if let Expr::Name(name) = inner {
                let binding = self.resolve(name.id.as_str());
                self.mutation(
                    binding,
                    range,
                    MutationKind::Escape,
                    MutationConfidence::Maybe,
                    None,
                );
            }
        }

        self.describe(range, FlowNodeKind::Call, Some(callee_text));
        let callee_text = self.intern(callee_text);
        let function = self.function();
        self.flow
            .call_by_node
            .insert(call_node, self.flow.calls.len() as u32);
        self.flow.calls.push(CallFact {
            range: range_of(range),
            function,
            callee,
            callee_text,
            args: args.into_boxed_slice(),
            is_new: false,
            is_jsx: false,
        });
        call_node
    }
}

/// The text of a string-literal key: `d["q"]`, `{"q": v}`.
fn string_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(literal) => Some(literal.value.to_str().to_string()),
        Expr::NumberLiteral(_) => None,
        _ => None,
    }
}

/// Names bound by an assignment target.
fn target_declarations(target: &Expr, out: &mut Vec<Declaration>) {
    match target {
        Expr::Name(name) => out.push(Declaration {
            name: name.id.to_string(),
            range: name.range,
            kind: FlowNodeKind::Variable,
            import: None,
        }),
        Expr::Tuple(ast::ExprTuple { elts, .. }) | Expr::List(ast::ExprList { elts, .. }) => {
            for element in elts {
                target_declarations(element, out);
            }
        }
        Expr::Starred(starred) => target_declarations(&starred.value, out),
        _ => {}
    }
}

/// The names a block binds, in order of their first binding site, without
/// looking into nested functions and classes (but binding their names).
fn collect_declarations(
    body: &[Stmt],
    in_class: bool,
    out: &mut Vec<Declaration>,
    globals: &mut FxHashSet<String>,
) {
    for statement in body {
        match statement {
            Stmt::FunctionDef(function) => {
                if !in_class {
                    out.push(Declaration {
                        name: function.name.to_string(),
                        range: function.name.range,
                        kind: FlowNodeKind::Variable,
                        import: None,
                    });
                }
            }
            Stmt::ClassDef(class) => {
                if !in_class {
                    out.push(Declaration {
                        name: class.name.to_string(),
                        range: class.name.range,
                        kind: FlowNodeKind::Variable,
                        import: None,
                    });
                }
            }
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    target_declarations(target, out);
                }
            }
            Stmt::AugAssign(assign) => target_declarations(&assign.target, out),
            Stmt::AnnAssign(assign) => target_declarations(&assign.target, out),
            Stmt::For(for_stmt) => {
                target_declarations(&for_stmt.target, out);
                collect_declarations(&for_stmt.body, in_class, out, globals);
                collect_declarations(&for_stmt.orelse, in_class, out, globals);
            }
            Stmt::While(while_stmt) => {
                collect_declarations(&while_stmt.body, in_class, out, globals);
                collect_declarations(&while_stmt.orelse, in_class, out, globals);
            }
            Stmt::If(if_stmt) => {
                collect_declarations(&if_stmt.body, in_class, out, globals);
                for clause in &if_stmt.elif_else_clauses {
                    collect_declarations(&clause.body, in_class, out, globals);
                }
            }
            Stmt::With(with) => {
                for item in &with.items {
                    if let Some(vars) = &item.optional_vars {
                        target_declarations(vars, out);
                    }
                }
                collect_declarations(&with.body, in_class, out, globals);
            }
            Stmt::Match(match_stmt) => {
                for case in &match_stmt.cases {
                    collect_declarations(&case.body, in_class, out, globals);
                }
            }
            Stmt::Try(try_stmt) => {
                collect_declarations(&try_stmt.body, in_class, out, globals);
                for handler in &try_stmt.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    if let Some(name) = &handler.name {
                        out.push(Declaration {
                            name: name.to_string(),
                            range: name.range,
                            kind: FlowNodeKind::Variable,
                            import: None,
                        });
                    }
                    collect_declarations(&handler.body, in_class, out, globals);
                }
                collect_declarations(&try_stmt.orelse, in_class, out, globals);
                collect_declarations(&try_stmt.finalbody, in_class, out, globals);
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    let (name, range, module) = match &alias.asname {
                        Some(asname) => (asname.to_string(), asname.range, alias.name.to_string()),
                        // `import a.b.c` binds `a`.
                        None => {
                            let full = alias.name.as_str();
                            let first = full.split('.').next().unwrap_or(full).to_string();
                            (first.clone(), alias.name.range, first)
                        }
                    };
                    out.push(Declaration {
                        name,
                        range,
                        kind: FlowNodeKind::Import,
                        import: Some(ImportFact {
                            module,
                            name: "*".to_string(),
                            namespace: true,
                        }),
                    });
                }
            }
            Stmt::ImportFrom(import) => {
                let module = format!(
                    "{}{}",
                    ".".repeat(import.level as usize),
                    import.module.as_ref().map(|m| m.as_str()).unwrap_or("")
                );
                for alias in &import.names {
                    if alias.name.as_str() == "*" {
                        continue;
                    }
                    let (name, range) = match &alias.asname {
                        Some(asname) => (asname.to_string(), asname.range),
                        None => (alias.name.to_string(), alias.name.range),
                    };
                    out.push(Declaration {
                        name,
                        range,
                        kind: FlowNodeKind::Import,
                        import: Some(ImportFact {
                            module: module.clone(),
                            name: alias.name.to_string(),
                            namespace: false,
                        }),
                    });
                }
            }
            Stmt::Global(global) => {
                for name in &global.names {
                    globals.insert(name.to_string());
                }
            }
            // `nonlocal x` is resolved in the enclosing function: not declared here.
            Stmt::Nonlocal(nonlocal) => {
                for name in &nonlocal.names {
                    globals.insert(format!("\0nonlocal:{name}"));
                    out.retain(|d| d.name != name.as_str());
                }
            }
            _ => {}
        }
    }
    // Names declared nonlocal are not local, wherever the statement is.
    let nonlocals: Vec<String> = globals
        .iter()
        .filter_map(|g| g.strip_prefix("\0nonlocal:").map(str::to_string))
        .collect();
    if !nonlocals.is_empty() {
        out.retain(|d| !nonlocals.contains(&d.name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(flow: &FileFlow, name: &str) -> u32 {
        flow.nodes
            .iter()
            .position(|n| n.binding && n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no binding {name}")) as u32
    }

    #[test]
    fn assignments_reads_and_fstrings() {
        let source = "def view(request):\n    q = request.GET[\"q\"]\n    sql = f\"select {q}\"\n    cursor.execute(sql)\n";
        let flow = build_file_flow(source).unwrap();
        let q = binding(&flow, "q");
        let sql = binding(&flow, "sql");
        assert!(flow.in_edges[q as usize]
            .iter()
            .any(|(_, e)| *e == FlowEdgeKind::Assign));
        assert!(flow.in_edges[sql as usize]
            .iter()
            .any(|(_, e)| *e == FlowEdgeKind::Assign));
        assert_eq!(flow.calls.len(), 1);
        assert_eq!(&*flow.calls[0].callee_text, "cursor.execute");
    }

    #[test]
    fn methods_self_and_fields() {
        let source = "class Repo:\n    def __init__(self, db):\n        self.db = db\n    def find(self, id):\n        return self.db.get(id)\n";
        let flow = build_file_flow(source).unwrap();
        assert_eq!(flow.functions.len(), 2);
        let class = flow.functions[0].class_node.unwrap();
        let methods: Vec<&str> = flow.class_methods[&class]
            .iter()
            .map(|(n, _)| &**n)
            .collect();
        assert_eq!(methods, vec!["__init__", "find"]);
        let fields: Vec<&str> = flow.class_fields[&class]
            .iter()
            .map(|(n, _)| &**n)
            .collect();
        assert_eq!(fields, vec!["db"]);
    }

    #[test]
    fn list_mutations() {
        let source = "items = []\nitems.append(1)\nitems[0] = 2\nitems += [3]\nitems.count(1)\n";
        let flow = build_file_flow(source).unwrap();
        let kinds: Vec<MutationKind> = flow.mutations.iter().map(|m| m.kind).collect();
        assert_eq!(
            kinds,
            vec![
                MutationKind::MethodCall,
                MutationKind::PropertyWrite,
                MutationKind::Update
            ]
        );
    }
}
