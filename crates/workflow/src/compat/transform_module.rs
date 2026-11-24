//! What `codemod workflow upgrade` reads from a jssg transform module to
//! call it from a plain `forEachFile` callback,
//! `(file) => { const out = transform(parse(file)); ... }`, in place of the
//! compatibility adapter that runs it as workflow.yaml's js-ast-grep step did.
//!
//! The plain callback does what the adapter does for a transform that
//! returns its new text (or `{ edit, data }`), reads `options`, and has a
//! `getSelector`. Two things only the adapter does: `root.rename()`, which
//! in a callback is `file.rename()`, and `jssgTransform` on other files,
//! which needs the adapter's host. A module that uses either, or whose
//! default export is not a function the upgrade can read, keeps the adapter.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Declaration, ExportDefaultDeclarationKind, Expression, FormalParameters, ModuleExportName,
    Program, PropertyKey, Statement, TSType, TSTypeAnnotation,
};
use oxc::ast::AstKind;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;

/// How to call a transform module's default export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransformModule {
    /// It takes the jssg options (`params`, `language`, `targetDir`, ...):
    /// it declares a second parameter, or its type (`Transform<T>`) does.
    pub takes_options: bool,
    /// Its result may be a promise.
    pub is_async: bool,
    /// It may return `{ edit, data }` rather than the new text.
    pub returns_result: bool,
    /// It exports `getSelector`: files without a match are skipped, and the
    /// matches are `options.matches`.
    pub get_selector: bool,
}

/// What a module needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Analysis {
    Plain(TransformModule),
    /// The compatibility adapter, and why.
    Adapter(String),
}

/// Read the module at `path`, a jssg transform.
pub(crate) fn analyze(path: &Path) -> Analysis {
    let Ok(source) = std::fs::read_to_string(path) else {
        return Analysis::Adapter(format!("{} could not be read", path.display()));
    };
    if let Some(reason) = adapter_only(path) {
        return Analysis::Adapter(reason);
    }
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    if parsed.panicked || !parsed.errors.is_empty() {
        return Analysis::Adapter("it could not be parsed".into());
    }
    match default_export(&parsed.program, &source) {
        Some(mut module) => {
            module.returns_result |= has_result_keys(&parsed.program);
            module.get_selector = exports(&parsed.program, "getSelector");
            Analysis::Plain(module)
        }
        None => {
            Analysis::Adapter("its default export is not a function the upgrade can read".into())
        }
    }
}

