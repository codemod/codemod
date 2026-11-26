//! Module resolution for Python codemods: what a dotted or relative name
//! resolves to, where a module's name lands, which name reaches a file, and
//! which files import one.
//!
//! Names resolve as [`PyFlow`] resolves them (the workspace root, `src/`
//! under it, the directories above the importing file, `__init__.py`
//! packages and `.pyi` stubs), then in a workspace virtualenv's
//! `site-packages`. File text comes through the provider's snapshot, so
//! answers describe the workspace as it was when the step started.

use crate::flow_host::PyFlow;
use language_core::{
    ByteRange, DefinitionKind, DefinitionResult, ImportKind, Importer, ImportersFile,
    ImportersResult, SemanticError, SemanticResult, SpecifierOptions, SpecifierStyle, SymbolKind,
    SymbolLocation, UnparsedFile,
};
use language_flow::query::FlowHost;
use parking_lot::RwLock;
use rayon::prelude::*;
use ruff_python_ast::visitor::{walk_expr, walk_stmt, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::TextRange;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Virtualenv directories under the workspace root whose `site-packages`
/// are searched after the workspace.
const VENV_DIRS: &[&str] = &[".venv", "venv", "env"];

/// Calls that import the module their first argument names.
const DYNAMIC_CALLS: &[&str] = &["importlib.import_module", "import_module", "__import__"];

/// `patch` as it is usually spelled, alone or with `.dict` / `.multiple`.
const PATCH_CALLS: &[&str] = &["patch", "mock.patch", "unittest.mock.patch", "mocker.patch"];

/// pytest's `monkeypatch` calls that take a `"module.attr"` string.
const MONKEYPATCH_CALLS: &[&str] = &["monkeypatch.setattr", "monkeypatch.delattr"];

/// Every module reference in the workspace, built on the first `importers`
/// query and kept until the cache is cleared: a snapshot, as for `callers()`.
#[derive(Default)]
struct ImportersIndex {
    by_target: HashMap<PathBuf, Vec<(PathBuf, Importer)>>,
    contents: HashMap<PathBuf, Arc<str>>,
    unparsed: Vec<UnparsedFile>,
}

/// The provider's module state: the importers table.
#[derive(Default)]
pub(crate) struct PyModules {
    importers: RwLock<Option<Arc<ImportersIndex>>>,
}

impl PyModules {
    pub(crate) fn clear(&self) {
        *self.importers.write() = None;
    }
}

/// Module queries over one provider: its flow host (for resolution), its
/// module state and its snapshot reader.
pub(crate) struct Modules<'a, F> {
    flow: &'a PyFlow,
    read: F,
    root: PathBuf,
    /// Roots a module is named from: the workspace root, `src/`, then
    /// virtualenv `site-packages`.
    naming_roots: Vec<PathBuf>,
    site_packages: Vec<PathBuf>,
}

