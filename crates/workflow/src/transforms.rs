//! Transforms and rule files a workflow names, read without running it.
//!
//! `jssg` and the shard function in `shard({ method: { function } })` take a
//! transform imported from a file of the package:
//!
//! ```ts
//! import codemod from "./scripts/codemod.ts";
//! await step(forEachFile(jssg(codemod)));
//! ```
//!
//! The import is not loaded: the transform runs in the engine, file by file.
//! The build drops the import and passes the engine a reference to the file
//! (`__transform("./scripts/codemod.ts")`), and publishing includes the file.
//! `astGrep` rule files are data: their path is written in the call.
//!
//! Anything the build cannot read (a path in a string, a path computed at
//! run time, a transform used for something else) is a build error, so a
//! published package never misses a file it needs.

use std::collections::HashMap;
use std::path::Path;

use oxc::ast::ast::{
    Argument, CallExpression, Expression, ImportDeclarationSpecifier, ObjectExpression,
    ObjectPropertyKind, Program, PropertyKey, Statement,
};
use oxc::ast::AstKind;
use oxc::semantic::{Semantic, SymbolId};
use oxc::span::{GetSpan, Span};

use crate::build::{error_at, is_flow_module, BuildError};

/// Local name the build gives `__transform` from `codemod/workflow`.
pub(crate) const TRANSFORM_REFERENCE: &str = "__flowTransform";

/// Extensions of files the engine runs as transforms.
const SCRIPT_EXTENSIONS: &[&str] = &["ts", "mts", "cts", "tsx", "js", "mjs", "cjs", "jsx"];

/// A default import of a file that the workflow gives to `jssg` or `shard`.
#[derive(Debug, Clone)]
pub(crate) struct TransformImport {
    /// The import as written (`./scripts/codemod.ts`), relative to the workflow.
    pub specifier: String,
    /// What to cut from the declaration to drop only the default import:
    /// the whole declaration when it imports nothing else.
    pub cut: Span,
}

/// What the analysis found.
#[derive(Debug, Default)]
pub(crate) struct Transforms {
    /// Imported transforms, by their binding.
    pub imports: HashMap<SymbolId, TransformImport>,
    /// Text to put in place of each argument that names a transform.
    pub replacements: Vec<(Span, String)>,
    /// Files the workflow names, in source order: rule files given to
    /// `astGrep`, and imported transforms and shard functions.
    pub files: Vec<String>,
    /// The specifiers of imported transforms, as written.
    pub specifiers: Vec<String>,
}

#[derive(Debug, Clone)]
enum Imported {
    Default,
    Named(String),
    Namespace,
}

#[derive(Debug, Clone)]
struct ImportedBinding {
    source: String,
    imported: Imported,
    declaration: Span,
    /// The specifier's span, and where the next specifier (or the closing
    /// part of the declaration) begins.
    specifier: Span,
    alone: bool,
}

/// What a transform argument is, for messages.
#[derive(Clone, Copy)]
enum Role {
    Jssg,
    ShardFunction,
    ShardJssg,
}

impl Role {
    fn call(self) -> &'static str {
        match self {
            Role::Jssg => "jssg",
            Role::ShardFunction => "shard's method.function",
            Role::ShardJssg => "shard's jssg.transform",
        }
    }

    fn example_name(self) -> &'static str {
        match self {
            Role::ShardFunction => "shardFiles",
            _ => "codemod",
        }
    }

    fn example_file(self) -> &'static str {
        match self {
            Role::ShardFunction => "./scripts/shard.ts",
            _ => "./scripts/codemod.ts",
        }
    }

    fn written(self, name: &str) -> String {
        match self {
            Role::Jssg => format!("jssg({name})"),
            Role::ShardFunction => format!("shard({{ method: {{ function: {name} }} }})"),
            Role::ShardJssg => format!("shard({{ jssg: {{ transform: {name} }} }})"),
        }
    }
}

struct Analyzer<'s, 'a> {
    file: &'s str,
    source: &'s str,
    semantic: &'s Semantic<'a>,
    bindings: HashMap<SymbolId, ImportedBinding>,
    found: Transforms,
    /// Identifier spans of accepted transform arguments.
    accepted: Vec<Span>,
}

