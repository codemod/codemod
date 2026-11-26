//! What one file says that entry points are read from: its bindings by
//! scope, its definitions with their decorators and bases, its statement
//! calls, and the calls its `__main__` guard makes. Expressions are kept as
//! [`Ex`], a small tree that outlives the syntax tree.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;
use std::collections::HashMap;

/// An expression, as far as entry points need it.
#[derive(Debug, Clone, PartialEq)]
pub enum Ex {
    Str(String),
    /// `a.b.c`.
    Ref(Vec<String>),
    /// `.name` on something that is not a dotted name.
    Attr(Box<Ex>, String),
    Call {
        func: Box<Ex>,
        args: Vec<Ex>,
        kwargs: Vec<(String, Ex)>,
    },
    /// A list or a tuple.
    List(Vec<Ex>),
    Dict(Vec<(Ex, Ex)>),
    /// `a + b`.
    Add(Box<Ex>, Box<Ex>),
    /// Anything else, as written.
    Other(String),
}

impl Ex {
    pub fn kwarg(&self, name: &str) -> Option<&Ex> {
        match self {
            Ex::Call { kwargs, .. } => kwargs.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn arg(&self, index: usize) -> Option<&Ex> {
        match self {
            Ex::Call { args, .. } => args.get(index),
            _ => None,
        }
    }

    /// The `index`th positional argument, or the keyword `name`.
    pub fn param(&self, index: usize, name: &str) -> Option<&Ex> {
        self.arg(index).or_else(|| self.kwarg(name))
    }

    /// The called expression of a call; the expression itself otherwise
    /// (`@shared_task` and `@shared_task()` decorate alike).
    pub fn callee(&self) -> &Ex {
        match self {
            Ex::Call { func, .. } => func,
            other => other,
        }
    }

    /// The text a label shows for it.
    pub fn text(&self) -> String {
        match self {
            Ex::Str(s) => format!("{s:?}"),
            Ex::Ref(parts) => parts.join("."),
            Ex::Attr(value, name) => format!("{}.{name}", value.text()),
            Ex::Call { func, .. } => format!("{}(…)", func.text()),
            Ex::List(_) | Ex::Dict(_) | Ex::Add(..) => "…".to_string(),
            Ex::Other(text) => text.clone(),
        }
    }

    /// Replace the name `name` with `value` (`x = x + [...]` keeps the old `x`).
    fn substitute(self, name: &str, value: &Ex) -> Ex {
        match self {
            Ex::Ref(parts) if parts.len() == 1 && parts[0] == name => value.clone(),
            Ex::Add(a, b) => Ex::Add(
                Box::new(a.substitute(name, value)),
                Box::new(b.substitute(name, value)),
            ),
            Ex::List(items) => Ex::List(
                items
                    .into_iter()
                    .map(|i| i.substitute(name, value))
                    .collect(),
            ),
            other => other,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Binding {
    /// `import a.b` (binds `a`, name None), `from m import n` (name `n`).
    Import {
        module: String,
        name: Option<String>,
    },
    Value(Ex),
    /// Index into [`FileSummary::defs`].
    Def(usize),
}

#[derive(Debug, Default)]
pub struct Scope {
    pub parent: Option<usize>,
    pub is_class: bool,
    pub names: HashMap<String, Binding>,
}

#[derive(Debug)]
pub struct Def {
    pub name: String,
    /// `Class.method`, `outer.inner`.
    pub qualname: String,
    /// Where the name starts: how a function is found again.
    pub name_start: u32,
    pub is_class: bool,
    /// The scope the definition is in (its decorators and bases resolve there).
    pub scope: usize,
    /// The scope of its body.
    pub body: usize,
    pub decorators: Vec<Ex>,
    pub bases: Vec<Ex>,
    /// A class's methods, by name.
    pub methods: Vec<usize>,
}

/// A call that may name a dependency (`Depends(fn)`), and what uses it.
#[derive(Debug)]
pub struct MarkedCall {
    pub scope: usize,
    pub call: Ex,
    /// The function or module-level name the call appears in.
    pub context: String,
}

#[derive(Debug, Default)]
pub struct FileSummary {
    /// Scope 0 is the module.
    pub scopes: Vec<Scope>,
    pub defs: Vec<Def>,
    /// Calls made as statements, by scope: `router.register(...)`.
    pub statement_calls: Vec<(usize, Ex)>,
    /// What `if __name__ == "__main__":` calls.
    pub main_guard_calls: Vec<Ex>,
    /// What the module's own statements call (outside definitions).
    pub module_calls: Vec<Ex>,
    /// Calls of names in [`MARKED_CALLS`] anywhere.
    pub marked_calls: Vec<MarkedCall>,
}

/// Callee names (last part) whose calls anywhere in a file are kept.
const MARKED_CALLS: &[&str] = &["Depends", "Security"];

impl FileSummary {
    pub fn parse(source: &str) -> Option<FileSummary> {
        let parsed = ruff_python_parser::parse_module(source).ok()?;
        let mut builder = Builder {
            source,
            out: FileSummary {
                scopes: vec![Scope::default()],
                ..Default::default()
            },
            class: None,
            qualname: Vec::new(),
            context: None,
        };
        builder.body(&parsed.syntax().body, 0, true);
        Some(builder.out)
    }
}

struct Builder<'s> {
    source: &'s str,
    out: FileSummary,
    class: Option<usize>,
    qualname: Vec<String>,
    /// The module-level name being assigned, for marked calls.
    context: Option<String>,
}

impl Builder<'_> {
    fn text(&self, node: &impl Ranged) -> String {
        self.source[node.range()].to_string()
    }

    fn ex(&self, expr: &Expr) -> Ex {
        match expr {
            Expr::StringLiteral(s) => Ex::Str(s.value.to_str().to_string()),
            Expr::Name(name) => Ex::Ref(vec![name.id.to_string()]),
            Expr::Attribute(attr) => match self.ex(&attr.value) {
                Ex::Ref(mut parts) => {
                    parts.push(attr.attr.to_string());
                    Ex::Ref(parts)
                }
                value => Ex::Attr(Box::new(value), attr.attr.to_string()),
            },
            Expr::Call(call) => Ex::Call {
                func: Box::new(self.ex(&call.func)),
                args: call
                    .arguments
                    .args
                    .iter()
                    .filter(|a| !a.is_starred_expr())
                    .map(|a| self.ex(a))
                    .collect(),
                kwargs: call
                    .arguments
                    .keywords
                    .iter()
                    .filter_map(|k| Some((k.arg.as_ref()?.to_string(), self.ex(&k.value))))
                    .collect(),
            },
            Expr::List(list) => Ex::List(list.elts.iter().map(|e| self.ex(e)).collect()),
            Expr::Tuple(tuple) => Ex::List(tuple.elts.iter().map(|e| self.ex(e)).collect()),
            Expr::Dict(dict) => Ex::Dict(
                dict.items
                    .iter()
                    .filter_map(|item| Some((self.ex(item.key.as_ref()?), self.ex(&item.value))))
                    .collect(),
            ),
            Expr::BinOp(bin) if bin.op == ast::Operator::Add => {
                Ex::Add(Box::new(self.ex(&bin.left)), Box::new(self.ex(&bin.right)))
            }
            other => Ex::Other(self.text(other)),
        }
    }

    fn bind(&mut self, scope: usize, name: String, binding: Binding) {
        let binding = match (binding, self.out.scopes[scope].names.get(&name)) {
            (Binding::Value(value), Some(Binding::Value(old))) => {
                Binding::Value(value.substitute(&name, old))
            }
            (binding, _) => binding,
        };
        self.out.scopes[scope].names.insert(name, binding);
    }

    fn body(&mut self, body: &[Stmt], scope: usize, module_level: bool) {
        for stmt in body {
            self.stmt(stmt, scope, module_level);
        }
    }

    fn stmt(&mut self, stmt: &Stmt, scope: usize, module_level: bool) {
        match stmt {
            Stmt::FunctionDef(function) => self.function(function, scope),
            Stmt::ClassDef(class) => self.class(class, scope),
            Stmt::Import(import) => {
                for alias in &import.names {
                    let full = alias.name.to_string();
                    let (name, module) = match &alias.asname {
                        Some(asname) => (asname.to_string(), full),
                        None => {
                            let first = full.split('.').next().unwrap_or(&full).to_string();
                            (first.clone(), first)
                        }
                    };
                    self.bind(scope, name, Binding::Import { module, name: None });
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
                    let local = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                    self.bind(
                        scope,
                        local,
                        Binding::Import {
                            module: module.clone(),
                            name: Some(alias.name.to_string()),
                        },
                    );
                }
            }
            Stmt::Assign(assign) => {
                let target = match assign.targets.as_slice() {
                    [Expr::Name(name)] => Some(name.id.to_string()),
                    _ => None,
                };
                self.marked_in(&assign.value, scope, target.clone());
                if let Some(target) = target {
                    let value = self.ex(&assign.value);
                    self.bind(scope, target, Binding::Value(value));
                }
                self.calls_in(&assign.value, module_level);
            }
            Stmt::AnnAssign(assign) => {
                let target = match &*assign.target {
                    Expr::Name(name) => Some(name.id.to_string()),
                    _ => None,
                };
                self.marked_in(&assign.annotation, scope, target.clone());
                if let Some(value) = &assign.value {
                    self.marked_in(value, scope, target.clone());
                    if let Some(target) = target {
                        let value = self.ex(value);
                        self.bind(scope, target, Binding::Value(value));
                    }
                    self.calls_in(value, module_level);
                }
            }
            Stmt::AugAssign(assign) => {
                self.marked_in(&assign.value, scope, None);
                if let (Expr::Name(name), ast::Operator::Add) = (&*assign.target, assign.op) {
                    let name = name.id.to_string();
                    let old = match self.out.scopes[scope].names.get(&name) {
                        Some(Binding::Value(old)) => old.clone(),
                        _ => Ex::Ref(vec![name.clone()]),
                    };
                    let value = Ex::Add(Box::new(old), Box::new(self.ex(&assign.value)));
                    self.out.scopes[scope]
                        .names
                        .insert(name, Binding::Value(value));
                }
                self.calls_in(&assign.value, module_level);
            }
            Stmt::Expr(expr) => {
                self.marked_in(&expr.value, scope, None);
                if let Expr::Call(_) = &*expr.value {
                    let call = self.ex(&expr.value);
                    self.out.statement_calls.push((scope, call));
                }
                self.calls_in(&expr.value, module_level);
            }
            Stmt::If(if_stmt) => {
                self.marked_in(&if_stmt.test, scope, None);
                if module_level && is_main_guard(&if_stmt.test) {
                    let mut calls = CallCollector::default();
                    for stmt in &if_stmt.body {
                        calls.visit_stmt(stmt);
                    }
                    let found: Vec<Ex> = calls.calls.iter().map(|c| self.ex(c)).collect();
                    self.out.main_guard_calls.extend(found);
                }
                self.body(&if_stmt.body, scope, module_level);
                for clause in &if_stmt.elif_else_clauses {
                    self.body(&clause.body, scope, module_level);
                }
            }
            Stmt::For(for_stmt) => {
                self.body(&for_stmt.body, scope, module_level);
                self.body(&for_stmt.orelse, scope, module_level);
            }
            Stmt::While(while_stmt) => {
                self.body(&while_stmt.body, scope, module_level);
                self.body(&while_stmt.orelse, scope, module_level);
            }
            Stmt::With(with) => {
                for item in &with.items {
                    self.calls_in(&item.context_expr, module_level);
                }
                self.body(&with.body, scope, module_level);
            }
            Stmt::Try(try_stmt) => {
                self.body(&try_stmt.body, scope, module_level);
                for handler in &try_stmt.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    self.body(&handler.body, scope, module_level);
                }
                self.body(&try_stmt.orelse, scope, module_level);
                self.body(&try_stmt.finalbody, scope, module_level);
            }
            Stmt::Return(ret) => {
                if let Some(value) = &ret.value {
                    self.marked_in(value, scope, None);
                }
            }
            _ => {}
        }
    }

    /// Module-level calls, for `__main__.py`.
    fn calls_in(&mut self, expr: &Expr, module_level: bool) {
        if !module_level {
            return;
        }
        let mut calls = CallCollector::default();
        calls.visit_expr(expr);
        let found: Vec<Ex> = calls.calls.iter().map(|c| self.ex(c)).collect();
        self.out.module_calls.extend(found);
    }

    fn marked_in(&mut self, expr: &Expr, scope: usize, target: Option<String>) {
        let mut marked = MarkedCollector::default();
        marked.visit_expr(expr);
        self.record_marked(marked.calls, scope, target);
    }

    fn record_marked(&mut self, calls: Vec<&ast::ExprCall>, scope: usize, target: Option<String>) {
        for call in calls {
            let context = self
                .context
                .clone()
                .or_else(|| target.clone())
                .unwrap_or_else(|| {
                    if self.qualname.is_empty() {
                        "<module>".to_string()
                    } else {
                        self.qualname.join(".")
                    }
                });
            let call = self.ex(&Expr::Call(call.clone()));
            self.out.marked_calls.push(MarkedCall {
                scope,
                call,
                context,
            });
        }
    }

    fn new_scope(&mut self, parent: usize, is_class: bool) -> usize {
        self.out.scopes.push(Scope {
            parent: Some(parent),
            is_class,
            names: HashMap::new(),
        });
        self.out.scopes.len() - 1
    }

    fn function(&mut self, function: &ast::StmtFunctionDef, scope: usize) {
        let name = function.name.to_string();
        let decorators: Vec<Ex> = function
            .decorator_list
            .iter()
            .map(|d| self.ex(&d.expression))
            .collect();
        self.qualname.push(name.clone());
        let qualname = self.qualname.join(".");
        let saved = self.context.replace(qualname.clone());
        // Decorator arguments, defaults and annotations:
        // `dependencies=[Depends(auth)]`, `user = Depends(current_user)`.
        let mut marked = MarkedCollector::default();
        for decorator in &function.decorator_list {
            marked.visit_expr(&decorator.expression);
        }
        marked.visit_parameters(&function.parameters);
        if let Some(returns) = &function.returns {
            marked.visit_expr(returns);
        }
        let bare = marked.bare_with_annotation(&function.parameters);
        self.record_marked(marked.calls, scope, None);
        // `common: Params = Depends()` depends on the annotation.
        for (call, annotation) in bare {
            if let Ex::Call { func, kwargs, .. } = self.ex(&Expr::Call(call.clone())) {
                self.out.marked_calls.push(MarkedCall {
                    scope,
                    call: Ex::Call {
                        func,
                        args: vec![self.ex(annotation)],
                        kwargs,
                    },
                    context: qualname.clone(),
                });
            }
        }
        let body = self.new_scope(scope, false);
        let index = self.out.defs.len();
        self.out.defs.push(Def {
            name: name.clone(),
            qualname,
            name_start: function.name.range.start().to_u32(),
            is_class: false,
            scope,
            body,
            decorators,
            bases: Vec::new(),
            methods: Vec::new(),
        });
        if let Some(class) = self.class {
            self.out.defs[class].methods.push(index);
        }
        self.bind(scope, name, Binding::Def(index));
        let saved_class = self.class.take();
        self.body(&function.body, body, false);
        self.class = saved_class;
        self.context = saved;
        self.qualname.pop();
    }

    fn class(&mut self, class: &ast::StmtClassDef, scope: usize) {
        let name = class.name.to_string();
        let decorators = class
            .decorator_list
            .iter()
            .map(|d| self.ex(&d.expression))
            .collect();
        let bases = class
            .arguments
            .as_ref()
            .map(|a| a.args.iter().map(|b| self.ex(b)).collect())
            .unwrap_or_default();
        self.qualname.push(name.clone());
        let body = self.new_scope(scope, true);
        let index = self.out.defs.len();
        self.out.defs.push(Def {
            name: name.clone(),
            qualname: self.qualname.join("."),
            name_start: class.name.range.start().to_u32(),
            is_class: true,
            scope,
            body,
            decorators,
            bases,
            methods: Vec::new(),
        });
        self.bind(scope, name, Binding::Def(index));
        let saved = self.class.replace(index);
        self.body(&class.body, body, false);
        self.class = saved;
        self.qualname.pop();
    }
}

/// `__name__ == "__main__"`, either way round.
fn is_main_guard(test: &Expr) -> bool {
    let Expr::Compare(compare) = test else {
        return false;
    };
    if compare.ops.as_ref() != [ast::CmpOp::Eq] {
        return false;
    }
    let is_name = |e: &Expr| matches!(e, Expr::Name(n) if n.id.as_str() == "__name__");
    let is_main = |e: &Expr| matches!(e, Expr::StringLiteral(s) if s.value.to_str() == "__main__");
    let right = &compare.comparators[0];
    (is_name(&compare.left) && is_main(right)) || (is_main(&compare.left) && is_name(right))
}

/// Calls in a subtree, outside nested definitions and lambdas: their
/// callees run when the subtree does.
#[derive(Default)]
struct CallCollector<'a> {
    calls: Vec<&'a Expr>,
}

impl<'a> Visitor<'a> for CallCollector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(_) => {}
            Expr::Call(call) => {
                self.calls.push(&call.func);
                visitor::walk_expr(self, expr);
            }
            _ => visitor::walk_expr(self, expr),
        }
    }
}