impl<'a, F> Modules<'a, F>
where
    F: Fn(&Path) -> Option<Arc<str>> + Sync,
{
    /// Module queries need the workspace: an `Unsupported` error in file scope.
    pub(crate) fn new(flow: &'a PyFlow, read: F, operation: &str) -> SemanticResult<Self> {
        let Some(root) = flow.root() else {
            return Err(SemanticError::Unsupported {
                operation: operation.to_string(),
                reason: "it needs `semanticAnalysis: \"workspace\"`".to_string(),
            });
        };
        let root = flow.canonical(root);
        let site_packages = site_packages(&root);
        let mut naming_roots = vec![root.clone()];
        let src = root.join("src");
        if src.is_dir() {
            naming_roots.push(src);
        }
        naming_roots.extend(site_packages.iter().cloned());
        Ok(Modules {
            flow,
            read,
            root,
            naming_roots,
            site_packages,
        })
    }

    /// The file `specifier` (`a.b`, `.models`, `..core.db`) resolves to from
    /// `from_file`, or None.
    pub(crate) fn resolve_module(&self, from_file: &Path, specifier: &str) -> Option<PathBuf> {
        if specifier.is_empty()
            || !specifier
                .chars()
                .all(|c| c == '.' || c == '_' || c.is_alphanumeric())
        {
            return None;
        }
        let from = self.flow.canonical(from_file);
        if let Some(found) = self.flow.module_path(&from, specifier) {
            return Some(found);
        }
        if specifier.starts_with('.') {
            return None;
        }
        let parts: Vec<&str> = specifier.split('.').filter(|p| !p.is_empty()).collect();
        self.site_packages
            .iter()
            .find_map(|root| PyFlow::module_file(root, &parts))
            .map(|found| self.flow.canonical(&found))
    }

    /// Where `name` of the module at `module_path` lands: its definition,
    /// following `from x import y` re-exports, `import a.b as c`, star
    /// imports and `__init__.py` chains; a submodule (`from pkg import mod`)
    /// lands on its file. The import itself when its module does not
    /// resolve. None when the module has no such name.
    pub(crate) fn resolve_export(
        &self,
        module_path: &Path,
        name: &str,
    ) -> Option<DefinitionResult> {
        let module = self.module_file_of(module_path)?;
        self.export_in(&module, name, &mut HashSet::new())
    }

    fn export_in(
        &self,
        module: &Path,
        name: &str,
        seen: &mut HashSet<(PathBuf, String)>,
    ) -> Option<DefinitionResult> {
        if !seen.insert((module.to_path_buf(), name.to_string())) {
            return None;
        }
        let content = (self.read)(module)?;
        let parsed = ruff_python_parser::parse_module(&content).ok();
        let bindings = parsed
            .as_ref()
            .map(|parsed| module_bindings(&parsed.syntax().body))
            .unwrap_or_default();
        if let Some(binding) = bindings.named(name) {
            let found = match &binding.what {
                What::Defined(kind) => Some(definition(
                    module,
                    &content,
                    binding.range,
                    *kind,
                    name,
                    DefinitionKind::External,
                )),
                What::From {
                    module: from,
                    name: imported,
                } => {
                    // `from . import mod`, `from pkg import mod`: the submodule.
                    let submodule = self.resolve_module(module, &join_specifier(from, imported));
                    match (submodule, self.resolve_module(module, from)) {
                        (Some(sub), _) => self.module_definition(&sub),
                        (None, Some(next)) => self.export_in(&next, imported, seen),
                        (None, None) => Some(definition(
                            module,
                            &content,
                            binding.range,
                            SymbolKind::Import,
                            name,
                            DefinitionKind::Import,
                        )),
                    }
                }
                What::Module(specifier) => match self.resolve_module(module, specifier) {
                    Some(target) => self.module_definition(&target),
                    None => Some(definition(
                        module,
                        &content,
                        binding.range,
                        SymbolKind::Import,
                        name,
                        DefinitionKind::Import,
                    )),
                },
            };
            if found.is_some() {
                return found;
            }
        }
        // `from x import *` brings in x's public names.
        if !name.starts_with('_') {
            for star in &bindings.stars {
                if let Some(next) = self.resolve_module(module, star) {
                    if let Some(found) = self.export_in(&next, name, seen) {
                        return Some(found);
                    }
                }
            }
        }
        // `from pkg import mod`: the submodule, when the package binds no `mod`.
        if is_package_init(module) {
            let dir = module.parent()?;
            if let Some(sub) = PyFlow::module_file(dir, &[name]) {
                return self.module_definition(&self.flow.canonical(&sub));
            }
        }
        None
    }

    /// A module as a definition: the whole file.
    fn module_definition(&self, module: &Path) -> Option<DefinitionResult> {
        let content = (self.read)(module)?;
        let name = self
            .dotted_names(module)
            .into_iter()
            .next()
            .unwrap_or_else(|| module_stem(module));
        let range = ByteRange::new(0, content.len() as u32);
        let mut result = DefinitionResult::new(
            SymbolLocation::new(module.to_path_buf(), range, SymbolKind::Namespace, name),
            content.to_string(),
            DefinitionKind::External,
        );
        result.export_name = None;
        Some(result)
    }

    /// A specifier `from_file` can import `target` with, checked by
    /// resolving it back. Absolute dotted names count as the alias style,
    /// relative ones (`.models`) as the relative style. Without a
    /// preference: absolute when `from_file` itself is importable from a
    /// search root, relative otherwise.
    pub(crate) fn specifier_for(
        &self,
        from_file: &Path,
        target: &Path,
        options: &SpecifierOptions,
    ) -> Option<String> {
        let from = self.flow.canonical(from_file);
        let target = self.module_file_of(target)?;
        let from_names = self.dotted_names(&from);

        let mut candidates: Vec<(String, bool)> = self
            .dotted_names(&target)
            .into_iter()
            .map(|name| (name, false))
            .collect();
        // A script run directly imports from its own directory.
        if from_names.is_empty() {
            if let Some(dir) = from.parent() {
                if let Some(name) = dotted_name(&target, dir) {
                    candidates.push((name, false));
                }
            }
        }
        if let Some(relative) = relative_specifier(&from, &target) {
            let dots = relative.chars().take_while(|c| *c == '.').count();
            // A relative import cannot climb above its top-level package.
            let allowed = match from_names.first() {
                Some(own) => {
                    let depth = own.split('.').count() - usize::from(!is_package_init(&from));
                    dots <= depth
                }
                None => from
                    .parent()
                    .is_some_and(|dir| dir.join("__init__.py").is_file()),
            };
            if allowed {
                candidates.push((relative, true));
            }
        }

        let mut seen = HashSet::new();
        candidates.retain(|(specifier, _)| {
            seen.insert(specifier.clone())
                && self.resolve_module(&from, specifier).as_ref() == Some(&target)
        });

        let like = options.like.as_deref();
        let want_relative = match (like, options.prefer) {
            (Some(like), _) => like.starts_with('.'),
            (None, Some(style)) => style == SpecifierStyle::Relative,
            (None, None) => from_names.is_empty(),
        };
        let common = |specifier: &str| {
            like.map_or(0, |like| {
                specifier
                    .chars()
                    .zip(like.chars())
                    .take_while(|(a, b)| a == b)
                    .count()
            })
        };
        candidates
            .into_iter()
            .min_by_key(|(specifier, relative)| {
                (
                    *relative != want_relative,
                    std::cmp::Reverse(common(specifier)),
                    specifier.len(),
                    specifier.clone(),
                )
            })
            .map(|(specifier, _)| specifier)
    }

    /// The dotted names other code imports `file` with from the workspace's
    /// search roots (and virtualenv `site-packages`), each checked by
    /// resolving it. Empty when the file is not importable.
    pub(crate) fn public_specifiers(&self, file: &Path) -> Vec<String> {
        let Some(file) = self.module_file_of(file) else {
            return Vec::new();
        };
        let probe = self.root.join("__probe__.py");
        let mut names: Vec<String> = self
            .dotted_names(&file)
            .into_iter()
            .filter(|name| self.resolve_module(&probe, name).as_ref() == Some(&file))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Every place in the workspace that refers to the module at `target`.
    pub(crate) fn importers(&self, target: &Path) -> ImportersResult {
        let Some(target) = self.module_file_of(target) else {
            return ImportersResult::default();
        };
        let index = self.index();
        let mut files: Vec<ImportersFile> = Vec::new();
        for (file, importer) in index.by_target.get(&target).into_iter().flatten() {
            match files.last_mut() {
                Some(last) if &last.file_path == file => last.importers.push(importer.clone()),
                _ => files.push(ImportersFile {
                    file_path: file.clone(),
                    content: index
                        .contents
                        .get(file)
                        .map(|c| c.to_string())
                        .unwrap_or_default(),
                    importers: vec![importer.clone()],
                }),
            }
        }
        ImportersResult {
            files,
            unparsed: index.unparsed.clone(),
        }
    }

    fn index(&self) -> Arc<ImportersIndex> {
        if let Some(index) = self.flow.modules.importers.read().as_ref() {
            return Arc::clone(index);
        }
        let built = Arc::new(self.build_index());
        let mut slot = self.flow.modules.importers.write();
        Arc::clone(slot.get_or_insert(built))
    }

    fn build_index(&self) -> ImportersIndex {
        let mut files = self.flow.workspace_files();
        files.sort();
        type FileRefs = (PathBuf, Arc<str>, Result<Vec<(PathBuf, Importer)>, String>);
        let per_file: Vec<FileRefs> = files
            .into_par_iter()
            .filter_map(|file| {
                let content = (self.read)(&file)?;
                let refs = ruff_python_parser::parse_module(&content)
                    .map_err(|e| e.to_string())
                    .map(|parsed| self.file_refs(&file, &parsed.syntax().body));
                Some((file, content, refs))
            })
            .collect();
        let mut index = ImportersIndex::default();
        for (file, content, refs) in per_file {
            let refs = match refs {
                Ok(refs) => refs,
                Err(message) => {
                    index.unparsed.push(UnparsedFile {
                        file_path: file,
                        message,
                    });
                    continue;
                }
            };
            if refs.is_empty() {
                continue;
            }
            for (target, importer) in refs {
                index
                    .by_target
                    .entry(target)
                    .or_default()
                    .push((file.clone(), importer));
            }
            index.contents.insert(file, content);
        }
        index
    }

    /// The module references in one file, each with the module it reaches.
    fn file_refs(&self, file: &Path, body: &[Stmt]) -> Vec<(PathBuf, Importer)> {
        let mut collector = Collector::default();
        collector.visit_body(body);
        let mut memo: HashMap<String, Option<PathBuf>> = HashMap::new();
        let mut resolve = |specifier: &str| {
            memo.entry(specifier.to_string())
                .or_insert_with(|| self.resolve_module(file, specifier))
                .clone()
        };
        let mut refs: Vec<(PathBuf, Importer)> = collector
            .refs
            .into_iter()
            .filter_map(|found| {
                let target = found.modules.iter().find_map(|m| resolve(m))?;
                Some((
                    target,
                    Importer {
                        range: found.range,
                        kind: found.kind,
                        specifier: found.specifier,
                    },
                ))
            })
            .collect();
        refs.sort_by_key(|(_, i)| (i.range.start, i.range.end));
        refs.dedup_by(|a, b| a.0 == b.0 && a.1.range == b.1.range);
        refs
    }

    /// The module file `path` names: the file, or a package directory's
    /// `__init__.py`.
    fn module_file_of(&self, path: &Path) -> Option<PathBuf> {
        let path = self.flow.canonical(path);
        if path.is_dir() {
            return PyFlow::module_file(&path, &[]).map(|init| self.flow.canonical(&init));
        }
        Some(path)
    }

    /// `file`'s dotted names from the naming roots. A root inside another
    /// (`src/` in the workspace root) names the files under it alone.
    fn dotted_names(&self, file: &Path) -> Vec<String> {
        self.naming_roots
            .iter()
            .filter(|root| {
                !self.naming_roots.iter().any(|inner| {
                    inner != *root && inner.starts_with(root) && file.starts_with(inner)
                })
            })
            .filter_map(|root| dotted_name(file, root))
            .collect()
    }
}

/// `file`'s dotted module name relative to `root`: `a/b/c.py` is `a.b.c`,
/// `a/b/__init__.py` is `a.b`. None outside `root` or when a part is not an
/// identifier.
fn dotted_name(file: &Path, root: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    let mut parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let last = parts.pop()?;
    let stem = last
        .strip_suffix(".py")
        .or_else(|| last.strip_suffix(".pyi"))?;
    if stem != "__init__" {
        parts.push(stem.to_string());
    }
    if parts.is_empty() || !parts.iter().all(|p| is_identifier(p)) {
        return None;
    }
    Some(parts.join("."))
}

/// The relative specifier from `from`'s package to `target`: `.models`,
/// `..core.db`, `.` for the package itself.
fn relative_specifier(from: &Path, target: &Path) -> Option<String> {
    let from_dir: Vec<_> = from.parent()?.components().collect();
    let module: PathBuf = if is_package_init(target) {
        target.parent()?.to_path_buf()
    } else {
        let stem = target.file_stem()?;
        target.parent()?.join(stem)
    };
    let module: Vec<_> = module.components().collect();
    let common = from_dir
        .iter()
        .zip(&module)
        .take_while(|(a, b)| a == b)
        .count();
    let dots = from_dir.len() - common + 1;
    let rest: Vec<String> = module[common..]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if !rest.iter().all(|p| is_identifier(p)) {
        return None;
    }
    Some(format!("{}{}", ".".repeat(dots), rest.join(".")))
}

/// `site-packages` of the workspace's virtualenvs.
fn site_packages(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for venv in VENV_DIRS {
        let venv = root.join(venv);
        let windows = venv.join("Lib").join("site-packages");
        if windows.is_dir() {
            found.push(windows);
        }
        let Ok(entries) = std::fs::read_dir(venv.join("lib")) else {
            continue;
        };
        let mut pythons: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("python"))
            .map(|e| e.path().join("site-packages"))
            .filter(|p| p.is_dir())
            .collect();
        pythons.sort();
        found.extend(pythons);
    }
    found
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect()
}