/// Find the transforms and rule files a workflow names, and check that the
/// build can read every one of them.
pub(crate) fn analyze<'a>(
    file: &str,
    source: &str,
    program: &Program<'a>,
    semantic: &Semantic<'a>,
) -> Result<Transforms, BuildError> {
    let mut analyzer = Analyzer {
        file,
        source,
        semantic,
        bindings: HashMap::new(),
        found: Transforms::default(),
        accepted: Vec::new(),
    };
    analyzer.collect_imports(program);

    let scoping = semantic.scoping();
    let mut flow: HashMap<SymbolId, String> = HashMap::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if import.import_kind.is_type() || !is_flow_module(&import.source.value) {
            continue;
        }
        for specifier in import.specifiers.iter().flatten() {
            if let ImportDeclarationSpecifier::ImportSpecifier(s) = specifier {
                if !s.import_kind.is_type() {
                    flow.insert(s.local.symbol_id(), s.imported.name().to_string());
                }
            }
        }
    }

    for node in semantic.nodes().iter() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        let Expression::Identifier(callee) = &call.callee else {
            continue;
        };
        let Some(name) = scoping
            .get_reference(callee.reference_id())
            .symbol_id()
            .and_then(|symbol| flow.get(&symbol))
        else {
            continue;
        };
        match name.as_str() {
            "jssg" => analyzer.jssg(call)?,
            "astGrep" => analyzer.ast_grep(call)?,
            "shard" => analyzer.shard(call)?,
            _ => {}
        }
    }
    analyzer.check_other_uses()?;
    Ok(analyzer.found)
}

