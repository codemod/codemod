//! Build step.
//!
//! Finds every `forEachFile(selector?, callback)` call and every function
//! step (`step(() => ...)`, `step(label, () => ...)`) in a workflow module,
//! extracts the function into its own module, which runs in an isolate, and
//! rewrites the call site so the function's outside variables travel as
//! captures:
//!
//! - module-level declarations and imports the function uses are copied into
//!   its module;
//! - locals of an enclosing function become captures, evaluated when the step
//!   starts and sent by value;
//! - anything else (assignment to an outside variable, a captured local that
//!   is reassigned, a captured function) is a build error.
//!
//! The rest of the module, the workflow body, must stay deterministic: it
//! runs again on every resume. So imports are sorted into classes
//! ([`ModuleClass`]). Pure modules (`path`, `url`, `codemod/ast-grep`, ...)
//! work anywhere; modules that touch files, the network, processes or the
//! clock only inside a step or callback. Package and relative imports the
//! body uses are listed in [`Built::body_imports`] for [`check_imports`],
//! which walks their import graphs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use codemod_sandbox::codemod_modules::codemod_module;
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

/// Specifier of the authoring module. `codemod/workflow`, the older
/// spelling, names it too; see [`is_flow_module`].
pub const FLOW_MODULE: &str = "codemod/workflow";

/// Whether a specifier names the authoring module (`codemod/workflow`, or
/// the older `codemod/workflow`).
pub fn is_flow_module(specifier: &str) -> bool {
    codemod_module(specifier) == Some("workflow")
}

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
/// callback or function step, keyed by a content hash.
#[derive(Debug, Clone)]
pub struct Built {
    pub main: String,
    pub callbacks: BTreeMap<String, String>,
    /// Package and relative imports the workflow body uses; see [`check_imports`].
    pub body_imports: Vec<BodyImport>,
}

/// A package or relative import used by the workflow body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyImport {
    pub source: String,
    /// Where the import is, for error positions.
    pub offset: u32,
}

/// Where code from a module may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleClass {
    /// `codemod/workflow` itself.
    Workflow,
    /// Deterministic: usable in the workflow body.
    Pure,
    /// Touches files, the network, processes, the clock or randomness: only
    /// inside a step or `forEachFile` callback.
    StepsOnly,
    /// A package or relative file, classified by [`check_imports`].
    Package,
}

/// Builtin modules the workflow body may use.
const PURE_BUILTINS: &[&str] = &[
    "path",
    "url",
    "util",
    "buffer",
    "string_decoder",
    "assert",
    "events",
    "zlib",
    "stream/web",
];

/// The class of an import specifier, without looking at the file system.
pub fn module_class(specifier: &str) -> Result<ModuleClass, String> {
    match codemod_module(specifier) {
        Some("workflow") => return Ok(ModuleClass::Workflow),
        Some("metrics" | "ast-grep") => return Ok(ModuleClass::Pure),
        Some(_) => return Ok(ModuleClass::StepsOnly),
        None => {}
    }
    // Type-only language maps: `codemod/ast-grep/langs/java`.
    if ["codemod/ast-grep/", "codemod/ast-grep/"]
        .iter()
        .any(|prefix| specifier.starts_with(prefix))
    {
        return Ok(ModuleClass::Pure);
    }
    if specifier.starts_with("codemod:") {
        return Err(format!("unknown module '{specifier}'"));
    }
    if let Some(name) = specifier.strip_prefix("codemod/") {
        return Err(format!(
            "'{specifier}' is for Node.js, not for workflows: workflows import codemod/workflow, codemod/ast-grep, codemod/metrics, codemod/runtime and codemod/llm ({name} is not one of them)"
        ));
    }
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    if PURE_BUILTINS.contains(&bare) {
        return Ok(ModuleClass::Pure);
    }
    if specifier.starts_with("node:") || is_effectful_builtin(bare) {
        return Ok(ModuleClass::StepsOnly);
    }
    Ok(ModuleClass::Package)
}

fn is_effectful_builtin(name: &str) -> bool {
    matches!(
        name,
        "fs" | "fs/promises"
            | "child_process"
            | "os"
            | "process"
            | "crypto"
            | "timers"
            | "timers/promises"
            | "perf_hooks"
            | "tty"
            | "net"
            | "http"
            | "https"
            | "http2"
            | "dns"
            | "dgram"
            | "tls"
            | "worker_threads"
            | "cluster"
            | "readline"
            | "module"
            | "v8"
            | "vm"
    )
}

#[derive(Debug, Clone)]
struct ImportBinding {
    source: String,
    /// `None` for default and namespace imports.
    imported: Option<String>,
    /// The import rewritten for a single binding.
    line: String,
    /// Start of the import declaration.
    at: u32,
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
    captures: BTreeSet<String>,
}