fn is_package_init(path: &Path) -> bool {
    path.file_stem().is_some_and(|stem| stem == "__init__")
}

fn module_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn is_identifier(part: &str) -> bool {
    let mut chars = part.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
}

fn byte_range(range: TextRange) -> ByteRange {
    ByteRange::new(range.start().to_u32(), range.end().to_u32())
}

fn definition(
    module: &Path,
    content: &str,
    range: TextRange,
    kind: SymbolKind,
    name: &str,
    definition_kind: DefinitionKind,
) -> DefinitionResult {
    let mut result = DefinitionResult::new(
        SymbolLocation::new(
            module.to_path_buf(),
            byte_range(range),
            kind,
            name.to_string(),
        ),
        content.to_string(),
        definition_kind,
    );
    result.export_name = Some(name.to_string());
    result
}

/// `module` and `name` joined as one specifier: `a` + `b` is `a.b`, `..` +
/// `b` is `..b`.
fn join_specifier(module: &str, name: &str) -> String {
    if module.is_empty() || module.ends_with('.') {
        format!("{module}{name}")
    } else {
        format!("{module}.{name}")
    }
}

// --- What a module binds at its top level -------------------------------

#[derive(Debug)]
enum What {
    /// `def`, `class`, an assignment.
    Defined(SymbolKind),
    /// `from module import name [as binding]`.
    From { module: String, name: String },
    /// `import a.b as c` (the module `a.b`), `import a.b` (binds `a`).
    Module(String),
}