impl<'a> Analyzer<'_, 'a> {
    fn error(&self, offset: u32, message: String) -> BuildError {
        error_at(self.file, self.source, offset, message)
    }

    fn collect_imports(&mut self, program: &Program<'a>) {
        for statement in &program.body {
            let Statement::ImportDeclaration(import) = statement else {
                continue;
            };
            if import.import_kind.is_type() {
                continue;
            }
            let Some(specifiers) = &import.specifiers else {
                continue;
            };
            let values: Vec<&ImportDeclarationSpecifier> = specifiers
                .iter()
                .filter(|s| {
                    !matches!(s, ImportDeclarationSpecifier::ImportSpecifier(s) if s.import_kind.is_type())
                })
                .collect();
            for (index, specifier) in specifiers.iter().enumerate() {
                let (symbol, imported) = match specifier {
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                        (s.local.symbol_id(), Imported::Default)
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        (s.local.symbol_id(), Imported::Namespace)
                    }
                    ImportDeclarationSpecifier::ImportSpecifier(s) => {
                        if s.import_kind.is_type() {
                            continue;
                        }
                        (
                            s.local.symbol_id(),
                            Imported::Named(s.imported.name().to_string()),
                        )
                    }
                };
                // A default import comes first: up to what follows the comma
                // after it (`{` or `*`) is what to cut.
                let span = specifier.span();
                let end = match specifiers.get(index + 1) {
                    Some(next) => {
                        let between = &self.source[span.end as usize..next.span().start as usize];
                        let after_comma = between.find(',').map_or(0, |comma| comma + 1);
                        let rest = &between[after_comma..];
                        let skipped = rest.len() - rest.trim_start().len();
                        span.end + (after_comma + skipped) as u32
                    }
                    None => span.end,
                };
                self.bindings.insert(
                    symbol,
                    ImportedBinding {
                        source: import.source.value.to_string(),
                        imported,
                        declaration: import.span,
                        specifier: Span::new(span.start, end),
                        alone: values.len() == 1,
                    },
                );
            }
        }
    }

    fn jssg(&mut self, call: &CallExpression<'a>) -> Result<(), BuildError> {
        let argument = self.first(call, Role::Jssg)?;
        self.transform(argument, None, Role::Jssg)
    }

    fn ast_grep(&mut self, call: &CallExpression<'a>) -> Result<(), BuildError> {
        let message = || {
            "astGrep needs its rule file's path written directly in the code (astGrep(\"./rules/config.yml\")), so the build can include the file when publishing".to_string()
        };
        let Some(argument) = call.arguments.first().and_then(Argument::as_expression) else {
            return Err(self.error(call.span.start, message()));
        };
        match literal_string(argument) {
            Some(path) => {
                self.found.files.push(path);
                Ok(())
            }
            None => Err(self.error(argument.span().start, message())),
        }
    }

    fn shard(&mut self, call: &CallExpression<'a>) -> Result<(), BuildError> {
        let (file, source) = (self.file, self.source);
        let inline = |at: u32, what: &str| {
            error_at(
                file,
                source,
                at,
                format!("write shard's {what} inline, as an object, so the build can find the files it runs"),
            )
        };
        let Some(Expression::ObjectExpression(options)) = call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        else {
            return Err(inline(call.span.start, "options"));
        };
        if let Some(spread) = spread(options) {
            return Err(inline(spread, "options"));
        }
        if let Some(method) = property(options, "method") {
            let Expression::ObjectExpression(method) = method.value.get_inner_expression() else {
                return Err(inline(method.value.span().start, "method"));
            };
            if let Some(spread) = spread(method) {
                return Err(inline(spread, "method"));
            }
            if let Some(function) = property(method, "function") {
                let shorthand = function.shorthand.then_some((function.span, "function"));
                self.transform(&function.value, shorthand, Role::ShardFunction)?;
            }
        }
        if let Some(jssg) = property(options, "jssg") {
            let Expression::ObjectExpression(jssg) = jssg.value.get_inner_expression() else {
                return Err(inline(jssg.value.span().start, "jssg options"));
            };
            if let Some(spread) = spread(jssg) {
                return Err(inline(spread, "jssg options"));
            }
            if let Some(path) = property(jssg, "path") {
                let fix = literal_string(&path.value)
                    .map(|path| suggestion(&path, Role::ShardJssg))
                    .unwrap_or_else(|| example(Role::ShardJssg));
                return Err(self.error(
                    path.span.start,
                    format!("shard's jssg takes the transform as `transform`, imported, not a file path as `path`: {fix}"),
                ));
            }
            match property(jssg, "transform") {
                Some(transform) => {
                    let shorthand = transform.shorthand.then_some((transform.span, "transform"));
                    self.transform(&transform.value, shorthand, Role::ShardJssg)?;
                }
                None => {
                    return Err(self.error(
                        jssg.span.start,
                        format!(
                            "shard's jssg needs the transform whose files it keeps: {}",
                            example(Role::ShardJssg)
                        ),
                    ))
                }
            }
        }
        Ok(())
    }

    fn first<'c>(
        &self,
        call: &'c CallExpression<'a>,
        role: Role,
    ) -> Result<&'c Expression<'a>, BuildError> {
        call.arguments
            .first()
            .and_then(Argument::as_expression)
            .ok_or_else(|| {
                self.error(
                    call.arguments
                        .first()
                        .map(|a| a.span().start)
                        .unwrap_or(call.span.start),
                    format!(
                        "{} takes an imported transform: {}",
                        role.call(),
                        example(role)
                    ),
                )
            })
    }

    /// Check one transform argument and record what to replace it with.
    /// `shorthand` is the property (`{ transform }`) when the argument is
    /// written as one, with its key.
    fn transform(
        &mut self,
        argument: &Expression<'a>,
        shorthand: Option<(Span, &str)>,
        role: Role,
    ) -> Result<(), BuildError> {
        let inner = argument.get_inner_expression();
        let at = argument.span().start;
        let call = role.call();
        match inner {
            // Choosing between imported transforms at run time.
            // (A shorthand property's value is an identifier, never this.)
            Expression::ConditionalExpression(choice) => {
                self.transform(&choice.consequent, None, role)?;
                self.transform(&choice.alternate, None, role)
            }
            Expression::Identifier(ident) => {
                let symbol = self
                    .semantic
                    .scoping()
                    .get_reference(ident.reference_id())
                    .symbol_id();
                let Some(binding) = symbol.and_then(|symbol| self.bindings.get(&symbol)) else {
                    return Err(self.error(
                        at,
                        format!(
                            "{call} takes a transform imported at the top of the workflow, so the build can include its file when publishing: {}",
                            example(role)
                        ),
                    ));
                };
                let binding = binding.clone();
                let name = ident.name.as_str();
                let specifier = &binding.source;
                match binding.imported {
                    Imported::Default => {}
                    Imported::Named(imported) => {
                        return Err(self.error(
                            at,
                            format!(
                                "{call} runs a file's default export, and '{name}' is the export '{imported}' of \"{specifier}\": export the transform as the file's default, and import it as import {name} from \"{specifier}\""
                            ),
                        ));
                    }
                    Imported::Namespace => {
                        return Err(self.error(
                            at,
                            format!(
                                "{call} runs a file's default export: import it as import {name} from \"{specifier}\", not as a namespace"
                            ),
                        ));
                    }
                }
                if !is_relative(specifier) {
                    return Err(self.error(
                        at,
                        format!(
                            "{call} runs transforms from the package's own files, and \"{specifier}\" is a package: copy the transform into this package and import it with a relative path (\"./...\"), or run that codemod with codemod(...)"
                        ),
                    ));
                }
                let extension = Path::new(specifier)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default();
                if !SCRIPT_EXTENSIONS.contains(&extension) {
                    return Err(self.error(
                        at,
                        format!(
                            "write the transform's file extension in its import (\"{specifier}.ts\"): the engine runs the file the import names"
                        ),
                    ));
                }
                let symbol = symbol.unwrap_or_else(|| unreachable!("bindings are keyed by symbol"));
                let reference = format!(
                    "{TRANSFORM_REFERENCE}({})",
                    serde_json::to_string(specifier).unwrap_or_default()
                );
                let replacement = match shorthand {
                    Some((property, key)) => (property, format!("{key}: {reference}")),
                    None => (argument.span(), reference),
                };
                self.found.replacements.push(replacement);
                self.accepted.push(ident.span);
                self.found.files.push(specifier.clone());
                if !self.found.specifiers.contains(specifier) {
                    self.found.specifiers.push(specifier.clone());
                }
                let cut = if binding.alone {
                    binding.declaration
                } else {
                    binding.specifier
                };
                self.found.imports.insert(
                    symbol,
                    TransformImport {
                        specifier: specifier.clone(),
                        cut,
                    },
                );
                Ok(())
            }
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {
                Err(self.error(
                    at,
                    format!(
                        "{call} cannot take an inline function yet: move the transform into its own file, as that file's default export, and import it ({})",
                        example(role)
                    ),
                ))
            }
            _ => match literal_string(inner) {
                Some(path) => Err(self.error(
                    at,
                    format!(
                        "{call} takes an imported transform, not a file path: {}",
                        suggestion(&path, role)
                    ),
                )),
                None => Err(self.error(
                    at,
                    format!(
                        "{call} takes an imported transform, not a file path, so the build can include its file when publishing: {}",
                        example(role)
                    ),
                )),
            },
        }
    }

    /// An imported transform is only given to `jssg` or `shard`: the
    /// workflow does not load its code.
    fn check_other_uses(&self) -> Result<(), BuildError> {
        if self.found.imports.is_empty() {
            return Ok(());
        }
        let scoping = self.semantic.scoping();
        for node in self.semantic.nodes().iter() {
            let AstKind::IdentifierReference(ident) = node.kind() else {
                continue;
            };
            let reference = scoping.get_reference(ident.reference_id());
            if !reference.is_value() {
                continue;
            }
            let Some(symbol) = reference.symbol_id() else {
                continue;
            };
            let Some(import) = self.found.imports.get(&symbol) else {
                continue;
            };
            if self.accepted.contains(&ident.span) {
                continue;
            }
            return Err(self.error(
                ident.span.start,
                format!(
                    "'{}' is the transform imported from \"{}\": only give it to jssg(...) or shard(...). The workflow does not load a transform's code; the engine runs it on each file",
                    ident.name, import.specifier
                ),
            ));
        }
        Ok(())
    }
}

