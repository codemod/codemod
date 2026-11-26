//! Mechanical `fs` conversions for workflow upgrade.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use codemod_sandbox::sandbox::engine::target_files::is_fs;
use oxc::allocator::Allocator;
use oxc::ast::ast::{Argument, Expression, ImportDeclarationSpecifier, Statement};
use oxc::ast::AstKind;
use oxc::parser::Parser;
use oxc::semantic::{SemanticBuilder, SymbolId};
use oxc::span::{GetSpan, SourceType, Span};

enum Binding {
    Namespace,
    Named(String),
}

/// Rewrite only when all references are direct calls with matching text semantics.
pub(super) fn rewrite(path: &Path, text: &str) -> Option<String> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if !parsed.errors.is_empty() {
        return None;
    }
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let scoping = semantic.scoping();
    let mut imports = Vec::new();
    let mut bindings = HashMap::<SymbolId, Binding>::new();
    for statement in &parsed.program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if !is_fs(&import.source.value) {
            continue;
        }
        if import.import_kind.is_type() {
            return None;
        }
        for specifier in import.specifiers.as_ref()?.iter() {
            match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(specifier)
                    if !specifier.import_kind.is_type() =>
                {
                    bindings.insert(
                        specifier.local.symbol_id(),
                        Binding::Named(specifier.imported.name().to_string()),
                    );
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                    bindings.insert(specifier.local.symbol_id(), Binding::Namespace);
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                    bindings.insert(specifier.local.symbol_id(), Binding::Namespace);
                }
                _ => return None,
            }
        }
        imports.push(import.span);
    }
    if bindings.is_empty() {
        return None;
    }
    let binding = |id: &oxc::ast::ast::IdentifierReference<'_>| {
        let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
        bindings.get(&symbol)
    };
    let mut rewritten = HashSet::new();
    let mut edits: Vec<(Span, String)> = Vec::new();
    let alias = (0..)
        .map(|i| format!("__codemodFiles{i}"))
        .find(|name| !text.contains(name))?;
    for node in semantic.nodes().iter() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        if call.optional || call.type_arguments.is_some() {
            continue;
        }
        let (id, name) = match &call.callee {
            Expression::Identifier(id) => match binding(id) {
                Some(Binding::Named(name)) => (id.as_ref(), name.as_str()),
                _ => continue,
            },
            Expression::StaticMemberExpression(member) if !member.optional => {
                let Expression::Identifier(id) = &member.object else {
                    continue;
                };
                if !matches!(binding(id), Some(Binding::Namespace)) {
                    continue;
                }
                (id.as_ref(), member.property.name.as_str())
            }
            _ => continue,
        };
        let awaited = matches!(
            semantic.nodes().parent_kind(node.id()),
            AstKind::AwaitExpression(_)
        );
        let utf8 = |arg: &Argument<'_>| matches!(arg, Argument::StringLiteral(format) if matches!(format.value.as_str(), "utf8" | "utf-8"));
        let text_arg = |arg: &Argument<'_>| {
            matches!(
                arg,
                Argument::StringLiteral(_) | Argument::TemplateLiteral(_)
            )
        };
        let (api, keep) = match (name, call.arguments.as_slice()) {
            ("existsSync", [_]) => ("exists", 1),
            ("readFile" | "readFileSync", [_, format])
                if utf8(format) && (name == "readFileSync" || awaited) =>
            {
                ("read", 1)
            }
            ("writeFile" | "writeFileSync", [_, text, rest @ ..])
                if text_arg(text)
                    && (name == "writeFileSync" || awaited)
                    && (rest.is_empty() || matches!(rest, [format] if utf8(format))) =>
            {
                ("create", 2)
            }
            _ => return None,
        };
        if call
            .arguments
            .iter()
            .any(|argument| matches!(argument, Argument::SpreadElement(_)))
        {
            return None;
        }
        rewritten.insert(id.span.start);
        edits.push((call.callee.span(), format!("{alias}.{api}")));
        if keep < call.arguments.len() {
            edits.push((
                Span::new(
                    call.arguments[keep - 1].span().end,
                    call.arguments.last()?.span().end,
                ),
                String::new(),
            ));
        }
    }
    for node in semantic.nodes().iter() {
        if let AstKind::IdentifierReference(id) = node.kind() {
            if binding(id).is_some() && !rewritten.contains(&id.span.start) {
                return None;
            }
        }
    }
    for (i, span) in imports.into_iter().enumerate() {
        let replacement = if i == 0 {
            format!("import {{ files as {alias} }} from \"codemod/files\";")
        } else {
            String::new()
        };
        edits.push((span, replacement));
    }
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut result = text.to_owned();
    for (span, replacement) in edits {
        result.replace_range(span.start as usize..span.end as usize, &replacement);
    }
    Some(result)
}