#[derive(Debug)]
struct Binding {
    name: String,
    /// The binding's name as written.
    range: TextRange,
    what: What,
    /// Bound only in an `except` handler: a fallback.
    fallback: bool,
}

#[derive(Default)]
struct Bindings {
    bindings: Vec<Binding>,
    /// Modules of `from m import *`.
    stars: Vec<String>,
}

impl Bindings {
    /// The binding that holds `name` after the module runs: the last one,
    /// with `except` fallbacks only when nothing else binds it.
    fn named(&self, name: &str) -> Option<&Binding> {
        let all: Vec<&Binding> = self.bindings.iter().filter(|b| b.name == name).collect();
        all.iter()
            .rev()
            .find(|b| !b.fallback)
            .or_else(|| all.last())
            .copied()
    }
}

fn module_bindings(body: &[Stmt]) -> Bindings {
    let mut out = Bindings::default();
    collect_bindings(body, false, &mut out);
    out
}

fn collect_bindings(body: &[Stmt], fallback: bool, out: &mut Bindings) {
    let bind = |out: &mut Bindings, name: &str, range: TextRange, what: What| {
        out.bindings.push(Binding {
            name: name.to_string(),
            range,
            what,
            fallback,
        })
    };
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(def) => bind(
                out,
                def.name.as_str(),
                def.name.range,
                What::Defined(SymbolKind::Function),
            ),
            Stmt::ClassDef(class) => bind(
                out,
                class.name.as_str(),
                class.name.range,
                What::Defined(SymbolKind::Class),
            ),
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    bind_targets(target, fallback, out);
                }
            }
            Stmt::AnnAssign(assign) => bind_targets(&assign.target, fallback, out),
            Stmt::AugAssign(assign) => bind_targets(&assign.target, fallback, out),
            Stmt::TypeAlias(alias) => {
                if let Expr::Name(name) = alias.name.as_ref() {
                    bind(
                        out,
                        name.id.as_str(),
                        name.range,
                        What::Defined(SymbolKind::Type),
                    );
                }
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    let full = alias.name.as_str();
                    match &alias.asname {
                        Some(asname) => bind(
                            out,
                            asname.as_str(),
                            asname.range,
                            What::Module(full.to_string()),
                        ),
                        None => {
                            let first = full.split('.').next().unwrap_or(full);
                            bind(
                                out,
                                first,
                                alias.name.range,
                                What::Module(first.to_string()),
                            )
                        }
                    }
                }
            }
            Stmt::ImportFrom(import) => {
                let module = from_module(import);
                for alias in &import.names {
                    if alias.name.as_str() == "*" {
                        out.stars.push(module.clone());
                        continue;
                    }
                    let (name, range) = match &alias.asname {
                        Some(asname) => (asname.as_str(), asname.range),
                        None => (alias.name.as_str(), alias.name.range),
                    };
                    bind(
                        out,
                        name,
                        range,
                        What::From {
                            module: module.clone(),
                            name: alias.name.to_string(),
                        },
                    );
                }
            }
            Stmt::If(if_stmt) => {
                collect_bindings(&if_stmt.body, fallback, out);
                for clause in &if_stmt.elif_else_clauses {
                    collect_bindings(&clause.body, fallback, out);
                }
            }
            Stmt::Try(try_stmt) => {
                collect_bindings(&try_stmt.body, fallback, out);
                for handler in &try_stmt.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    collect_bindings(&handler.body, true, out);
                }
                collect_bindings(&try_stmt.orelse, fallback, out);
                collect_bindings(&try_stmt.finalbody, fallback, out);
            }
            Stmt::With(with) => collect_bindings(&with.body, fallback, out),
            Stmt::For(for_stmt) => {
                bind_targets(&for_stmt.target, fallback, out);
                collect_bindings(&for_stmt.body, fallback, out);
                collect_bindings(&for_stmt.orelse, fallback, out);
            }
            Stmt::While(while_stmt) => {
                collect_bindings(&while_stmt.body, fallback, out);
                collect_bindings(&while_stmt.orelse, fallback, out);
            }
            Stmt::Match(match_stmt) => {
                for case in &match_stmt.cases {
                    collect_bindings(&case.body, fallback, out);
                }
            }
            _ => {}
        }
    }
}