/// The source with imported transforms replaced by references to their
/// files, and their imports dropped; `None` when it imports none. This is
/// what the Node.js host loads in place of the workflow file: the result
/// keeps the source's lines and its TypeScript.
pub fn rewrite_transforms(file: &str, source: &str) -> Result<Option<String>, BuildError> {
    use oxc::allocator::Allocator;
    use oxc::parser::Parser;
    use oxc::semantic::SemanticBuilder;
    use oxc::span::SourceType;

    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file)
        .unwrap_or_else(|_| SourceType::ts())
        .with_module(true);
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if let Some(error) = parsed.errors.first() {
        return Err(error_at(file, source, 0, error.to_string()));
    }
    let program = parsed.program;
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let found = analyze(file, source, &program, &semantic)?;
    if found.imports.is_empty() {
        return Ok(None);
    }
    let mut splices = found.replacements;
    let mut cuts: Vec<Span> = found.imports.values().map(|import| import.cut).collect();
    cuts.sort_by_key(|span| span.start);
    cuts.dedup();
    splices.extend(cuts.into_iter().map(|span| (span, String::new())));
    splices.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut rewritten = source.to_string();
    for (span, text) in splices {
        rewritten.replace_range(span.start as usize..span.end as usize, &text);
    }
    // At the end, so every line stays where it was: imports are hoisted.
    // `codemod:workflow` resolves to this package's own module wherever the
    // workflow is.
    rewritten.push_str(&format!(
        "\nimport {{ __transform as {TRANSFORM_REFERENCE} }} from \"codemod:workflow\";\n"
    ));
    Ok(Some(rewritten))
}

