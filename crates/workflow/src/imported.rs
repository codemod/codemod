//! What a workflow imports that the build reads without running it.
//!
//! - **Imported callbacks.** `forEachFile` takes a callback written inline or
//!   imported from a file of the package, whose default export is the same
//!   function (`import migrate from "./scripts/migrate.ts"`, then
//!   `forEachFile(selector, migrate)`); so do `shard`'s `method.function`
//!   and `filter`. The build extracts an imported callback as it does an
//!   inline one: its module imports the file, the workflow body never loads
//!   it, and its id changes with the file and the files it imports.
//! - **YAML data.** `import rules from "./rules.yaml"` gives the file's
//!   content, which the module loader reads as JSON; `astGrep` takes it.
//! - **No file paths.** A path given to `forEachFile`, `shard` or `astGrep`
//!   is a build error that says what to import instead, so a published
//!   package never misses a file it needs. `jssg` was removed: per-file
//!   logic is a callback.

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

/// Local name the Node.js host's rewrite gives `__importedCallback` from
/// `codemod/workflow`.
pub(crate) const IMPORTED_REFERENCE: &str = "__flowImported";

/// Extensions of files callbacks are imported from.
const SCRIPT_EXTENSIONS: &[&str] = &["ts", "mts", "cts", "tsx", "js", "mjs", "cjs", "jsx"];

/// Where a callback is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    ForEachFile,
    ShardFunction,
    ShardFilter,
}

impl Role {
    fn what(self) -> &'static str {
        match self {
            Role::ForEachFile => "forEachFile",
            Role::ShardFunction => "shard's method.function",
            Role::ShardFilter => "shard's filter",
        }
    }

    fn example_name(self) -> &'static str {
        match self {
            Role::ForEachFile => "migrate",
            Role::ShardFunction => "shardFiles",
            Role::ShardFilter => "keep",
        }
    }

    fn example_file(self) -> &'static str {
        match self {
            Role::ForEachFile => "./scripts/migrate.ts",
            Role::ShardFunction => "./scripts/shard.ts",
            Role::ShardFilter => "./scripts/keep.ts",
        }
    }

    fn written(self, name: &str) -> String {
        match self {
            Role::ForEachFile => format!("forEachFile(selector, {name})"),
            Role::ShardFunction => format!("shard({{ method: {{ function: {name} }} }})"),
            Role::ShardFilter => format!("shard({{ filter: {name}, ... }})"),
        }
    }

    /// Whether the engine runs it in an isolate on every host. A
    /// `forEachFile` callback runs in place on the Node.js host.
    pub fn isolated(self) -> bool {
        self != Role::ForEachFile
    }
}

/// A callback given as a default import of a file of the package.
#[derive(Debug, Clone)]
pub(crate) struct ImportedCallback {
    pub symbol: SymbolId,
    /// The import as written (`./scripts/migrate.ts`), relative to the workflow.
    pub specifier: String,
    pub role: Role,
    /// The argument, replaced by a reference to the extracted callback; for
    /// a shorthand property (`{ filter }`), the whole property.
    pub argument: Span,
    /// The property's key when the argument is a shorthand property.
    pub shorthand: Option<&'static str>,
    /// The binding's name, as written.
    pub name: String,
    /// What to cut from the import to drop only this binding: the whole
    /// declaration when it imports nothing else.
    pub cut: Span,
}

/// What the analysis found.
#[derive(Debug, Default)]
pub(crate) struct Imports {
    /// Imported callbacks, in source order.
    pub callbacks: Vec<ImportedCallback>,
    /// Files the workflow imports for the build: imported callbacks and
    /// YAML files, in source order, relative to it.
    pub files: Vec<String>,
    /// Imports to keep as imports when the workflow is bundled, as written:
    /// callbacks, whose modules import them, and YAML, which is data.
    pub externals: Vec<String>,
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
    /// The specifier, up to what follows the comma after it.
    specifier: Span,
    alone: bool,
}