/// Why the module, or a file of the package it imports, needs the adapter.
fn adapter_only(path: &Path) -> Option<String> {
    let mut seen = BTreeSet::new();
    let mut queue = vec![path.to_path_buf()];
    while let Some(file) = queue.pop() {
        if seen.len() > 200 || !seen.insert(file.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if text.contains("jssgTransform") {
            return Some(format!("{name} calls jssgTransform"));
        }
        if calls_rename(&text) {
            return Some(format!("{name} calls root.rename()"));
        }
        let dir = file.parent().unwrap_or(Path::new("."));
        queue.extend(
            relative_imports(&text)
                .iter()
                .filter_map(|specifier| resolve(dir, specifier)),
        );
    }
    None
}

/// `x.rename(`, other than Node.js's `fs.rename`.
fn calls_rename(text: &str) -> bool {
    text.match_indices(".rename(").any(|(at, _)| {
        let before = &text[..at];
        let object = before
            .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .next()
            .unwrap_or("");
        !matches!(object, "fs" | "promises" | "fsp" | "fsPromises")
    })
}

/// Relative specifiers in `import`, `export ... from` and `require`.
fn relative_imports(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for keyword in ["from", "import", "require("] {
        for (at, _) in text.match_indices(keyword) {
            let rest = text[at + keyword.len()..].trim_start_matches([' ', '(', '\t']);
            let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let Some(end) = rest[1..].find(quote) else {
                continue;
            };
            let specifier = &rest[1..1 + end];
            if specifier.starts_with("./") || specifier.starts_with("../") {
                found.push(specifier.to_string());
            }
        }
    }
    found
}

fn resolve(dir: &Path, specifier: &str) -> Option<PathBuf> {
    let base = dir.join(specifier);
    if base.is_file() {
        return Some(base);
    }
    // `./x.js` written for `./x.ts`, or no extension at all.
    let stem = match base.extension().and_then(|e| e.to_str()) {
        Some("js" | "mjs" | "cjs" | "jsx") => base.with_extension(""),
        _ => base.clone(),
    };
    let extensions = ["ts", "tsx", "mts", "cts", "js", "mjs", "cjs", "jsx"];
    extensions
        .iter()
        .map(|ext| PathBuf::from(format!("{}.{ext}", stem.display())))
        .chain(
            extensions
                .iter()
                .map(|ext| base.join(format!("index.{ext}"))),
        )
        .find(|candidate| candidate.is_file())
}

/// The default export, when it is a function, or a variable holding one.
fn default_export(program: &Program, source: &str) -> Option<TransformModule> {
    for statement in &program.body {
        match statement {
            Statement::ExportDefaultDeclaration(export) => {
                return match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        Some(from_function(
                            &function.params,
                            function.r#async,
                            function.return_type.as_deref(),
                            source,
                        ))
                    }
                    ExportDefaultDeclarationKind::Identifier(identifier) => {
                        binding(program, &identifier.name, source, 0)
                    }
                    other => other
                        .as_expression()
                        .and_then(|expression| from_expression(expression, source)),
                };
            }
            Statement::ExportNamedDeclaration(export) if export.source.is_none() => {
                for specifier in &export.specifiers {
                    if export_name(&specifier.exported) == "default" {
                        return binding(program, &export_name(&specifier.local), source, 0);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::IdentifierName(name) => name.name.to_string(),
        ModuleExportName::IdentifierReference(name) => name.name.to_string(),
        ModuleExportName::StringLiteral(name) => name.value.to_string(),
    }
}

/// The top-level function or variable `name`, following aliases
/// (`var codemod_default = transform`, as bundlers write).
fn binding(program: &Program, name: &str, source: &str, depth: usize) -> Option<TransformModule> {
    if depth > 8 {
        return None;
    }
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportNamedDeclaration(export) => export.declaration.as_ref(),
            other => other.as_declaration(),
        };
        match declaration {
            Some(Declaration::FunctionDeclaration(function))
                if function.id.as_ref().is_some_and(|id| id.name == name)
                    && function.body.is_some() =>
            {
                return Some(from_function(
                    &function.params,
                    function.r#async,
                    function.return_type.as_deref(),
                    source,
                ));
            }
            Some(Declaration::VariableDeclaration(variables)) => {
                for declarator in &variables.declarations {
                    if declarator
                        .id
                        .get_identifier_name()
                        .is_none_or(|id| id != name)
                    {
                        continue;
                    }
                    // `const transform: Transform<Js> = ...`: the type says how
                    // it is called.
                    if let Some(annotation) = &declarator.id.type_annotation {
                        if let TSType::TSTypeReference(_) = &annotation.type_annotation {
                            let text = annotation.span.source_text(source);
                            return Some(TransformModule {
                                takes_options: true,
                                is_async: true,
                                returns_result: text.contains("TransformResult"),
                                get_selector: false,
                            });
                        }
                    }
                    return match declarator
                        .init
                        .as_ref()
                        .map(Expression::without_parentheses)
                    {
                        Some(Expression::Identifier(alias)) => {
                            binding(program, &alias.name, source, depth + 1)
                        }
                        init => init.and_then(|init| from_expression(init, source)),
                    };
                }
            }
            _ => {}
        }
    }
    None
}

fn from_expression(expression: &Expression, source: &str) -> Option<TransformModule> {
    match expression.without_parentheses() {
        Expression::ArrowFunctionExpression(arrow) => Some(from_function(
            &arrow.params,
            arrow.r#async,
            arrow.return_type.as_deref(),
            source,
        )),
        Expression::FunctionExpression(function) => Some(from_function(
            &function.params,
            function.r#async,
            function.return_type.as_deref(),
            source,
        )),
        _ => None,
    }
}

fn from_function(
    params: &FormalParameters,
    is_async: bool,
    return_type: Option<&TSTypeAnnotation>,
    source: &str,
) -> TransformModule {
    let returns = return_type.map(|annotation| annotation.span.source_text(source));
    TransformModule {
        takes_options: params.items.len() >= 2 || params.rest.is_some(),
        // Without a return type, a function that is not async may still
        // return a promise it got elsewhere.
        is_async: is_async || returns.is_none_or(|text| text.contains("Promise")),
        returns_result: returns
            .is_some_and(|text| text.contains("TransformResult") || text.contains("edit")),
        get_selector: false,
    }
}

/// Whether the module exports `name`.
fn exports(program: &Program, name: &str) -> bool {
    program.body.iter().any(|statement| {
        let Statement::ExportNamedDeclaration(export) = statement else {
            return false;
        };
        if export
            .specifiers
            .iter()
            .any(|specifier| export_name(&specifier.exported) == name)
        {
            return true;
        }
        match &export.declaration {
            Some(Declaration::FunctionDeclaration(function)) => {
                function.id.as_ref().is_some_and(|id| id.name == name)
            }
            Some(Declaration::VariableDeclaration(variables)) => {
                variables.declarations.iter().any(|declarator| {
                    declarator
                        .id
                        .get_identifier_name()
                        .is_some_and(|id| id == name)
                })
            }
            _ => false,
        }
    })
}

/// Whether an object literal has an `edit` or `data` key: a `{ edit, data }` result.
fn has_result_keys(program: &Program) -> bool {
    let semantic = SemanticBuilder::new().build(program).semantic;
    let found = semantic.nodes().iter().any(|node| {
        matches!(
            node.kind(),
            AstKind::ObjectProperty(property)
                if matches!(&property.key, PropertyKey::StaticIdentifier(key) if key.name == "edit" || key.name == "data")
        )
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzed(name: &str, text: &str) -> Analysis {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        analyze(&path)
    }

    #[test]
    fn a_typed_function_returning_text_is_called_plainly() {
        assert_eq!(
            analyzed(
                "t.ts",
                "import type { SgRoot } from '@codemod.com/jssg-types/main';\nexport default function transform(root: SgRoot): string | null { return null; }\n"
            ),
            Analysis::Plain(TransformModule {
                takes_options: false,
                is_async: false,
                returns_result: false,
                get_selector: false,
            })
        );
    }

    #[test]
    fn a_transform_typed_variable_takes_options() {
        assert_eq!(
            analyzed(
                "t.ts",
                "const transform: Transform<Json> = async (root) => null;\nexport default transform;\n"
            ),
            Analysis::Plain(TransformModule {
                takes_options: true,
                is_async: true,
                returns_result: false,
                get_selector: false,
            })
        );
    }

    #[test]
    fn results_and_selectors_are_found() {
        let Analysis::Plain(module) = analyzed(
            "t.ts",
            "export function getSelector() { return { rule: { pattern: 'x' } }; }\nexport default async function (root, options) { return { edit: null, data: 1 }; }\n",
        ) else {
            panic!("adapter");
        };
        assert!(
            module.takes_options && module.is_async && module.returns_result && module.get_selector
        );
    }

    #[test]
    fn rename_and_jssg_transform_keep_the_adapter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("helper.ts"),
            "export const move = (root) => root.rename('x.css');\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("t.ts"),
            "import { move } from './helper.js';\nexport default function transform(root) { move(root); return null; }\n",
        )
        .unwrap();
        assert_eq!(
            analyze(&dir.path().join("t.ts")),
            Analysis::Adapter("helper.ts calls root.rename()".into())
        );
        assert!(matches!(
            analyzed("t.ts", "import { jssgTransform } from 'codemod:ast-grep';\nexport default function t(root) { return null; }\n"),
            Analysis::Adapter(_)
        ));
        assert!(matches!(
            analyzed("t.ts", "import fs from 'node:fs';\nexport default function t(root) { fs.rename('a', 'b', () => {}); return null; }\n"),
            Analysis::Plain(_)
        ));
        assert!(matches!(
            analyzed("t.ts", "export default class {}\n"),
            Analysis::Adapter(_)
        ));
    }

    #[test]
    fn a_bundled_default_export_is_followed_to_its_function() {
        assert_eq!(
            analyzed(
                "t.ts",
                "async function transform(root) { return null; }\nvar codemod_default = transform;\nexport { codemod_default as default };\n"
            ),
            Analysis::Plain(TransformModule {
                takes_options: false,
                is_async: true,
                returns_result: false,
                get_selector: false,
            })
        );
    }
}