fn is_relative(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../")
}

/// A string literal, or a template literal without expressions.
fn literal_string(expression: &Expression) -> Option<String> {
    match expression.get_inner_expression() {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
            .quasis
            .first()
            .and_then(|q| q.value.cooked.as_ref().map(|c| c.to_string())),
        _ => None,
    }
}

fn property<'o, 'a>(
    object: &'o ObjectExpression<'a>,
    name: &str,
) -> Option<&'o oxc::ast::ast::ObjectProperty<'a>> {
    object.properties.iter().find_map(|property| {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        let named = match &property.key {
            PropertyKey::StaticIdentifier(id) => id.name == name,
            PropertyKey::StringLiteral(s) => s.value == name,
            _ => false,
        };
        (named && !property.computed).then_some(&**property)
    })
}

/// Where a spread (`{ ...options }`) in an object starts, if it has one.
fn spread(object: &ObjectExpression) -> Option<u32> {
    object
        .properties
        .iter()
        .find_map(|property| match property {
            ObjectPropertyKind::SpreadProperty(spread) => Some(spread.span.start),
            ObjectPropertyKind::ObjectProperty(_) => None,
        })
}

/// How to import the transform at `path`, as a fix for a path argument.
fn suggestion(path: &str, role: Role) -> String {
    let specifier = import_specifier(path);
    let name = binding_name(&specifier, role.example_name());
    format!(
        "add import {name} from \"{specifier}\" at the top of the workflow and write {}",
        role.written(&name)
    )
}

fn example(role: Role) -> String {
    let name = role.example_name();
    format!(
        "import {name} from \"{}\", then {}",
        role.example_file(),
        role.written(name)
    )
}

/// A relative import specifier for a path relative to the workflow.
pub(crate) fn import_specifier(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    if is_relative(&path) {
        path
    } else {
        format!("./{path}")
    }
}

/// An identifier for the file a specifier names, from its file name
/// (`./scripts/rename-imports.ts` gives `renameImports`).
pub(crate) fn binding_name(specifier: &str, fallback: &str) -> String {
    let stem = Path::new(specifier)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut name = String::new();
    let mut upper = false;
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
            if upper && !name.is_empty() {
                name.push(c.to_ascii_uppercase());
            } else {
                name.push(c);
            }
            upper = false;
        } else {
            upper = true;
        }
    }
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) || is_reserved(&name) {
        return fallback.to_string();
    }
    name
}

fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "let"
            | "static"
            | "await"
            | "implements"
            | "interface"
            | "package"
            | "private"
            | "protected"
            | "public"
            // The functions a workflow imports from codemod/workflow.
            | "step"
            | "jssg"
            | "shard"
            | "astGrep"
            | "forEachFile"
            | "shell"
            | "agent"
    )
}