struct Analyzer<'s, 'a> {
    file: &'s str,
    source: &'s str,
    semantic: &'s Semantic<'a>,
    bindings: HashMap<SymbolId, ImportedBinding>,
    found: Imports,
    /// Identifier spans of accepted callback arguments.
    accepted: Vec<Span>,
}

/// Find the callbacks and YAML a workflow imports, and check that the build
/// can read every file it names.
pub(crate) fn analyze<'a>(
    file: &str,
    source: &str,
    program: &Program<'a>,
    semantic: &Semantic<'a>,
) -> Result<Imports, BuildError> {
    let mut analyzer = Analyzer {
        file,
        source,
        semantic,
        bindings: HashMap::new(),
        found: Imports::default(),
        accepted: Vec::new(),
    };
    analyzer.collect_imports(program);
    analyzer.yaml_imports(program)?;

    let scoping = semantic.scoping();
    // The build inserts `__flowCallback` and the like as text: a binding of
    // the workflow's own by such a name would shadow them.
    if let Some(symbol) = scoping
        .symbol_ids()
        .find(|&symbol| scoping.symbol_name(symbol).starts_with("__flow"))
    {
        return Err(analyzer.error(
            scoping.symbol_span(symbol).start,
            format!(
                "rename '{}': names starting with __flow are reserved for the codemod build",
                scoping.symbol_name(symbol)
            ),
        ));
    }
    let mut flow: HashMap<SymbolId, String> = HashMap::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if import.import_kind.is_type() || !is_flow_module(&import.source.value) {
            continue;
        }
        for specifier in import.specifiers.iter().flatten() {
            let ImportDeclarationSpecifier::ImportSpecifier(s) = specifier else {
                continue;
            };
            if s.import_kind.is_type() {
                continue;
            }
            let name = s.imported.name().to_string();
            if name == "jssg" {
                return Err(analyzer.error(s.span.start, JSSG_REMOVED.to_string()));
            }
            flow.insert(s.local.symbol_id(), name);
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
            "forEachFile" => analyzer.for_each_file(call)?,
            "astGrep" => analyzer.ast_grep(call)?,
            "shard" => analyzer.shard(call)?,
            _ => {}
        }
    }
    // `selectFiles(...).filter(cb).map(cb)`: each callback, as forEachFile's.
    let select = crate::select::select_symbols(program);
    let calls = crate::select::selection_calls(semantic, &select)
        .map_err(|(at, message)| analyzer.error(at, message))?;
    for call in calls {
        if let Some(argument) = call.argument().and_then(Argument::as_expression) {
            analyzer.callback(argument, None, Role::ForEachFile)?;
        }
    }
    analyzer.check_other_uses()?;
    Ok(analyzer.found)
}