fn bind_targets(target: &Expr, fallback: bool, out: &mut Bindings) {
    match target {
        Expr::Name(name) => out.bindings.push(Binding {
            name: name.id.to_string(),
            range: name.range,
            what: What::Defined(SymbolKind::Variable),
            fallback,
        }),
        Expr::Tuple(tuple) => {
            for element in &tuple.elts {
                bind_targets(element, fallback, out);
            }
        }
        Expr::List(list) => {
            for element in &list.elts {
                bind_targets(element, fallback, out);
            }
        }
        Expr::Starred(starred) => bind_targets(&starred.value, fallback, out),
        _ => {}
    }
}

/// The module of `from <module> import …` as written: dots, then the name.
fn from_module(import: &ast::StmtImportFrom) -> String {
    format!(
        "{}{}",
        ".".repeat(import.level as usize),
        import.module.as_ref().map(|m| m.as_str()).unwrap_or("")
    )
}

// --- Module references in a file -----------------------------------------

/// One reference, with the modules it may name, best first.
struct Found {
    range: ByteRange,
    kind: ImportKind,
    specifier: String,
    modules: Vec<String>,
}

#[derive(Default)]
struct Collector {
    refs: Vec<Found>,
}

impl Collector {
    fn push(
        &mut self,
        range: TextRange,
        kind: ImportKind,
        specifier: String,
        modules: Vec<String>,
    ) {
        self.refs.push(Found {
            range: byte_range(range),
            kind,
            specifier,
            modules,
        });
    }