#[cfg(test)]
mod tests {
    use crate::build::{build, inspect};

    use super::rewrite_transforms;

    fn error(source: &str) -> String {
        match build("workflow.ts", source) {
            Ok(built) => panic!("expected a build error, got:\n{}", built.main),
            Err(error) => error.message,
        }
    }

    #[test]
    fn an_imported_transform_becomes_a_reference_to_its_file() {
        let source = r#"
import { step, forEachFile, jssg } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";

export default async function main() {
  await step("Migrate", forEachFile(jssg(codemod, { params: { strict: true } })));
}
"#;
        let built = build("workflow.ts", source).unwrap();
        assert!(
            built
                .main
                .contains(r#"jssg(__flowTransform("./scripts/codemod.ts"), {"#),
            "{}",
            built.main
        );
        assert!(
            built
                .main
                .contains(r#"import { __transform as __flowTransform } from "codemod/workflow";"#),
            "{}",
            built.main
        );
        // The transform's module is not loaded by the body.
        assert!(
            !built.main.contains("from \"./scripts/codemod.ts\""),
            "{}",
            built.main
        );
        assert!(built.body_imports.is_empty(), "{:?}", built.body_imports);

        let inspection = inspect("workflow.ts", source).unwrap();
        assert_eq!(inspection.files, vec!["./scripts/codemod.ts"]);
        assert_eq!(inspection.transforms, vec!["./scripts/codemod.ts"]);
    }

    #[test]
    fn shard_functions_and_shard_transforms_are_imported_too() {
        let source = r#"
import { step, shard } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";
import byTeam from "../shared/by-team.mjs";

export default async function main() {
  await step(shard({ method: { function: byTeam }, jssg: { transform: codemod, include: "**/*.ts" } }));
}
"#;
        let built = build("workflow.ts", source).unwrap();
        assert!(
            built
                .main
                .contains(r#"function: __flowTransform("../shared/by-team.mjs")"#),
            "{}",
            built.main
        );
        assert!(
            built
                .main
                .contains(r#"transform: __flowTransform("./scripts/codemod.ts")"#),
            "{}",
            built.main
        );
        let inspection = inspect("workflow.ts", source).unwrap();
        assert_eq!(
            inspection.files,
            vec!["../shared/by-team.mjs", "./scripts/codemod.ts"]
        );
    }

    #[test]
    fn astgrep_paths_written_in_the_call_still_work() {
        let source = r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  await step(forEachFile(astGrep("./rules.yaml")));
  await step(forEachFile(astGrep(`rules/more.yml`)));
}
"#;
        let built = build("workflow.ts", source).unwrap();
        assert!(
            built.main.contains(r#"astGrep("./rules.yaml")"#),
            "{}",
            built.main
        );
        let inspection = inspect("workflow.ts", source).unwrap();
        assert_eq!(inspection.files, vec!["./rules.yaml", "rules/more.yml"]);
        assert!(inspection.transforms.is_empty());
    }

    #[test]
    fn a_path_given_to_jssg_is_an_error_that_says_how_to_import_it() {
        let message = error(
            r#"
import { step, jssg } from "codemod/workflow";
export default async function main() {
  await step(jssg("scripts/rename-imports.ts"));
}
"#,
        );
        assert_eq!(
            message,
            r#"jssg takes an imported transform, not a file path: add import renameImports from "./scripts/rename-imports.ts" at the top of the workflow and write jssg(renameImports)"#
        );
    }

    #[test]
    fn a_path_computed_at_run_time_is_an_error() {
        let message = error(
            r#"
import { step, jssg } from "codemod/workflow";
export default async function main({ params }) {
  await step(jssg(`./transforms/${params.name}.ts`));
}
"#,
        );
        assert!(
            message.starts_with("jssg takes an imported transform, not a file path, so the build can include its file when publishing"),
            "{message}"
        );
        let message = error(
            r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
const rules = "./rules.yaml";
export default async function main() {
  await step(forEachFile(astGrep(rules)));
}
"#,
        );
        assert!(
            message.starts_with("astGrep needs its rule file's path written directly in the code"),
            "{message}"
        );
        let message = error(
            r#"
import { step, shard } from "codemod/workflow";
export default async function main({ params }) {
  await step(shard({ method: { function: `./shards/${params.kind}.ts` } }));
}
"#,
        );
        assert!(
            message.starts_with("shard's method.function takes an imported transform"),
            "{message}"
        );
        let message = error(
            r#"
import { step, shard } from "codemod/workflow";
export default async function main({ params }) {
  await step(shard(params.shardOptions));
}
"#,
        );
        assert!(
            message.starts_with("write shard's options inline"),
            "{message}"
        );
    }

    #[test]
    fn a_transform_can_be_chosen_at_run_time_from_imported_ones() {
        let source = r#"
import { step, jssg } from "codemod/workflow";
import strict from "./scripts/strict.ts";
import loose from "./scripts/loose.ts";
export default async function main({ params }) {
  await step(jssg(params.strict ? strict : loose));
}
"#;
        let built = build("workflow.ts", source).unwrap();
        assert!(
            built.main.contains(
                r#"jssg(params.strict ? __flowTransform("./scripts/strict.ts") : __flowTransform("./scripts/loose.ts"))"#
            ),
            "{}",
            built.main
        );
        let inspection = inspect("workflow.ts", source).unwrap();
        assert_eq!(
            inspection.files,
            vec!["./scripts/strict.ts", "./scripts/loose.ts"]
        );
        let message = error(
            r#"
import { step, jssg } from "codemod/workflow";
import strict from "./scripts/strict.ts";
export default async function main({ params }) {
  await step(jssg(params.strict ? strict : "./scripts/loose.ts"));
}
"#,
        );
        assert!(
            message
                .starts_with("jssg takes an imported transform, not a file path: add import loose"),
            "{message}"
        );
    }

    #[test]
    fn an_inline_transform_is_not_supported_yet() {
        let message = error(
            r#"
import { step, jssg } from "codemod/workflow";
export default async function main() {
  await step(jssg((root) => null));
}
"#,
        );
        assert!(
            message.starts_with("jssg cannot take an inline function yet"),
            "{message}"
        );
    }

    #[test]
    fn an_imported_transform_is_only_given_to_jssg() {
        let message = error(
            r#"
import { step, forEachFile, jssg } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";
export default async function main() {
  await step(forEachFile(jssg(codemod)));
  console.log(codemod.name);
}
"#,
        );
        assert!(
            message.starts_with(
                "'codemod' is the transform imported from \"./scripts/codemod.ts\": only give it to jssg(...) or shard(...)"
            ),
            "{message}"
        );
        // Also in a callback, which would load the transform's code.
        let message = error(
            r#"
import { step, forEachFile, jssg } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";
export default async function main() {
  await step(forEachFile(jssg(codemod)));
  await step(() => typeof codemod);
}
"#,
        );
        assert!(
            message.starts_with("'codemod' is the transform imported"),
            "{message}"
        );
    }

    #[test]
    fn transforms_are_default_imports_of_the_packages_own_files() {
        let named = error(
            r#"
import { step, jssg } from "codemod/workflow";
import { transform } from "./scripts/codemod.ts";
export default async function main() { await step(jssg(transform)); }
"#,
        );
        assert!(
            named.starts_with(
                "jssg runs a file's default export, and 'transform' is the export 'transform'"
            ),
            "{named}"
        );
        let namespace = error(
            r#"
import { step, jssg } from "codemod/workflow";
import * as codemod from "./scripts/codemod.ts";
export default async function main() { await step(jssg(codemod)); }
"#,
        );
        assert!(namespace.contains("not as a namespace"), "{namespace}");
        let package = error(
            r#"
import { step, jssg } from "codemod/workflow";
import codemod from "some-codemods/rename";
export default async function main() { await step(jssg(codemod)); }
"#,
        );
        assert!(
            package.contains("\"some-codemods/rename\" is a package"),
            "{package}"
        );
        let extension = error(
            r#"
import { step, jssg } from "codemod/workflow";
import codemod from "./scripts/codemod";
export default async function main() { await step(jssg(codemod)); }
"#,
        );
        assert!(
            extension.starts_with("write the transform's file extension"),
            "{extension}"
        );
        let local = error(
            r#"
import { step, jssg } from "codemod/workflow";
const codemod = (root: unknown) => null;
export default async function main() { await step(jssg(codemod)); }
"#,
        );
        assert!(
            local.starts_with("jssg takes a transform imported at the top of the workflow"),
            "{local}"
        );
    }

    #[test]
    fn the_rest_of_a_transforms_import_stays() {
        let source = r#"import { step, forEachFile, jssg } from "codemod/workflow";
import codemod, { PATTERN } from "./scripts/codemod.ts";
export default async function main() {
  await step(forEachFile(jssg(codemod, { params: { pattern: PATTERN } })));
}
"#;
        let built = build("workflow.ts", source).unwrap();
        assert!(
            built
                .main
                .contains(r#"import { PATTERN } from "./scripts/codemod.ts";"#),
            "{}",
            built.main
        );
        assert!(!built.main.contains("import codemod"), "{}", built.main);

        let rewritten = rewrite_transforms("workflow.ts", source).unwrap().unwrap();
        assert!(
            rewritten.contains("import { PATTERN } from \"./scripts/codemod.ts\";"),
            "{rewritten}"
        );
    }

    #[test]
    fn the_node_rewrite_keeps_lines_and_types() {
        let source = r#"import { step, forEachFile, jssg, shard } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";
import shardFiles from "./scripts/shard.ts";

export default async function main(): Promise<void> {
  const transform = codemod;
  await step(forEachFile(jssg(codemod as any)));
}
"#;
        // A transform used for something else fails here too.
        let message = rewrite_transforms("workflow.ts", source)
            .unwrap_err()
            .message;
        assert!(
            message.starts_with("'codemod' is the transform imported"),
            "{message}"
        );

        let source = r#"import { step, forEachFile, jssg, shard } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";
import shardFiles from "./scripts/shard.ts";

export default async function main(): Promise<void> {
  const function_ = shardFiles;
  await step(shard({ method: { function: shardFiles } }));
  await step(forEachFile(jssg(codemod as any)));
}
"#;
        assert!(rewrite_transforms("workflow.ts", source).is_err());

        let source = r#"import { step, forEachFile, jssg } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";

export default async function main(): Promise<void> {
  await step(forEachFile(jssg(codemod as any)));
}
"#;
        let rewritten = rewrite_transforms("workflow.ts", source).unwrap().unwrap();
        assert_eq!(
            rewritten,
            r#"import { step, forEachFile, jssg } from "codemod/workflow";


export default async function main(): Promise<void> {
  await step(forEachFile(jssg(__flowTransform("./scripts/codemod.ts"))));
}

import { __transform as __flowTransform } from "codemod:workflow";
"#
        );
        let plain =
            "import { step } from \"codemod/workflow\";\nexport default async function main() {}\n";
        assert_eq!(rewrite_transforms("workflow.ts", plain).unwrap(), None);
    }

    #[test]
    fn a_shorthand_property_is_rewritten_with_its_key() {
        let source = r#"
import { step, shard } from "codemod/workflow";
import transform from "./scripts/codemod.ts";
export default async function main() {
  await step(shard({ method: { type: "directory", maxFilesPerShard: 5 }, jssg: { transform } }));
}
"#;
        let built = build("workflow.ts", source).unwrap();
        assert!(
            built
                .main
                .contains(r#"jssg: { transform: __flowTransform("./scripts/codemod.ts") }"#),
            "{}",
            built.main
        );
        let message = error(
            r#"
import { step, shard } from "codemod/workflow";
export default async function main() {
  await step(shard({ method: { type: "directory", maxFilesPerShard: 5 }, jssg: { path: "scripts/codemod.ts" } }));
}
"#,
        );
        assert_eq!(
            message,
            r#"shard's jssg takes the transform as `transform`, imported, not a file path as `path`: add import codemod from "./scripts/codemod.ts" at the top of the workflow and write shard({ jssg: { transform: codemod } })"#
        );
    }
}
