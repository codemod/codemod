//! Build step.
//!
//! Finds every `forEachFile(selector, callback)` call in a workflow module,
//! extracts the callback into its own module, and rewrites the call site so
//! the callback's outside variables travel as captures:
//!
//! - module-level declarations the callback uses are copied into its module;
//! - locals of an enclosing function become captures, evaluated when the step
//!   starts and sent by value;
//! - anything else (assignment to an outside variable, a captured local that
//!   is reassigned, a captured function) is a build error.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Argument, Declaration, Expression, ImportDeclarationSpecifier, Statement, VariableDeclaration,
};
use oxc::ast::AstKind;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::{Scoping, SemanticBuilder, SymbolFlags, SymbolId};
use oxc::span::{GetSpan, SourceType, Span};
use oxc::transformer::{TransformOptions, Transformer};
use sha2::{Digest, Sha256};

/// Specifier of the authoring module.
pub const FLOW_MODULE: &str = "codemod:workflow";

/// Prelude appended to every extracted callback module.
const CALLBACK_PRELUDE: &str = include_str!("js/callback_prelude.js");

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{file}:{line}:{column}: {message}")]
pub struct BuildError {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// Output of the build: the rewritten workflow module and one module per
/// callback, keyed by a content hash.
#[derive(Debug, Clone)]
pub struct Built {
    pub main: String,
    pub callbacks: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct ImportBinding {
    source: String,
    /// `None` for default and namespace imports.
    imported: Option<String>,
    /// The import rewritten for a single binding.
    line: String,
}

#[derive(Debug, Clone)]
struct ModuleDecl {
    /// Span whose text is copied, and whose references are followed.
    span: Span,
    emit: String,
}

struct Analysis<'a> {
    file: &'a str,
    source: &'a str,
    scoping: &'a Scoping,
    imports: HashMap<SymbolId, ImportBinding>,
    decls: HashMap<SymbolId, ModuleDecl>,
    /// Every identifier reference in the module: its span and resolved symbol
    /// (`None` for globals).
    references: Vec<IdentRef>,
    /// Variables initialised with a function or class expression.
    function_valued: HashSet<SymbolId>,
}

#[derive(Debug, Clone, Copy)]
struct IdentRef {
    span: Span,
    symbol: Option<SymbolId>,
    write: bool,
    value: bool,
}

#[derive(Default)]
struct CallbackParts {
    imports: BTreeSet<String>,
    /// Hoisted declarations, keyed by source position.
    hoisted: BTreeMap<u32, String>,
    parse_aliases: BTreeSet<String>,
    captures: BTreeSet<String>,
}

pub fn build(file: &str, source: &str) -> Result<Built, BuildError> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if let Some(error) = parsed.errors.first() {
        return Err(error_at(file, source, 0, error.to_string()));
    }
    let program = parsed.program;
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let scoping = semantic.scoping();

    let mut analysis = Analysis {
        file,
        source,
        scoping,
        imports: HashMap::new(),
        decls: HashMap::new(),
        references: Vec::new(),
        function_valued: HashSet::new(),
    };