    fn call(&mut self, call: &ast::ExprCall) {
        let Some(callee) = dotted_expr(&call.func) else {
            return;
        };
        let Some(Expr::StringLiteral(first)) = call.arguments.args.first() else {
            return;
        };
        let written = first.value.to_str().to_string();
        if !written
            .chars()
            .all(|c| c == '.' || c == '_' || c.is_alphanumeric())
        {
            return;
        }
        if DYNAMIC_CALLS.contains(&callee.as_str()) {
            let module = if written.starts_with('.') {
                // `import_module(".b", package="a")`; `__import__` levels are not followed.
                if callee == "__import__" {
                    return;
                }
                let package = call
                    .arguments
                    .args
                    .get(1)
                    .or_else(|| {
                        call.arguments
                            .keywords
                            .iter()
                            .find(|k| k.arg.as_ref().is_some_and(|a| a.as_str() == "package"))
                            .map(|k| &k.value)
                    })
                    .and_then(|e| match e {
                        Expr::StringLiteral(s) => Some(s.value.to_str().to_string()),
                        _ => None,
                    });
                let Some(package) = package else {
                    return;
                };
                let Some(absolute) = absolutize(&written, &package) else {
                    return;
                };
                absolute
            } else {
                written.clone()
            };
            self.push(call.range, ImportKind::Dynamic, written, vec![module]);
            return;
        }
        let patch = PATCH_CALLS.iter().any(|base| {
            callee == *base
                || callee == format!("{base}.dict")
                || callee == format!("{base}.multiple")
        });
        if (patch || MONKEYPATCH_CALLS.contains(&callee.as_str())) && !written.starts_with('.') {
            // `"a.b.attr"`: the longest prefix that is a module.
            let parts: Vec<&str> = written.split('.').collect();
            let modules = (1..=parts.len())
                .rev()
                .map(|n| parts[..n].join("."))
                .collect();
            self.push(call.range, ImportKind::Mock, written, modules);
        }
    }
}