/// What kind of function is being extracted, for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Isolated {
    Callback,
    FunctionStep,
}

impl Isolated {
    fn noun(self) -> &'static str {
        match self {
            Isolated::Callback => "forEachFile callback",
            Isolated::FunctionStep => "function step",
        }
    }
}

/// `codemod/workflow` functions that work inside an isolate: the JSSG state API.
const ISOLATE_STATE_API: &[&str] = &["getState", "setState", "unsetState", "acquireLock"];

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

    // Value imports are re-emitted (or dropped) below, so their declarations
    // are cut from the body.
    let mut import_spans: Vec<Span> = Vec::new();
    let mut side_effect_imports: Vec<(String, u32)> = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::ImportDeclaration(import) => {
                if import.import_kind.is_type() {
                    continue;
                }
                let module = import.source.value.to_string();
                let class = module_class(&module)
                    .map_err(|message| analysis.error(import.source.span.start, message))?;
                import_spans.push(import.span);
                let Some(specifiers) = &import.specifiers else {
                    side_effect_imports.push((module, import.span.start));
                    continue;
                };
                if specifiers.is_empty() {
                    side_effect_imports.push((module, import.span.start));
                    continue;
                }
                for specifier in specifiers {
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
                    if class == ModuleClass::Workflow && imported.as_deref() == Some("parse") {
                        return Err(analysis.error(
                            specifier.span().start,
                            "parse comes from \"codemod/ast-grep\" now: import { parse } from \"codemod/ast-grep\"".to_string(),
                        ));
                    }
                    analysis.imports.insert(
                        local.symbol_id(),
                        ImportBinding {
                            source: module.clone(),
                            imported,
                            line,
                            at: import.span.start,
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

    // Functions to extract, in source order: forEachFile callbacks and
    // inline function steps.
    let mut sites: Vec<(Span, Isolated)> = Vec::new();
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
                let workflow_function = analysis.imports.get(&symbol).and_then(|import| {
                    is_flow_module(&import.source)
                        .then(|| import.imported.clone())
                        .flatten()
                });
                match workflow_function.as_deref() {
                    Some("forEachFile") => match call.arguments.last() {
                        Some(Argument::ArrowFunctionExpression(f)) => {
                            sites.push((f.span, Isolated::Callback))
                        }
                        Some(Argument::FunctionExpression(f)) => {
                            sites.push((f.span, Isolated::Callback))
                        }
                        Some(other) => {
                            return Err(analysis.error(
                                    other.span().start,
                                    "forEachFile's callback must be written inline as a function, so the build can extract it".to_string(),
                                ));
                        }
                        None => {
                            return Err(analysis.error(
                                call.span.start,
                                "forEachFile takes a callback, optionally after a selector"
                                    .to_string(),
                            ));
                        }
                    },
                    Some("step") => {
                        for argument in call.arguments.iter().take(2) {
                            match argument {
                                Argument::ArrowFunctionExpression(f) => {
                                    sites.push((f.span, Isolated::FunctionStep))
                                }
                                Argument::FunctionExpression(f) => {
                                    sites.push((f.span, Isolated::FunctionStep))
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Only the outermost function of a nest is extracted; using step() or
    // forEachFile inside it is reported by callback_parts.
    sites.sort_by_key(|(span, _)| span.start);
    let mut outermost: Vec<(Span, Isolated)> = Vec::new();
    for (span, kind) in sites {
        let nested = outermost
            .iter()
            .any(|(outer, _)| outer.start <= span.start && span.end <= outer.end);
        if !nested {
            outermost.push((span, kind));
        }
    }

    let mut callbacks = BTreeMap::new();
    let mut splices: Vec<(Span, String)> = Vec::new();
    for &(span, kind) in &outermost {
        let parts = analysis.callback_parts(span, kind)?;
        let callback_source = &source[span.start as usize..span.end as usize];
        let destructure = parts
            .captures
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");

        let mut module = String::new();
        for line in &parts.imports {
            module.push_str(line);
            module.push('\n');
        }
        for text in parts.hoisted.values() {
            module.push_str(text);
            module.push('\n');
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

    // Imports the body keeps: codemod/workflow, and others the body itself
    // uses. Those that only extracted code uses are left to its modules.
    let in_extracted = |at: Span| {
        outermost
            .iter()
            .any(|(span, _)| span.start <= at.start && at.end <= span.end)
    };
    let mut body_lines: Vec<String> = vec![format!(
        "import {{ __callback as __flowCallback }} from {FLOW_MODULE:?};"
    )];
    let mut body_imports: Vec<BodyImport> = Vec::new();
    let mut bindings: Vec<(&SymbolId, &ImportBinding)> = analysis.imports.iter().collect();
    bindings.sort_by_key(|(_, binding)| (binding.at, binding.line.clone()));
    for (symbol, binding) in bindings {
        let class = module_class(&binding.source).unwrap_or(ModuleClass::Package);
        if class == ModuleClass::Workflow {
            body_lines.push(binding.line.clone());
            continue;
        }
        let body_use = analysis
            .references
            .iter()
            .find(|r| r.value && r.symbol == Some(*symbol) && !in_extracted(r.span));
        let Some(body_use) = body_use else {
            continue;
        };
        if class == ModuleClass::StepsOnly {
            return Err(analysis.error(
                body_use.span.start,
                format!(
                    "uses '{}' from '{}' outside a step; it can change between runs, so use it inside step(() => ...) or a forEachFile callback",
                    analysis.name(*symbol),
                    binding.source
                ),
            ));
        }
        if class == ModuleClass::Package
            && !body_imports
                .iter()
                .any(|import| import.source == binding.source)
        {
            body_imports.push(BodyImport {
                source: binding.source.clone(),
                offset: binding.at,
            });
        }
        body_lines.push(binding.line.clone());
    }
    for (module, at) in side_effect_imports {
        match module_class(&module).unwrap_or(ModuleClass::Package) {
            ModuleClass::StepsOnly => {
                return Err(analysis.error(
                    at,
                    format!("imports '{module}' for its side effects in the workflow body; import it where a step uses it"),
                ))
            }
            ModuleClass::Package => body_imports.push(BodyImport {
                source: module.clone(),
                offset: at,
            }),
            _ => {}
        }
        body_lines.push(format!("import {module:?};"));
    }
    for span in import_spans {
        splices.push((span, String::new()));
    }

    let mut main = source.to_string();
    splices.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    for (span, replacement) in splices {
        main.replace_range(span.start as usize..span.end as usize, &replacement);
    }
    let main = format!("{}\n{main}", body_lines.join("\n"));
    let main = strip_types(file, &main).map_err(|message| error_at(file, source, 0, message))?;

    Ok(Built {
        main,
        callbacks,
        body_imports,
    })
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

    fn callback_parts(&self, callback: Span, kind: Isolated) -> Result<CallbackParts, BuildError> {
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
                        "assigns to '{name}', which is declared outside the {noun}; it runs in its own isolate, so return a value instead",
                        noun = kind.noun()
                    ),
                ));
            }
            if self.scoping.symbol_scope_id(symbol) == root {
                self.module_level(symbol, reference.span, kind, &mut parts, &mut hoist_queue)?;
                continue;
            }
            let flags = self.scoping.symbol_flags(symbol);
            if flags.intersects(SymbolFlags::Function | SymbolFlags::Class)
                || self.function_valued.contains(&symbol)
            {
                return Err(self.error(
                    reference.span.start,
                    format!(
                        "captures '{name}', a function or class declared inside a function; move it to module level so it can be copied into the {noun}",
                        noun = kind.noun()
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
                    self.module_level(inner, reference.span, kind, &mut parts, &mut hoist_queue)?;
                }
            }
        }
        Ok(parts)
    }

    fn module_level(
        &self,
        symbol: SymbolId,
        at: Span,
        kind: Isolated,
        parts: &mut CallbackParts,
        hoist_queue: &mut Vec<SymbolId>,
    ) -> Result<(), BuildError> {
        let name = self.name(symbol);
        if let Some(import) = self.imports.get(&symbol) {
            if is_flow_module(&import.source)
                && !import
                    .imported
                    .as_deref()
                    .is_some_and(|imported| ISOLATE_STATE_API.contains(&imported))
            {
                return Err(self.error(
                    at.start,
                    format!(
                        "uses '{name}' inside a {noun}; steps cannot start from inside one",
                        noun = kind.noun()
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
                "uses '{name}', which cannot be copied into the {noun}; only const, let, var, function and class declarations can",
                noun = kind.noun()
            ),
        ))
    }
}

/// Check the packages and relative files the workflow body imports
/// ([`Built::body_imports`]): following their imports must only reach
/// modules the body may use. A package that reaches `fs`, `child_process`,
/// the clock and so on can still be used inside steps and callbacks.
///
/// `file` is the workflow's path (imports resolve from its directory) and
/// `source` its TypeScript, for error positions.
pub fn check_imports(file: &Path, source: &str, built: &Built) -> Result<(), BuildError> {
    use codemod_sandbox::sandbox::resolvers::{ModuleResolver, OxcResolver};

    if built.body_imports.is_empty() {
        return Ok(());
    }
    let name = file.display().to_string();
    let base_dir = file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let resolver = OxcResolver::new(base_dir.clone(), None)
        .map_err(|e| error_at(&name, source, 0, format!("cannot resolve imports: {e}")))?;
    let mut walk = ImportWalk {
        resolver: &resolver,
        base_dir: &base_dir,
        visited: HashSet::new(),
    };
    for import in &built.body_imports {
        let at = |message: String| error_at(&name, source, import.offset, message);
        let resolved = resolver
            .resolve(&name, &import.source)
            .map_err(|e| at(format!("cannot resolve '{}': {e}", import.source)))?;
        let chain = walk.effectful(Path::new(&resolved)).map_err(at)?;
        if let Some(chain) = chain {
            let reached = chain.last().cloned().unwrap_or_default();
            return Err(at(format!(
                "the workflow body uses '{}', which imports '{reached}' ({}); the body runs again on every resume, so use '{}' inside a step or forEachFile callback",
                import.source,
                chain.join(" → "),
                import.source
            )));
        }
    }
    Ok(())
}

struct ImportWalk<'a, R> {
    resolver: &'a R,
    base_dir: &'a Path,
    visited: HashSet<PathBuf>,
}

impl<R: codemod_sandbox::sandbox::resolvers::ModuleResolver> ImportWalk<'_, R> {
    /// A file as the error shows it: inside a package as `package/file`,
    /// otherwise relative to the workflow's directory.
    fn shown(&self, path: &Path) -> String {
        let text = path.to_string_lossy().replace('\\', "/");
        if let Some((_, inside)) = text.rsplit_once("/node_modules/") {
            return inside.to_string();
        }
        let base = self
            .base_dir
            .canonicalize()
            .unwrap_or_else(|_| self.base_dir.to_path_buf());
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        canonical
            .strip_prefix(&base)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// `None` when everything `path` imports may run in the body; otherwise
    /// the chain of files that leads to a module that may not.
    fn effectful(&mut self, path: &Path) -> Result<Option<Vec<String>>, String> {
        if !self.visited.insert(path.to_path_buf()) {
            return Ok(None);
        }
        let shown = self.shown(path);
        if path.extension().is_some_and(|e| e == "json") {
            return Ok(None);
        }
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
        for specifier in module_specifiers(path, &text) {
            match module_class(&specifier) {
                Ok(ModuleClass::StepsOnly) => return Ok(Some(vec![shown, specifier])),
                Ok(ModuleClass::Pure | ModuleClass::Workflow) | Err(_) => continue,
                Ok(ModuleClass::Package) => {
                    let base = path.to_string_lossy();
                    let resolved = self
                        .resolver
                        .resolve(&base, &specifier)
                        .map_err(|e| format!("{shown}: cannot resolve '{specifier}': {e}"))?;
                    if let Some(mut chain) = self.effectful(Path::new(&resolved))? {
                        chain.insert(0, shown);
                        return Ok(Some(chain));
                    }
                }
            }
        }
        Ok(None)
    }
}

/// Every module a JavaScript or TypeScript file imports, re-exports,
/// imports dynamically (with a string) or `require`s.
fn module_specifiers(path: &Path, text: &str) -> Vec<String> {
    use oxc::ast::ast::ModuleDeclaration;

    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::mjs());
    let parsed = Parser::new(&allocator, text, source_type).parse();
    let mut specifiers = Vec::new();
    for statement in &parsed.program.body {
        let Some(declaration) = statement.as_module_declaration() else {
            continue;
        };
        match declaration {
            ModuleDeclaration::ImportDeclaration(import) if !import.import_kind.is_type() => {
                specifiers.push(import.source.value.to_string())
            }
            ModuleDeclaration::ExportNamedDeclaration(export) if !export.export_kind.is_type() => {
                if let Some(source) = &export.source {
                    specifiers.push(source.value.to_string());
                }
            }
            ModuleDeclaration::ExportAllDeclaration(export) if !export.export_kind.is_type() => {
                specifiers.push(export.source.value.to_string())
            }
            _ => {}
        }
    }
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::ImportExpression(import) => {
                if let Expression::StringLiteral(literal) = &import.source {
                    specifiers.push(literal.value.to_string());
                }
            }
            AstKind::CallExpression(call) => {
                let is_require =
                    matches!(&call.callee, Expression::Identifier(id) if id.name == "require");
                if let (true, Some(Argument::StringLiteral(literal))) =
                    (is_require, call.arguments.first())
                {
                    specifiers.push(literal.value.to_string());
                }
            }
            _ => {}
        }
    }
    specifiers
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
    /// `codemod/workflow` functions the module calls, by imported name, in
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
            if !is_flow_module(&import.source.value) {
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