/// Calls of [`MARKED_CALLS`] names in an expression.
#[derive(Default)]
struct MarkedCollector<'a> {
    calls: Vec<&'a ast::ExprCall>,
}

impl<'a> MarkedCollector<'a> {
    /// Marked calls without arguments that are a parameter's default, with
    /// the parameter's annotation (taken out of `calls`).
    fn bare_with_annotation(
        &mut self,
        parameters: &'a ast::Parameters,
    ) -> Vec<(&'a ast::ExprCall, &'a Expr)> {
        let mut found = Vec::new();
        for parameter in parameters
            .posonlyargs
            .iter()
            .chain(&parameters.args)
            .chain(&parameters.kwonlyargs)
        {
            let (Some(Expr::Call(call)), Some(annotation)) = (
                parameter.default.as_deref(),
                parameter.parameter.annotation.as_deref(),
            ) else {
                continue;
            };
            if call.arguments.args.is_empty()
                && call.arguments.keywords.is_empty()
                && self.calls.iter().any(|c| std::ptr::eq(*c, call))
            {
                self.calls.retain(|c| !std::ptr::eq(*c, call));
                found.push((call, annotation));
            }
        }
        found
    }
}

impl<'a> Visitor<'a> for MarkedCollector<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            let last = match &*call.func {
                Expr::Name(name) => Some(name.id.as_str()),
                Expr::Attribute(attr) => Some(attr.attr.as_str()),
                _ => None,
            };
            if last.is_some_and(|name| MARKED_CALLS.contains(&name)) {
                self.calls.push(call);
            }
        }
        visitor::walk_expr(self, expr);
    }
}