    for statement in &program.body {
        match statement {
            Statement::ImportDeclaration(import) => {
                if import.import_kind.is_type() {
                    continue;
                }
                let module = import.source.value.to_string();
                for specifier in import.specifiers.iter().flatten() {
                    let (local, imported, line) = match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(s) => {
                            if s.import_kind.is_type() {
                                continue;
                            }
                            let imported = s.imported.name().to_string();
                            let line = format!(
                                "import {{ {imported} as {} }} from {:?};",
                                s.local.name, module
                            );
                            (&s.local, Some(imported), line)
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                            let line = format!("import {} from {:?};", s.local.name, module);
                            (&s.local, None, line)
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                            let line = format!("import * as {} from {:?};", s.local.name, module);
                            (&s.local, None, line)
                        }
                    };
                    if module != FLOW_MODULE {
                        return Err(analysis.error(
                            import.span.start,
                            format!(
                                "the spike only supports imports from '{FLOW_MODULE}' in the workflow module, not '{module}'"
                            ),
                        ));
                    }
                    analysis.imports.insert(
                        local.symbol_id(),
                        ImportBinding {
                            source: module.clone(),
                            imported,
                            line,
                        },
                    );
                }
            }
            Statement::VariableDeclaration(declaration) => {
                analysis.collect_variables(declaration);
            }
            Statement::FunctionDeclaration(function) => {
                if let Some(id) = &function.id {
                    analysis.collect_whole(id.symbol_id(), function.span);
                }
            }
            Statement::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    analysis.collect_whole(id.symbol_id(), class.span);
                }
            }
            Statement::ExportNamedDeclaration(export) => match &export.declaration {
                Some(Declaration::VariableDeclaration(declaration)) => {
                    analysis.collect_variables(declaration);
                }
                Some(Declaration::FunctionDeclaration(function)) => {
                    if let Some(id) = &function.id {
                        analysis.collect_whole(id.symbol_id(), function.span);
                    }
                }
                Some(Declaration::ClassDeclaration(class)) => {
                    if let Some(id) = &class.id {
                        analysis.collect_whole(id.symbol_id(), class.span);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    let mut calls: Vec<Span> = Vec::new();
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::IdentifierReference(ident) => {
                let reference = scoping.get_reference(ident.reference_id());
                analysis.references.push(IdentRef {
                    span: ident.span,
                    symbol: reference.symbol_id(),
                    write: reference.is_write(),
                    value: reference.is_value(),
                });
            }
            AstKind::VariableDeclarator(declarator) => {
                if matches!(
                    declarator.init,
                    Some(
                        Expression::ArrowFunctionExpression(_)
                            | Expression::FunctionExpression(_)
                            | Expression::ClassExpression(_)
                    )
                ) {
                    for binding in declarator.id.get_binding_identifiers() {
                        analysis.function_valued.insert(binding.symbol_id());
                    }
                }
            }
            AstKind::CallExpression(call) => {
                let Expression::Identifier(callee) = &call.callee else {
                    continue;
                };
                let Some(symbol) = scoping.get_reference(callee.reference_id()).symbol_id() else {
                    continue;
                };
                let is_for_each_file = analysis.imports.get(&symbol).is_some_and(|import| {
                    import.source == FLOW_MODULE
                        && import.imported.as_deref() == Some("forEachFile")
                });
                if !is_for_each_file {
                    continue;
                }
                match call.arguments.get(1) {
                    Some(Argument::ArrowFunctionExpression(f)) => calls.push(f.span),
                    Some(Argument::FunctionExpression(f)) => calls.push(f.span),
                    Some(other) => {
                        return Err(analysis.error(
                            other.span().start,
                            "forEachFile's callback must be written inline as a function, so the build can extract it".to_string(),
                        ));
                    }
                    None => {
                        return Err(analysis.error(
                            call.span.start,
                            "forEachFile takes a selector and a callback".to_string(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    let mut callbacks = BTreeMap::new();
    let mut splices: Vec<(Span, String)> = Vec::new();
    for span in calls {
        let parts = analysis.callback_parts(span)?;
        let callback_source = &source[span.start as usize..span.end as usize];
        let destructure = parts
            .captures
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");

        let mut module = String::new();
        module.push_str("import { parse as __sgParse } from \"codemod:ast-grep\";\n");
        for line in &parts.imports {
            module.push_str(line);
            module.push('\n');
        }
        for text in parts.hoisted.values() {
            module.push_str(text);
            module.push('\n');
        }
        for local in &parts.parse_aliases {
            module.push_str(&format!("const {local} = __flowParse;\n"));
        }
        module.push_str(&format!(
            "const __make = ({{ {destructure} }}) => ({callback_source});\n"
        ));
        module.push_str(CALLBACK_PRELUDE);

        let module =
            strip_types(file, &module).map_err(|message| analysis.error(span.start, message))?;
        let id = content_id(&module);
        splices.push((
            span,
            format!("__flowCallback({id:?}, () => ({{ {destructure} }}))"),
        ));
        callbacks.insert(id, module);
    }

    let mut main = source.to_string();
    splices.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    for (span, replacement) in splices {
        main.replace_range(span.start as usize..span.end as usize, &replacement);
    }
    let main = format!("import {{ __callback as __flowCallback }} from {FLOW_MODULE:?};\n{main}");
    let main = strip_types(file, &main).map_err(|message| error_at(file, source, 0, message))?;

    Ok(Built { main, callbacks })
}

impl Analysis<'_> {
    fn error(&self, offset: u32, message: String) -> BuildError {
        error_at(self.file, self.source, offset, message)
    }

    fn text(&self, span: Span) -> &str {
        &self.source[span.start as usize..span.end as usize]
    }

    fn collect_variables(&mut self, declaration: &VariableDeclaration) {
        for declarator in &declaration.declarations {
            let emit = format!(
                "{} {};",
                declaration.kind.as_str(),
                self.text(declarator.span)
            );
            for binding in declarator.id.get_binding_identifiers() {
                self.decls.insert(
                    binding.symbol_id(),
                    ModuleDecl {
                        span: declarator.span,
                        emit: emit.clone(),
                    },
                );
            }
        }
    }

    fn collect_whole(&mut self, symbol: SymbolId, span: Span) {
        let emit = self.text(span).to_string();
        self.decls.insert(symbol, ModuleDecl { span, emit });
    }

    fn name(&self, symbol: SymbolId) -> &str {
        self.scoping.symbol_name(symbol)
    }

    fn is_reassigned(&self, symbol: SymbolId) -> bool {
        self.scoping
            .get_resolved_references(symbol)
            .any(|reference| reference.is_write())
    }

    fn references_in(&self, span: Span) -> impl Iterator<Item = &IdentRef> {
        self.references
            .iter()
            .filter(move |r| r.value && span.start <= r.span.start && r.span.end <= span.end)
    }

    fn callback_parts(&self, callback: Span) -> Result<CallbackParts, BuildError> {
        let root = self.scoping.root_scope_id();
        let mut parts = CallbackParts::default();
        let mut hoist_queue: Vec<SymbolId> = Vec::new();

        for reference in self.references_in(callback) {
            let Some(symbol) = reference.symbol else {
                continue; // a global
            };
            let declared = self.scoping.symbol_span(symbol);
            if callback.start <= declared.start && declared.end <= callback.end {
                continue; // the callback's own binding
            }
            let name = self.name(symbol);
            if reference.write {
                return Err(self.error(
                    reference.span.start,
                    format!(
                        "assigns to '{name}', which is declared outside the callback; callbacks run in isolated contexts, so return a value instead"
                    ),
                ));
            }
            if self.scoping.symbol_scope_id(symbol) == root {
                self.module_level(symbol, reference.span, &mut parts, &mut hoist_queue)?;
                continue;
            }
            let flags = self.scoping.symbol_flags(symbol);
            if flags.intersects(SymbolFlags::Function | SymbolFlags::Class)
                || self.function_valued.contains(&symbol)
            {
                return Err(self.error(
                    reference.span.start,
                    format!(
                        "captures '{name}', a function or class declared inside a function; move it to module level so it can be copied into the callback"
                    ),
                ));
            }
            if self.is_reassigned(symbol) {
                return Err(self.error(
                    reference.span.start,
                    format!(
                        "captures '{name}', which is reassigned; captured variables must not change after they are declared"
                    ),
                ));
            }
            parts.captures.insert(name.to_string());
        }

        let mut seen: HashSet<SymbolId> = HashSet::new();
        while let Some(symbol) = hoist_queue.pop() {
            if !seen.insert(symbol) {
                continue;
            }
            let decl = &self.decls[&symbol];
            parts.hoisted.insert(decl.span.start, decl.emit.clone());
            for reference in self.references_in(decl.span) {
                let Some(inner) = reference.symbol else {
                    continue;
                };
                let declared = self.scoping.symbol_span(inner);
                if decl.span.start <= declared.start && declared.end <= decl.span.end {
                    continue;
                }
                if self.scoping.symbol_scope_id(inner) == root {
                    self.module_level(inner, reference.span, &mut parts, &mut hoist_queue)?;
                }
            }
        }
        Ok(parts)
    }

    fn module_level(
        &self,
        symbol: SymbolId,
        at: Span,
        parts: &mut CallbackParts,
        hoist_queue: &mut Vec<SymbolId>,
    ) -> Result<(), BuildError> {
        let name = self.name(symbol);
        if let Some(import) = self.imports.get(&symbol) {
            if import.source == FLOW_MODULE {
                if import.imported.as_deref() == Some("parse") {
                    parts.parse_aliases.insert(name.to_string());
                    return Ok(());
                }
                return Err(self.error(
                    at.start,
                    format!(
                        "uses '{name}' inside a forEachFile callback; steps cannot start from inside a callback"
                    ),
                ));
            }
            parts.imports.insert(import.line.clone());
            return Ok(());
        }
        if self.decls.contains_key(&symbol) {
            if self.is_reassigned(symbol) {
                return Err(self.error(
                    at.start,
                    format!(
                        "uses '{name}', a module-level binding that is reassigned; only bindings that never change can be copied into a callback"
                    ),
                ));
            }
            hoist_queue.push(symbol);
            return Ok(());
        }
        Err(self.error(
            at.start,
            format!(
                "uses '{name}', which cannot be copied into the callback; only const, let, var, function and class declarations can"
            ),
        ))
    }
}

/// Strip TypeScript syntax.
fn strip_types(file: &str, source: &str) -> Result<String, String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if let Some(error) = parsed.errors.first() {
        return Err(format!("generated module does not parse: {error}"));
    }
    let mut program = parsed.program;
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    let options = TransformOptions::default();
    let transformed = Transformer::new(&allocator, Path::new(file), &options)
        .build_with_scoping(scoping, &mut program);
    if let Some(error) = transformed.errors.first() {
        return Err(format!("could not strip types: {error}"));
    }
    Ok(Codegen::new().build(&program).code)
}

fn content_id(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("cb_{hex}")
}

fn error_at(file: &str, source: &str, offset: u32, message: String) -> BuildError {
    let before = &source[..(offset as usize).min(source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map(|i| i + 1).unwrap_or(0) + 1;
    BuildError {
        file: file.to_string(),
        line,
        column,
        message,
    }
}

/// What a workflow module does, read without running it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspection {
    /// `codemod:workflow` functions the module calls, by imported name, in
    /// source order (`step`, `forEachFile`, `jssg`, `installSkill`, ...).
    pub calls: Vec<String>,
    /// `installSkill({ package, path })` calls whose options are literals.
    pub install_skills: Vec<InstalledSkill>,
    /// String-literal paths passed to `jssg`, `astGrep` and shard functions.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledSkill {
    pub package: String,
    pub path: Option<String>,
}

impl Inspection {
    pub fn calls(&self, name: &str) -> bool {
        self.calls.iter().any(|call| call == name)
    }
}

/// Read a workflow module's task calls without running it.
pub fn inspect(file: &str, source: &str) -> Result<Inspection, BuildError> {
    use oxc::ast::ast::{ObjectPropertyKind, PropertyKey};

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if let Some(error) = parsed.errors.first() {
        return Err(error_at(file, source, 0, error.to_string()));
    }
    let program = parsed.program;
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let scoping = semantic.scoping();

    let mut imported: HashMap<SymbolId, String> = HashMap::new();
    for statement in &program.body {
        if let Statement::ImportDeclaration(import) = statement {
            if import.source.value != FLOW_MODULE {
                continue;
            }
            for specifier in import.specifiers.iter().flatten() {
                if let ImportDeclarationSpecifier::ImportSpecifier(s) = specifier {
                    imported.insert(s.local.symbol_id(), s.imported.name().to_string());
                }
            }
        }
    }

    let string_of = |expression: &Expression| match expression {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
            .quasis
            .first()
            .and_then(|q| q.value.cooked.as_ref().map(|c| c.to_string())),
        _ => None,
    };

    let mut inspection = Inspection::default();
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
            .and_then(|symbol| imported.get(&symbol))
        else {
            continue;
        };
        inspection.calls.push(name.clone());
        let first = call.arguments.first().and_then(|a| a.as_expression());
        match name.as_str() {
            "jssg" | "astGrep" => {
                if let Some(path) = first.and_then(string_of) {
                    inspection.files.push(path);
                }
            }
            "installSkill" | "shard" => {
                let Some(Expression::ObjectExpression(object)) = first else {
                    continue;
                };
                let mut package = None;
                let mut path = None;
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        continue;
                    };
                    let key = match &property.key {
                        PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                        PropertyKey::StringLiteral(s) => s.value.to_string(),
                        _ => continue,
                    };
                    match (name.as_str(), key.as_str()) {
                        ("installSkill", "package") => package = string_of(&property.value),
                        ("installSkill", "path") => path = string_of(&property.value),
                        ("shard", "method") => {
                            if let Expression::ObjectExpression(method) = &property.value {
                                for inner in &method.properties {
                                    if let ObjectPropertyKind::ObjectProperty(inner) = inner {
                                        let is_function = matches!(&inner.key, PropertyKey::StaticIdentifier(id) if id.name == "function");
                                        if is_function {
                                            if let Some(file) = string_of(&inner.value) {
                                                inspection.files.push(file);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(package) = package {
                    inspection
                        .install_skills
                        .push(InstalledSkill { package, path });
                }
            }
            _ => {}
        }
    }
    Ok(inspection)
}