impl<'a> Visitor<'a> for Collector {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    let name = alias.name.to_string();
                    self.push(import.range, ImportKind::Import, name.clone(), vec![name]);
                }
            }
            Stmt::ImportFrom(import) => {
                let module = from_module(import);
                self.push(
                    import.range,
                    ImportKind::Import,
                    module.clone(),
                    vec![module.clone()],
                );
                // `from a import b`, `from . import b`: b may be a submodule.
                for alias in &import.names {
                    if alias.name.as_str() == "*" {
                        continue;
                    }
                    let joined = join_specifier(&module, alias.name.as_str());
                    self.push(
                        import.range,
                        ImportKind::Import,
                        joined.clone(),
                        vec![joined],
                    );
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            self.call(call);
        }
        walk_expr(self, expr);
    }
}

/// `a.b.c` for a name or attribute chain.
fn dotted_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.to_string()),
        Expr::Attribute(attribute) => Some(format!(
            "{}.{}",
            dotted_expr(&attribute.value)?,
            attribute.attr.as_str()
        )),
        _ => None,
    }
}

/// `importlib`'s relative names: `.b` in package `a` is `a.b`, `..c` in
/// `a.b` is `a.c`.
fn absolutize(relative: &str, package: &str) -> Option<String> {
    let dots = relative.chars().take_while(|c| *c == '.').count();
    let mut parts: Vec<&str> = package.split('.').filter(|p| !p.is_empty()).collect();
    if dots > parts.len() {
        return None;
    }
    parts.truncate(parts.len() + 1 - dots);
    let rest = &relative[dots..];
    if !rest.is_empty() {
        parts.push(rest);
    }
    (!parts.is_empty()).then(|| parts.join("."))
}
