//! `selectFiles(selector).filter(cb).map(cb)`: which `.map` and `.filter`
//! calls are the selection's, so the build extracts their callbacks.
//!
//! The build cannot run the body, so it only accepts a selection it can see
//! by the shape of the code. A *selection expression* is
//!
//! - a call of the `selectFiles` imported from `codemod/workflow` (resolved
//!   by scope, so a renamed import works and a local `selectFiles` does not);
//! - `<selection>.filter(arg)`;
//! - a reference to a `const` whose initializer is a selection expression.
//!
//! `<selection>.map(arg)` and `<selection>.filter(arg)` are the selection's
//! calls; `arg` is extracted like a `forEachFile` callback. Any `.map` on
//! something else (an array, a step's `outputs`) is left alone.
//!
//! Every selection expression must be *consumed*: be the receiver of one of
//! those calls, or the initializer of such a `const`. Anything else (passing
//! one to a helper, `let`, a conditional, spreading, logging, returning it)
//! is a build error, so a selection whose callbacks the build would not
//! extract never reaches the runtime.

use std::collections::{HashMap, HashSet};

use oxc::ast::ast::{
    Argument, BindingPatternKind, CallExpression, Expression, ImportDeclarationSpecifier, Program,
    Statement, VariableDeclarationKind,
};
use oxc::ast::AstKind;
use oxc::semantic::{Semantic, SymbolId};
use oxc::span::{GetSpan, Span};

use crate::build::is_flow_module;

/// Local bindings of `selectFiles` from `codemod/workflow`.
pub(crate) fn select_symbols(program: &Program) -> HashSet<SymbolId> {
    let mut symbols = HashSet::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if import.import_kind.is_type() || !is_flow_module(&import.source.value) {
            continue;
        }
        for specifier in import.specifiers.iter().flatten() {
            if let ImportDeclarationSpecifier::ImportSpecifier(s) = specifier {
                if !s.import_kind.is_type() && s.imported.name() == "selectFiles" {
                    symbols.insert(s.local.symbol_id());
                }
            }
        }
    }
    symbols
}

/// A `.map(arg)` or `.filter(arg)` on a selection.
pub(crate) struct SelectionCall<'a> {
    pub method: &'static str,
    pub call: &'a CallExpression<'a>,
}

impl<'a> SelectionCall<'a> {
    pub fn argument(&self) -> Option<&'a Argument<'a>> {
        self.call.arguments.first()
    }
}