/// The build error for importing `jssg`.
const JSSG_REMOVED: &str = "jssg was removed: per-file logic is a forEachFile callback, written inline, forEachFile(selector, (file) => { ... }), or imported from a file whose default export is that function (import migrate from \"./scripts/migrate.ts\", then forEachFile(selector, migrate)). In it, parse(file) from \"codemod/ast-grep\" gives the root a jssg transform got, file.write(text) replaces what it returned, and what the callback returns is collected into the step's outputs";

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
            let values = specifiers
                .iter()
                .filter(|s| {
                    !matches!(s, ImportDeclarationSpecifier::ImportSpecifier(s) if s.import_kind.is_type())
                })
                .count();
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
                        alone: values == 1,
                    },
                );
            }
        }
    }

    /// YAML files are imported as data, by their default export: the module
    /// loader reads one as its content, as JSON.
    fn yaml_imports(&mut self, program: &Program<'a>) -> Result<(), BuildError> {
        for statement in &program.body {
            let Statement::ImportDeclaration(import) = statement else {
                continue;
            };
            let specifier = import.source.value.as_str();
            if import.import_kind.is_type() || !is_yaml(specifier) {
                continue;
            }
            if !is_relative(specifier) {
                return Err(self.error(
                    import.source.span.start,
                    format!("import YAML from the package's own files, with a relative path (\"./...\"): \"{specifier}\" would not be published"),
                ));
            }
            for value in import.specifiers.iter().flatten() {
                if !matches!(value, ImportDeclarationSpecifier::ImportDefaultSpecifier(_)) {
                    return Err(self.error(
                        value.span().start,
                        format!("a YAML file is imported as its content, its default export: import rules from \"{specifier}\""),
                    ));
                }
            }
            self.found.files.push(specifier.to_string());
            if !self.found.externals.iter().any(|e| e == specifier) {
                self.found.externals.push(specifier.to_string());
            }
        }
        Ok(())
    }

    fn for_each_file(&mut self, call: &CallExpression<'a>) -> Result<(), BuildError> {
        let Some(argument) = call.arguments.last().and_then(Argument::as_expression) else {
            return Ok(());
        };
        // A file path where the callback goes: one that names a script.
        if let Some(path) = literal_string(argument) {
            let names_script = Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| SCRIPT_EXTENSIONS.contains(&e));
            if names_script {
                return Err(self.error(
                    argument.span().start,
                    format!(
                        "forEachFile takes a callback, not a file path: {}",
                        suggestion(&path, Role::ForEachFile)
                    ),
                ));
            }
            return Ok(());
        }
        self.callback(argument, None, Role::ForEachFile).map(|_| ())
    }

    /// `astGrep` takes rules, as objects or imported YAML: a string is a
    /// file path, which the build would not include when publishing.
    fn ast_grep(&mut self, call: &CallExpression<'a>) -> Result<(), BuildError> {
        let Some(argument) = call.arguments.first().and_then(Argument::as_expression) else {
            return Err(self.error(
                call.span.start,
                "astGrep takes rules: an object ({ id, language, rule, fix }), an array of them, or a YAML file imported as import rules from \"./rules.yaml\"".to_string(),
            ));
        };
        let message = match (literal_string(argument), argument.get_inner_expression()) {
            (Some(path), _) => {
                let specifier = import_specifier(&path);
                let name = binding_name(&specifier, "rules");
                format!(
                    "astGrep takes rules, not a file path: add import {name} from \"{specifier}\" at the top of the workflow and write astGrep({name}), so the build can include the file when publishing"
                )
            }
            (None, Expression::TemplateLiteral(_)) => "astGrep takes rules, not a file path: import the YAML file (import rules from \"./rules.yaml\") and write astGrep(rules), or write the rules as objects".to_string(),
            _ => return Ok(()),
        };
        Err(self.error(argument.span().start, message))
    }

    fn shard(&mut self, call: &CallExpression<'a>) -> Result<(), BuildError> {
        let (file, source) = (self.file, self.source);
        let inline = |at: u32, what: &str| {
            error_at(
                file,
                source,
                at,
                format!("write shard's {what} inline, as an object, so the build can find the functions it runs"),
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
        let computed_key = |object: &ObjectExpression| {
            object
                .properties
                .iter()
                .find_map(|property| match property {
                    ObjectPropertyKind::ObjectProperty(p) if p.computed => Some(p.span.start),
                    _ => None,
                })
        };
        let computed = |at: u32| {
            error_at(
                file,
                source,
                at,
                "write shard's keys as names, not computed ([...]), so the build can find the functions it runs".to_string(),
            )
        };
        if let Some(at) = computed_key(options) {
            return Err(computed(at));
        }
        if let Some(jssg) = property(options, "jssg") {
            return Err(self.error(
                jssg.span.start,
                "shard's jssg was removed: to keep only the files your codemod changes, give shard a filter callback, filter: (file) => ..., which keeps the files it returns true for".to_string(),
            ));
        }
        if let Some(pattern) = property(options, "filePattern") {
            return Err(self.error(
                pattern.span.start,
                "shard's filePattern was removed: select the files with files: \"**/*.ts\", or a selector such as { include, exclude, language }".to_string(),
            ));
        }
        if let Some(method) = property(options, "method") {
            let Expression::ObjectExpression(method) = method.value.get_inner_expression() else {
                return Err(inline(method.value.span().start, "method"));
            };
            if let Some(spread) = spread(method) {
                return Err(inline(spread, "method"));
            }
            if let Some(at) = computed_key(method) {
                return Err(computed(at));
            }
            if let Some(function) = property(method, "function") {
                let shorthand = function.shorthand.then_some((function.span, "function"));
                self.callback(&function.value, shorthand, Role::ShardFunction)?;
            }
        }
        if let Some(filter) = property(options, "filter") {
            let shorthand = filter.shorthand.then_some((filter.span, "filter"));
            self.callback(&filter.value, shorthand, Role::ShardFilter)?;
        }
        Ok(())
    }

    /// Check a callback argument. Records it, and returns `true`, when it is
    /// an imported callback; `false` for one written inline, or (for
    /// `forEachFile`) anything else, which the build checks.
    fn callback(
        &mut self,
        argument: &Expression<'a>,
        shorthand: Option<(Span, &'static str)>,
        role: Role,
    ) -> Result<bool, BuildError> {
        let inner = argument.get_inner_expression();
        let at = argument.span().start;
        let what = role.what();
        let takes = || {
            format!(
                "{what} takes a function, written inline or imported from its file ({})",
                example(role)
            )
        };
        let ident = match inner {
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {
                return Ok(false)
            }
            Expression::Identifier(ident) => ident,
            _ => {
                if let Some(path) = literal_string(inner) {
                    return Err(self.error(
                        at,
                        format!(
                            "{what} takes a function, not a file path: {}",
                            suggestion(&path, role)
                        ),
                    ));
                }
                if role == Role::ForEachFile {
                    return Ok(false);
                }
                return Err(self.error(at, takes()));
            }
        };
        let symbol = self
            .semantic
            .scoping()
            .get_reference(ident.reference_id())
            .symbol_id();
        let Some((symbol, binding)) =
            symbol.and_then(|symbol| Some((symbol, self.bindings.get(&symbol)?.clone())))
        else {
            if role == Role::ForEachFile {
                return Ok(false);
            }
            return Err(self.error(at, takes()));
        };
        let name = ident.name.as_str();
        let specifier = binding.source.clone();
        match binding.imported {
            Imported::Default => {}
            Imported::Named(imported) => {
                return Err(self.error(
                    at,
                    format!(
                        "{what} takes a file's default export, and '{name}' is the export '{imported}' of \"{specifier}\": export the function as the file's default, and import it as import {name} from \"{specifier}\""
                    ),
                ));
            }
            Imported::Namespace => {
                return Err(self.error(
                    at,
                    format!(
                        "{what} takes a file's default export: import it as import {name} from \"{specifier}\", not as a namespace"
                    ),
                ));
            }
        }
        if !is_relative(&specifier) {
            return Err(self.error(
                at,
                format!(
                    "{what} takes a function from the package's own files, and \"{specifier}\" is a package: import a file of this package with a relative path (\"./...\"), or run that codemod with codemod(...)"
                ),
            ));
        }
        let extension = Path::new(&specifier)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if !SCRIPT_EXTENSIONS.contains(&extension) {
            return Err(self.error(
                at,
                format!(
                    "write the file extension in the import of '{name}' (\"{specifier}.ts\"): the build reads the file the import names"
                ),
            ));
        }
        let (argument, key) = match shorthand {
            Some((property, key)) => (property, Some(key)),
            None => (argument.span(), None),
        };
        self.accepted.push(ident.span);
        self.found.files.push(specifier.clone());
        if !self.found.externals.contains(&specifier) {
            self.found.externals.push(specifier.clone());
        }
        self.found.callbacks.push(ImportedCallback {
            symbol,
            specifier,
            role,
            argument,
            shorthand: key,
            name: name.to_string(),
            cut: if binding.alone {
                binding.declaration
            } else {
                binding.specifier
            },
        });
        Ok(true)
    }

    /// An imported callback is only given where callbacks go: the workflow
    /// does not load its code.
    fn check_other_uses(&self) -> Result<(), BuildError> {
        if self.found.callbacks.is_empty() {
            return Ok(());
        }
        let scoping = self.semantic.scoping();
        for node in self.semantic.nodes().iter() {
            let AstKind::IdentifierReference(ident) = node.kind() else {
                continue;
            };
            let reference = scoping.get_reference(ident.reference_id());
            if !reference.is_value() || self.accepted.contains(&ident.span) {
                continue;
            }
            let Some(symbol) = reference.symbol_id() else {
                continue;
            };
            let Some(callback) = self.found.callbacks.iter().find(|c| c.symbol == symbol) else {
                continue;
            };
            return Err(self.error(
                ident.span.start,
                format!(
                    "'{}' is the callback imported from \"{}\": only give it to forEachFile(...), or as shard's method.function or filter. The workflow body does not load it; it runs where callbacks run",
                    ident.name, callback.specifier
                ),
            ));
        }
        Ok(())
    }
}

/// The text of the files an imported callback's module loads: the file the
/// import names and, following their relative imports, the files it
/// imports, each after its path. The callback's id is a hash of this, so it
/// changes when any of them does. Files are read relative to `base`, the
/// workflow's directory.
pub(crate) fn imported_text(base: &Path, specifier: &str) -> Result<String, String> {
    let root = base.join(specifier);
    let mut text = String::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = vec![root.clone()];
    while let Some(path) = queue.pop() {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !visited.insert(canonical) {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            if path == root {
                format!("cannot read \"{specifier}\": {e}")
            } else {
                format!(
                    "cannot read {} (imported by \"{specifier}\"): {e}",
                    path.display()
                )
            }
        })?;
        let shown = path.strip_prefix(base).unwrap_or(&path);
        text.push_str(&shown.to_string_lossy().replace('\\', "/"));
        text.push('\n');
        text.push_str(&content);
        text.push('\n');
        let dir = path.parent().unwrap_or(base);
        for imported in crate::build::module_specifiers(&path, &content) {
            if !is_relative(&imported) {
                continue;
            }
            if let Some(found) = resolve_relative(dir, &imported) {
                queue.push(found);
            }
        }
    }
    Ok(text)
}

/// A relative import as a file: as written, with a script extension added,
/// or a directory's index.
fn resolve_relative(dir: &Path, specifier: &str) -> Option<std::path::PathBuf> {
    let path = dir.join(specifier);
    if path.is_file() {
        return Some(path);
    }
    for extension in SCRIPT_EXTENSIONS {
        let with = path.with_extension(extension);
        if with.is_file() {
            return Some(with);
        }
        let index = path.join(format!("index.{extension}"));
        if index.is_file() {
            return Some(index);
        }
    }
    None
}

fn is_yaml(specifier: &str) -> bool {
    let lower = specifier.to_ascii_lowercase();
    lower.ends_with(".yaml") || lower.ends_with(".yml")
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

/// How to import the function at `path`, as a fix for a path argument.
fn suggestion(path: &str, role: Role) -> String {
    let specifier = import_specifier(path);
    let name = binding_name(&specifier, role.example_name());
    format!(
        "export the function as the default of \"{specifier}\", add import {name} from \"{specifier}\" at the top of the workflow, and write {}",
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
    match camel_stem(specifier) {
        Some(name) if !is_reserved(&name) => name,
        _ => fallback.to_string(),
    }
}

/// A file's name in camel case (`rename-imports` for `./rename-imports.ts`
/// gives `renameImports`), when it makes the start of an identifier.
pub(crate) fn camel_stem(specifier: &str) -> Option<String> {
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
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(name)
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
            | "shard"
            | "astGrep"
            | "forEachFile"
            | "shell"
            | "agent"
            | "codemod"
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::build::{build, inspect, rewrite_for_node};

    /// A package directory with these files; the workflow's path in it.
    fn package(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let workflow = dir.path().join("workflow.ts");
        (dir, workflow)
    }

    fn file(path: &Path) -> String {
        path.display().to_string()
    }

    const WORKFLOW: &str = r#"import { step, forEachFile, shard } from "codemod/workflow";
import migrate from "./scripts/migrate.ts";
import shardFiles from "./scripts/shard.ts";

export default async function main(): Promise<void> {
  const keyword = "var";
  await step(forEachFile("src/**", migrate));
  await step(shard({ method: { function: shardFiles }, files: "src/**", filter: (file) => file.text.includes(keyword) }));
}
"#;

    fn files() -> Vec<(&'static str, &'static str)> {
        vec![
            ("scripts/migrate.ts", "import { x } from \"./helper.ts\";\nexport default (file: any) => file.write(x);\n"),
            ("scripts/helper.ts", "export const x = \"1\";\n"),
            ("scripts/shard.ts", "export default (input: any) => [];\n"),
        ]
    }

    #[test]
    fn an_imported_callback_is_extracted_and_its_module_imports_the_file() {
        let (_dir, workflow) = package(&files());
        let built = build(&file(&workflow), WORKFLOW).unwrap();
        // The body does not load the callbacks.
        assert!(!built.main.contains("./scripts/"), "{}", built.main);
        let modules: Vec<&String> = built.callbacks.values().collect();
        assert!(
            modules
                .iter()
                .any(|m| m.contains("import __imported from \"./scripts/migrate.ts\"")),
            "{modules:#?}"
        );
        assert!(
            modules
                .iter()
                .any(|m| m.contains("import __imported from \"./scripts/shard.ts\"")),
            "{modules:#?}"
        );
        // The filter, written inline, is extracted with what it captures.
        assert!(
            built.main.contains("filter: __flowCallback("),
            "{}",
            built.main
        );
        let inspection = inspect(&file(&workflow), WORKFLOW).unwrap();
        assert_eq!(
            inspection.files,
            vec!["./scripts/migrate.ts", "./scripts/shard.ts"]
        );
        assert_eq!(inspection.external_imports, inspection.files);
    }

    #[test]
    fn an_imported_callback_s_id_follows_its_file_and_the_files_it_imports() {
        let (dir, workflow) = package(&files());
        let ids = || {
            let built = build(&file(&workflow), WORKFLOW).unwrap();
            built.callbacks.keys().cloned().collect::<Vec<_>>()
        };
        let before = ids();
        assert_eq!(before, ids());
        std::fs::write(
            dir.path().join("scripts/helper.ts"),
            "export const x = \"2\";\n",
        )
        .unwrap();
        let after = ids();
        assert_ne!(before, after);
        // Only the callback that imports it changed.
        assert_eq!(
            before.iter().filter(|id| after.contains(id)).count(),
            before.len() - 1
        );
        std::fs::remove_file(dir.path().join("scripts/migrate.ts")).unwrap();
        let error = build(&file(&workflow), WORKFLOW).unwrap_err().message;
        assert!(
            error.starts_with("cannot read \"./scripts/migrate.ts\""),
            "{error}"
        );
    }

    #[test]
    fn the_node_rewrite_keeps_lines_and_runs_shard_functions_in_isolates() {
        let (_dir, workflow) = package(&files());
        let rewritten = rewrite_for_node(&file(&workflow), WORKFLOW)
            .unwrap()
            .unwrap();
        assert_eq!(
            rewritten.lines().count(),
            WORKFLOW.lines().count() + 2,
            "{rewritten}"
        );
        // forEachFile's callback runs in place, with the build's id.
        assert!(
            rewritten.contains("import migrate from \"./scripts/migrate.ts\";"),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("forEachFile(\"src/**\", __flowImported(\"cb_"),
            "{rewritten}"
        );
        // shard's run in the engine: not loaded, referenced by id.
        assert!(!rewritten.contains("import shardFiles"), "{rewritten}");
        assert!(
            rewritten.contains("method: { function: __flowCallback(\"cb_"),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("filter: __flowCallback(\"cb_"),
            "{rewritten}"
        );
        assert!(rewritten.contains("() => ({ keyword })"), "{rewritten}");
        assert!(rewritten.ends_with(
            "import { __callback as __flowCallback, __importedCallback as __flowImported } from \"codemod:workflow\";\n"
        ), "{rewritten}");
        let plain =
            "import { step } from \"codemod/workflow\";\nexport default async function main() {}\n";
        assert_eq!(rewrite_for_node(&file(&workflow), plain).unwrap(), None);
    }

    /// Imports and inline shard functions written over several lines keep
    /// their line breaks, so the lines after them do not move.
    #[test]
    fn the_node_rewrite_keeps_lines_of_code_it_replaces_over_several_lines() {
        let (_dir, workflow) = package(&files());
        let source = r#"import { step, shard } from "codemod/workflow";
import shardFiles
  from "./scripts/shard.ts";

export default async function main(): Promise<void> {
  await step(shard({
    method: { function: shardFiles },
    files: "src/**",
    filter: (file) =>
      file.text.includes("var"),
  }));
  throw new Error("line 12");
}
"#;
        let rewritten = rewrite_for_node(&file(&workflow), source).unwrap().unwrap();
        let line = |text: &str| text.lines().position(|l| l.contains("line 12"));
        assert_eq!(line(&rewritten), line(source), "{rewritten}");
        assert!(!rewritten.contains("import shardFiles"), "{rewritten}");
        assert!(
            rewritten.contains("filter: __flowCallback(\"cb_"),
            "{rewritten}"
        );
    }

    /// `shard`'s keys may be quoted, as in any object; computed keys are
    /// refused, so the build finds every function it runs.
    #[test]
    fn shard_options_with_quoted_keys_are_extracted_and_computed_keys_refused() {
        let (_dir, workflow) = package(&files());
        let quoted = r#"import { step, shard } from "codemod/workflow";
export default async function main() {
  await step(shard({ "method": { "function": (input) => [] }, "filter": (file) => true }));
}
"#;
        let built = build(&file(&workflow), quoted).unwrap();
        assert_eq!(built.callbacks.len(), 2, "{}", built.main);
        assert!(
            built
                .main
                .contains("\"method\": { \"function\": __flowCallback(\"cb_"),
            "{}",
            built.main
        );
        for computed in [
            r#"shard({ method: { ["function"]: shardFiles } })"#,
            r#"shard({ ["method"]: { function: shardFiles } })"#,
            r#"shard({ method: { type: "directory", maxFilesPerShard: 1 }, ["filter"]: (file) => true })"#,
        ] {
            let source = format!(
                "import {{ step, shard }} from \"codemod/workflow\";\nimport shardFiles from \"./scripts/shard.ts\";\nexport default async function main() {{ await step({computed}); }}\n"
            );
            let error = build(&file(&workflow), &source).unwrap_err().message;
            assert!(
                error.contains("write shard's keys as names, not computed"),
                "{computed}: {error}"
            );
        }
    }

    /// The build inserts names starting with `__flow`; the workflow may not
    /// declare them.
    #[test]
    fn names_the_build_inserts_are_reserved() {
        let (_dir, workflow) = package(&files());
        let source = r#"import { step, forEachFile } from "codemod/workflow";
import migrate from "./scripts/migrate.ts";
export default async function main() {
  const __flowImported = 0;
  await step(forEachFile("src/**", migrate));
}
"#;
        let error = build(&file(&workflow), source).unwrap_err().message;
        assert!(
            error.contains(
                "'__flowImported': names starting with __flow are reserved for the codemod build"
            ),
            "{error}"
        );
    }

    #[test]
    fn yaml_is_imported_as_data_and_astgrep_takes_no_paths() {
        let source = r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
import rules from "./rules/config.yml";
export default async function main() {
  await step(forEachFile(astGrep(rules)));
}
"#;
        let built = build("workflow.ts", source).unwrap();
        assert!(
            built
                .main
                .contains("import rules from \"./rules/config.yml\";"),
            "{}",
            built.main
        );
        let inspection = inspect("workflow.ts", source).unwrap();
        assert_eq!(inspection.files, vec!["./rules/config.yml"]);
        let error = build(
            "workflow.ts",
            "import { step, forEachFile, astGrep } from \"codemod/workflow\";\nexport default async function main() { await step(forEachFile(astGrep(\"rules/config.yml\"))); }\n",
        )
        .unwrap_err()
        .message;
        assert_eq!(
            error,
            "astGrep takes rules, not a file path: add import config from \"./rules/config.yml\" at the top of the workflow and write astGrep(config), so the build can include the file when publishing"
        );
    }
}