/// The selection's `.map` and `.filter` calls, in source order, or where a
/// selection is used in a way the build cannot follow, and why.
pub(crate) fn selection_calls<'a>(
    semantic: &Semantic<'a>,
    select: &HashSet<SymbolId>,
) -> Result<Vec<SelectionCall<'a>>, (u32, String)> {
    if select.is_empty() {
        return Ok(Vec::new());
    }
    let finder = Finder {
        semantic,
        select,
        consts: grow_consts(semantic, select),
    };
    let scoping = semantic.scoping();

    let mut calls = Vec::new();
    // Spans of selection expressions that are used as they must be.
    let mut consumed: HashSet<Span> = HashSet::new();
    // Better messages for some shapes, by the selection's span.
    let mut hints: HashMap<Span, String> = HashMap::new();
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::CallExpression(call) => {
                let Some((method, object)) = finder.method_call(call) else {
                    continue;
                };
                consumed.insert(object.span());
                if call.arguments.len() != 1 {
                    return Err((
                        call.span.start,
                        format!("selectFiles(...).{method} takes one callback"),
                    ));
                }
                if let Some(Argument::SpreadElement(spread)) = call.arguments.first() {
                    return Err((
                        spread.span.start,
                        format!("write selectFiles(...).{method}'s callback as is, not spread"),
                    ));
                }
                calls.push(SelectionCall { method, call });
            }
            AstKind::VariableDeclarator(declarator) => {
                let Some(init) = &declarator.init else {
                    continue;
                };
                let init = init.get_inner_expression();
                if !finder.is_selection(init) {
                    continue;
                }
                let simple = matches!(declarator.id.kind, BindingPatternKind::BindingIdentifier(_));
                if declarator.kind == VariableDeclarationKind::Const && simple {
                    consumed.insert(init.span());
                } else if simple {
                    hints.insert(
                        init.span(),
                        format!(
                            "bind selectFiles(...) with const, not {}: the build follows a const to its .map(...) to find the callbacks",
                            declarator.kind.as_str()
                        ),
                    );
                }
            }
            AstKind::ConditionalExpression(conditional) => {
                for branch in [&conditional.consequent, &conditional.alternate] {
                    let branch = branch.get_inner_expression();
                    if finder.is_selection(branch) {
                        hints.insert(branch.span(), CHOOSE_THE_SELECTOR.to_string());
                    }
                }
            }
            AstKind::LogicalExpression(logical) => {
                for side in [&logical.left, &logical.right] {
                    let side = side.get_inner_expression();
                    if finder.is_selection(side) {
                        hints.insert(side.span(), CHOOSE_THE_SELECTOR.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    // Every selection expression must be consumed.
    for node in semantic.nodes().iter() {
        let span = match node.kind() {
            AstKind::CallExpression(call) => {
                if !finder.is_selection_call(call) {
                    continue;
                }
                call.span
            }
            AstKind::IdentifierReference(ident) => {
                let reference = scoping.get_reference(ident.reference_id());
                if !reference.is_value() {
                    continue;
                }
                match reference.symbol_id() {
                    Some(symbol) if finder.consts.contains(&symbol) => {}
                    _ => continue,
                }
                ident.span
            }
            _ => continue,
        };
        if consumed.contains(&span) {
            continue;
        }
        let message = hints.remove(&span).unwrap_or_else(|| UNFOLLOWED.to_string());
        return Err((span.start, message));
    }
    Ok(calls)
}

const CHOOSE_THE_SELECTOR: &str = "choose the selector, not the selection: selectFiles(condition ? a : b).map(...), so the build sees which .map(...) is the selection's";

const UNFOLLOWED: &str = "use selectFiles(...) as the start of a chain, selectFiles(selector).filter(...).map(...), or bind it to a const that is: the build finds the callbacks by that shape. A selection is not a list of files, and cannot be passed to helpers, logged, spread or looped over; to share one, share the selector and call selectFiles where it is mapped";

struct Finder<'s, 'a> {
    semantic: &'s Semantic<'a>,
    select: &'s HashSet<SymbolId>,
    /// `const`s bound to a selection.
    consts: HashSet<SymbolId>,
}

impl<'a> Finder<'_, 'a> {
    /// `selectFiles(...)`, called by its import.
    fn is_select_call(&self, call: &CallExpression) -> bool {
        let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
            return false;
        };
        self.semantic
            .scoping()
            .get_reference(callee.reference_id())
            .symbol_id()
            .is_some_and(|symbol| self.select.contains(&symbol))
    }

    /// `.map(...)` or `.filter(...)` on a selection: which, and the selection.
    fn method_call<'c>(
        &self,
        call: &'c CallExpression<'a>,
    ) -> Option<(&'static str, &'c Expression<'a>)> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        if member.optional {
            return None;
        }
        let method = match member.property.name.as_str() {
            "map" => "map",
            "filter" => "filter",
            _ => return None,
        };
        let object = member.object.get_inner_expression();
        self.is_selection(object).then_some((method, object))
    }

    /// A call that makes a selection: `selectFiles(...)`, or `.filter(...)` on one.
    fn is_selection_call(&self, call: &CallExpression<'a>) -> bool {
        self.is_select_call(call)
            || matches!(self.method_call(call), Some(("filter", _)))
    }

    fn is_selection(&self, expression: &Expression<'a>) -> bool {
        match expression.get_inner_expression() {
            Expression::CallExpression(call) => self.is_selection_call(call),
            Expression::Identifier(ident) => self
                .semantic
                .scoping()
                .get_reference(ident.reference_id())
                .symbol_id()
                .is_some_and(|symbol| self.consts.contains(&symbol)),
            _ => false,
        }
    }
}

/// `const`s whose initializer is a selection expression, to a fixed point
/// (`const all = selectFiles(...); const some = all.filter(...)`).
fn grow_consts(semantic: &Semantic<'_>, select: &HashSet<SymbolId>) -> HashSet<SymbolId> {
    let mut finder = Finder {
        semantic,
        select,
        consts: HashSet::new(),
    };
    loop {
        let mut grew = false;
        for node in semantic.nodes().iter() {
            let AstKind::VariableDeclarator(declarator) = node.kind() else {
                continue;
            };
            if declarator.kind != VariableDeclarationKind::Const {
                continue;
            }
            let BindingPatternKind::BindingIdentifier(id) = &declarator.id.kind else {
                continue;
            };
            let symbol = id.symbol_id();
            if finder.consts.contains(&symbol) {
                continue;
            }
            if declarator
                .init
                .as_ref()
                .is_some_and(|init| finder.is_selection(init))
            {
                finder.consts.insert(symbol);
                grew = true;
            }
        }
        if !grew {
            return finder.consts;
        }
    }
}
