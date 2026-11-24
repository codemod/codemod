//! The context an assessment shows the model, assembled before it is asked
//! so it does not spend tokens finding it:
//!
//! - **imports**: the files the file imports, followed through re-exports
//!   (`export { x } from`, `export *`, `import { a } from ...; export { a }`)
//!   to the file that defines each imported name, so a barrel `index.ts`
//!   leads to the module behind it. With a depth above 1, their imports too;
//! - **dependents**: the files of the target that import the file, directly
//!   or through re-exports;
//! - **extra** files the call names.
//!
//! Imports are read from JavaScript and TypeScript files; other languages
//! get only the file and its extra files. Paths resolve as the TypeScript
//! compiler would (relative paths, `tsconfig.json` paths, package
//! `exports`); files outside the target and in `node_modules` are left out.
//! Everything is in a fixed order, so the same files give the same context.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use codemod_sandbox::sandbox::resolvers::{ModuleResolver, OxcResolver};
use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Declaration, ExportDefaultDeclarationKind, ImportDeclarationSpecifier, ModuleDeclaration,
};
use oxc::parser::Parser;
use oxc::span::SourceType;
use serde::Deserialize;
use serde_json::Value;

use crate::run::RunContext;

/// Characters of context shown besides the file itself, unless the call
/// says otherwise.
const DEFAULT_LIMIT: usize = 100_000;
/// Dependents shown for `dependents: true`.
const DEFAULT_DEPENDENTS: usize = 10;
/// Deepest `imports` depth.
const MAX_DEPTH: u64 = 5;
/// Files the model may choose from when it asks for more context.
const MAX_CANDIDATES: usize = 20;
/// Files read for the dependents index.
const MAX_INDEXED: usize = 20_000;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContextOptions {
    /// `true` (the default: depth 1), `false`, or a depth.
    #[serde(default)]
    imports: Option<Value>,
    /// `false` (the default), `true` (up to 10), or how many.
    #[serde(default)]
    dependents: Option<Value>,
    /// Paths relative to the target.
    #[serde(default)]
    extra: Vec<String>,
    /// Characters of context besides the file.
    #[serde(default)]
    limit: Option<usize>,
}

fn count(option: &Option<Value>, name: &str, default: u64, max: u64) -> Result<u64, String> {
    match option {
        None => Ok(default),
        Some(Value::Bool(true)) => Ok(default.max(1)),
        Some(Value::Bool(false)) => Ok(0),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(n) if n <= max => Ok(n),
            _ => Err(format!(
                "assess(): context.{name} must be true, false or a whole number up to {max}"
            )),
        },
        Some(_) => Err(format!(
            "assess(): context.{name} must be true, false or a whole number"
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Section {
    Imports,
    Dependents,
    Extra,
    Requested,
}

impl Section {
    fn heading(self, file: &str) -> String {
        match self {
            Section::Imports => format!("Files {file} imports"),
            Section::Dependents => format!("Files that import {file}"),
            Section::Extra => "Other files".to_string(),
            Section::Requested => "Files asked for".to_string(),
        }
    }
}

struct Shown {
    path: String,
    text: String,
    section: Section,
    /// How the file relates to the assessed one.
    note: Option<String>,
}

/// The file, the context chosen for it, and what could be added.
pub(super) struct Assembled {
    root: PathBuf,
    overlay: Option<std::sync::Arc<crate::tasks::overlay::Overlay>>,
    path: String,
    text: String,
    shown: Vec<Shown>,
    /// Left out for the limit.
    omitted: Vec<String>,
    /// Files the model may ask to see, by path: imports one level deeper
    /// than shown, and dependents not shown.
    pool: BTreeSet<String>,
}

impl Assembled {
    /// The state the model is asked about.
    pub fn state(&self) -> String {
        let mut state = format!(
            "The file being assessed is {}. Answer about this file; the files after it are context.\n\n",
            self.path
        );
        push_file(&mut state, &self.path, None, &self.text);
        let mut sections: BTreeMap<Section, Vec<&Shown>> = BTreeMap::new();
        for shown in &self.shown {
            sections.entry(shown.section).or_default().push(shown);
        }
        for (section, files) in sections {
            state.push_str(&format!("\n## {}\n\n", section.heading(&self.path)));
            for shown in files {
                push_file(&mut state, &shown.path, shown.note.as_deref(), &shown.text);
            }
        }
        if !self.omitted.is_empty() {
            state.push_str(&format!(
                "\nNot shown, to keep the context small: {}\n",
                self.omitted.join(", ")
            ));
        }
        state
    }

    /// The assessed file and every file shown with it.
    pub fn files(&self) -> Vec<String> {
        std::iter::once(self.path.clone())
            .chain(self.shown.iter().map(|s| s.path.clone()))
            .collect()
    }

    /// Files the model may ask for, in path order.
    pub fn candidates(&self) -> Vec<String> {
        self.pool.iter().take(MAX_CANDIDATES).cloned().collect()
    }

    /// Show one more file, which the model asked for.
    pub fn request(&mut self, path: &str) -> Result<(), String> {
        self.pool.remove(path);
        let file = self.root.join(path);
        if let Some(text) = read(self.overlay.as_deref(), &file) {
            self.shown.push(Shown {
                path: path.to_string(),
                text,
                section: Section::Requested,
                note: None,
            });
        }
        Ok(())
    }
}

fn push_file(state: &mut String, path: &str, note: Option<&str>, text: &str) {
    match note {
        Some(note) => state.push_str(&format!("----- {path} ({note}) -----\n")),
        None => state.push_str(&format!("----- {path} -----\n")),
    }
    state.push_str(text);
    if !text.ends_with('\n') {
        state.push('\n');
    }
    state.push_str(&format!("----- end of {path} -----\n"));
}

fn read(overlay: Option<&crate::tasks::overlay::Overlay>, file: &Path) -> Option<String> {
    match overlay {
        Some(overlay) => overlay.read_file(file).ok(),
        None => std::fs::read_to_string(file).ok(),
    }
}

/// `path` relative to the target, with `/`; an error for one that leaves it.
fn relative_path(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    if path.is_empty()
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "assess(): '{path}' must be a path relative to the target, inside it"
        ));
    }
    Ok(candidate.to_path_buf())
}

pub(super) fn assemble(
    run: &RunContext,
    position: u64,
    path: &str,
    text: Option<String>,
    options: &ContextOptions,
    candidates: bool,
) -> Result<Assembled, String> {
    let depth = count(&options.imports, "imports", 1, MAX_DEPTH)?;
    let dependents = count(&options.dependents, "dependents", 0, 1_000)?;
    let dependents = match options.dependents {
        Some(Value::Bool(true)) => DEFAULT_DEPENDENTS,
        _ => dependents as usize,
    };
    let limit = options.limit.unwrap_or(DEFAULT_LIMIT);
    let root = dunce::canonicalize(&run.target).unwrap_or_else(|_| run.target.clone());
    let overlay = run.overlay.clone();
    let file = root.join(relative_path(path)?);
    let text = match text {
        Some(text) => text,
        None => read(overlay.as_deref(), &file)
            .ok_or_else(|| format!("assess(): cannot read '{path}' in the target"))?,
    };
    let mut graph = Graph::new(&root, overlay.as_deref());
    let mut assembled = Assembled {
        root: root.clone(),
        overlay: overlay.clone(),
        path: path.to_string(),
        text,
        shown: Vec::new(),
        omitted: Vec::new(),
        pool: BTreeSet::new(),
    };
    let mut used = 0;
    let mut seen: HashSet<PathBuf> = HashSet::from([graph.canonical(&file)]);
    let add = |assembled: &mut Assembled,
               seen: &mut HashSet<PathBuf>,
               file: &Path,
               section: Section,
               note: Option<String>,
               used: &mut usize| {
        let Some(shown_path) = graph_relative(&root, file) else {
            return;
        };
        if !seen.insert(file.to_path_buf()) {
            return;
        }
        let Some(text) = read(overlay.as_deref(), file) else {
            return;
        };
        if *used + text.len() > limit {
            assembled.omitted.push(shown_path);
            return;
        }
        *used += text.len();
        assembled.shown.push(Shown {
            path: shown_path,
            text,
            section,
            note,
        });
    };

    // Imports, breadth first: each level in path order.
    let mut level: Vec<(PathBuf, String)> = vec![(file.clone(), assembled.text.clone())];
    for current in 0..=depth {
        let mut next: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
        for (from, text) in &level {
            for (target, names) in graph.imported_definitions(from, Some(text)) {
                next.entry(target).or_default().extend(names);
            }
        }
        next.retain(|target, _| !seen.contains(target));
        if current == depth {
            // One level further than shown: what the model may ask for.
            if candidates {
                assembled
                    .pool
                    .extend(next.keys().filter_map(|t| graph_relative(&root, t)));
            }
            break;
        }
        let mut found = Vec::new();
        for (target, names) in next {
            let note = (!names.is_empty()).then(|| {
                format!(
                    "defines {}",
                    names.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            });
            add(
                &mut assembled,
                &mut seen,
                &target,
                Section::Imports,
                note,
                &mut used,
            );
            if let Some(text) = read(overlay.as_deref(), &target) {
                found.push((target, text));
            }
        }
        level = found;
    }

    if dependents > 0 || candidates {
        let index = run.assess.dependents(position, || {
            DependentsIndex::build(&root, overlay.as_deref())
        });
        let importers = index.importers(&graph.canonical(&file));
        for (shown, importer) in importers.iter().enumerate() {
            if shown < dependents {
                add(
                    &mut assembled,
                    &mut seen,
                    importer,
                    Section::Dependents,
                    None,
                    &mut used,
                );
            } else if candidates && !seen.contains(importer) {
                if let Some(path) = graph_relative(&root, importer) {
                    assembled.pool.insert(path);
                }
            }
        }
    }

    for extra in &options.extra {
        let target = root.join(relative_path(extra)?);
        if !target.is_file() && read(overlay.as_deref(), &target).is_none() {
            return Err(format!(
                "assess(): context.extra names '{extra}', which is not a file in the target"
            ));
        }
        add(
            &mut assembled,
            &mut seen,
            &target,
            Section::Extra,
            None,
            &mut used,
        );
    }
    assembled
        .pool
        .retain(|path| !assembled.shown.iter().any(|s| &s.path == path) && path != &assembled.path);
    Ok(assembled)
}

/// `file` relative to `root`, with `/`.
fn graph_relative(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn is_script(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs")
    )
}

/// What a module imports and exports, as far as following names needs.
#[derive(Debug, Default)]
struct ModuleInfo {
    /// `import ... from "specifier"`, with the names it takes.
    imports: Vec<(String, Vec<Imported>)>,
    /// Local name to where it was imported from.
    bindings: HashMap<String, (String, Imported)>,
    /// Names declared here and exported (`export function a`, `export default`).
    declared: HashSet<String>,
    /// `export { local as exported }`, by exported name.
    aliases: HashMap<String, String>,
    /// `export { imported as exported } from "specifier"`, by exported name.
    reexports: HashMap<String, (String, Imported)>,
    /// `export * from "specifier"`.
    stars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Imported {
    Name(String),
    /// The module itself: a namespace or side-effect import, `export * as`.
    Module,
}

fn parse_module(path: &Path, text: &str) -> ModuleInfo {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::mjs());
    let parsed = Parser::new(&allocator, text, source_type).parse();
    let mut info = ModuleInfo::default();
    for statement in &parsed.program.body {
        let Some(declaration) = statement.as_module_declaration() else {
            continue;
        };
        match declaration {
            ModuleDeclaration::ImportDeclaration(import) if !import.import_kind.is_type() => {
                let source = import.source.value.to_string();
                let mut names = Vec::new();
                for specifier in import.specifiers.iter().flatten() {
                    let (local, imported) = match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(s) => {
                            if s.import_kind.is_type() {
                                continue;
                            }
                            (
                                s.local.name.to_string(),
                                Imported::Name(s.imported.name().to_string()),
                            )
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                            (s.local.name.to_string(), Imported::Name("default".into()))
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                            (s.local.name.to_string(), Imported::Module)
                        }
                    };
                    info.bindings
                        .insert(local, (source.clone(), imported.clone()));
                    names.push(imported);
                }
                if names.is_empty() {
                    names.push(Imported::Module);
                }
                info.imports.push((source, names));
            }
            ModuleDeclaration::ExportNamedDeclaration(export) if !export.export_kind.is_type() => {
                match &export.source {
                    Some(source) => {
                        for specifier in &export.specifiers {
                            if specifier.export_kind.is_type() {
                                continue;
                            }
                            info.reexports.insert(
                                specifier.exported.name().to_string(),
                                (
                                    source.value.to_string(),
                                    Imported::Name(specifier.local.name().to_string()),
                                ),
                            );
                        }
                    }
                    None => {
                        for specifier in &export.specifiers {
                            info.aliases.insert(
                                specifier.exported.name().to_string(),
                                specifier.local.name().to_string(),
                            );
                        }
                        match &export.declaration {
                            Some(Declaration::VariableDeclaration(variables)) => {
                                for declarator in &variables.declarations {
                                    for binding in declarator.id.get_binding_identifiers() {
                                        info.declared.insert(binding.name.to_string());
                                    }
                                }
                            }
                            Some(declaration) => {
                                if let Some(id) = declaration.id() {
                                    info.declared.insert(id.name.to_string());
                                }
                            }
                            None => {}
                        }
                    }
                }
            }
            ModuleDeclaration::ExportDefaultDeclaration(export) => {
                match &export.declaration {
                    // `export default name`: the name may be an import.
                    ExportDefaultDeclarationKind::Identifier(id) => {
                        info.aliases.insert("default".into(), id.name.to_string());
                    }
                    _ => {
                        info.declared.insert("default".into());
                    }
                }
            }
            ModuleDeclaration::ExportAllDeclaration(export) if !export.export_kind.is_type() => {
                let source = export.source.value.to_string();
                match &export.exported {
                    Some(name) => {
                        info.reexports
                            .insert(name.name().to_string(), (source, Imported::Module));
                    }
                    None => info.stars.push(source),
                }
            }
            _ => {}
        }
    }
    info
}

/// Modules of the target, read and resolved as needed.
struct Graph<'a> {
    root: PathBuf,
    resolver: Option<OxcResolver>,
    overlay: Option<&'a crate::tasks::overlay::Overlay>,
    modules: HashMap<PathBuf, std::rc::Rc<ModuleInfo>>,
}

impl<'a> Graph<'a> {
    fn new(root: &Path, overlay: Option<&'a crate::tasks::overlay::Overlay>) -> Self {
        Self {
            root: root.to_path_buf(),
            resolver: OxcResolver::new(root.to_path_buf(), None).ok(),
            overlay,
            modules: HashMap::new(),
        }
    }

    fn canonical(&self, path: &Path) -> PathBuf {
        dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn module(&mut self, path: &Path, text: Option<&str>) -> std::rc::Rc<ModuleInfo> {
        if let Some(info) = self.modules.get(path) {
            return std::rc::Rc::clone(info);
        }
        let info = std::rc::Rc::new(match text {
            Some(text) => parse_module(path, text),
            None => read(self.overlay, path)
                .map(|text| parse_module(path, &text))
                .unwrap_or_default(),
        });
        self.modules
            .insert(path.to_path_buf(), std::rc::Rc::clone(&info));
        info
    }

    /// A specifier imported from `from`, as a file of the target outside
    /// `node_modules`.
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        let resolved = self
            .resolver
            .as_ref()?
            .resolve(&from.to_string_lossy(), specifier)
            .ok()?;
        let resolved = self.canonical(Path::new(&resolved));
        let inside = resolved.starts_with(&self.root)
            && !resolved
                .components()
                .any(|c| c.as_os_str() == "node_modules");
        (inside && is_script(&resolved)).then_some(resolved)
    }

    /// The file that defines `name` as `module` exports it, following
    /// re-exports; `None` when it cannot be found.
    fn definition(
        &mut self,
        module: &Path,
        name: &str,
        visited: &mut HashSet<(PathBuf, String)>,
    ) -> Option<PathBuf> {
        if !visited.insert((module.to_path_buf(), name.to_string())) {
            return None;
        }
        let info = self.module(module, None);
        if info.declared.contains(name) {
            return Some(module.to_path_buf());
        }
        if let Some(local) = info.aliases.get(name) {
            return match info.bindings.get(local) {
                Some((specifier, imported)) => {
                    let target = self.resolve(module, specifier)?;
                    self.follow(&target, imported, visited)
                }
                // Declared here, exported under another name.
                None => Some(module.to_path_buf()),
            };
        }
        if let Some((specifier, imported)) = info.reexports.get(name) {
            let target = self.resolve(module, specifier)?;
            return self.follow(&target, imported, visited);
        }
        for specifier in &info.stars {
            if let Some(target) = self.resolve(module, specifier) {
                if let Some(found) = self.definition(&target, name, visited) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn follow(
        &mut self,
        target: &Path,
        imported: &Imported,
        visited: &mut HashSet<(PathBuf, String)>,
    ) -> Option<PathBuf> {
        match imported {
            Imported::Module => Some(target.to_path_buf()),
            Imported::Name(name) => self
                .definition(target, name, visited)
                .or_else(|| Some(target.to_path_buf())),
        }
    }

    /// The files that define what `file` imports (and re-exports), with
    /// the names it takes from each. A module imported whole is its own
    /// definition.
    fn imported_definitions(
        &mut self,
        file: &Path,
        text: Option<&str>,
    ) -> BTreeMap<PathBuf, BTreeSet<String>> {
        let mut found: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
        if !is_script(file) {
            return found;
        }
        let info = self.module(file, text);
        let mut wanted: Vec<(String, Imported)> = Vec::new();
        for (specifier, names) in &info.imports {
            wanted.extend(names.iter().map(|n| (specifier.clone(), n.clone())));
        }
        for (specifier, imported) in info.reexports.values() {
            wanted.push((specifier.clone(), imported.clone()));
        }
        for specifier in &info.stars {
            wanted.push((specifier.clone(), Imported::Module));
        }
        for (specifier, imported) in wanted {
            let Some(target) = self.resolve(file, &specifier) else {
                continue;
            };
            let definition = self.follow(&target, &imported, &mut HashSet::new());
            let Some(definition) = definition else {
                continue;
            };
            let names = found.entry(definition).or_default();
            if let Imported::Name(name) = imported {
                names.insert(name);
            }
        }
        found
    }
}

/// Which files import which, across the target's scripts.
pub(crate) struct DependentsIndex {
    /// Definition file to the files that import from it, in path order.
    importers: HashMap<PathBuf, BTreeSet<PathBuf>>,
}

impl DependentsIndex {
    fn build(root: &Path, overlay: Option<&crate::tasks::overlay::Overlay>) -> Self {
        let mut graph = Graph::new(root, overlay);
        let mut importers: HashMap<PathBuf, BTreeSet<PathBuf>> = HashMap::new();
        let walk = ignore::WalkBuilder::new(root)
            .hidden(false)
            .filter_entry(|entry| {
                entry.file_name() != "node_modules" && entry.file_name() != ".git"
            })
            .build();
        let mut files: Vec<PathBuf> = walk
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
            .map(|entry| entry.into_path())
            .filter(|path| is_script(path))
            .take(MAX_INDEXED)
            .collect();
        files.sort();
        for file in files {
            let file = graph.canonical(&file);
            let info = graph.module(&file, None);
            // A file depends on the modules it names and on what defines the
            // names it takes from them.
            let mut targets: BTreeSet<PathBuf> = graph
                .imported_definitions(&file, None)
                .into_keys()
                .collect();
            for (specifier, _) in &info.imports {
                if let Some(target) = graph.resolve(&file, specifier) {
                    targets.insert(target);
                }
            }
            for target in targets {
                if target != file {
                    importers.entry(target).or_default().insert(file.clone());
                }
            }
        }
        Self { importers }
    }

    fn importers(&self, file: &Path) -> Vec<PathBuf> {
        self.importers
            .get(file)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../codemod-sandbox/tests/integration/semantic/fixtures/typescript/reexport_definition"
    );

    #[test]
    fn imports_are_followed_through_re_exports_to_their_definitions() {
        let root = dunce::canonicalize(FIXTURE).unwrap();
        let mut graph = Graph::new(&root, None);
        let app = root.join("packages/web/src/app.ts");
        let found = graph.imported_definitions(&app, None);
        let shown: BTreeMap<String, Vec<String>> = found
            .into_iter()
            .map(|(path, names)| {
                (
                    graph_relative(&root, &path).unwrap(),
                    names.into_iter().collect(),
                )
            })
            .collect();
        // `toMoment` and `viaImport` (re-exported from local.ts, which
        // imported it from dates.ts) are defined in dates.ts, `deepFn` in
        // deep.ts through two `export *`, and `renamed` in inner.ts.
        assert_eq!(
            shown,
            BTreeMap::from([
                (
                    "packages/shared/src/dates.ts".to_string(),
                    vec!["toMoment".to_string(), "viaImport".to_string()]
                ),
                (
                    "packages/shared/src/deep.ts".to_string(),
                    vec!["deepFn".to_string()]
                ),
                (
                    "packages/shared/src/inner.ts".to_string(),
                    vec!["renamed".to_string()]
                ),
            ])
        );
    }

    #[test]
    fn dependents_are_found_through_re_exports() {
        let root = dunce::canonicalize(FIXTURE).unwrap();
        let index = DependentsIndex::build(&root, None);
        let importers = |file: &str| -> Vec<String> {
            index
                .importers(&root.join(file))
                .iter()
                .map(|p| graph_relative(&root, p).unwrap())
                .collect()
        };
        assert_eq!(
            importers("packages/shared/src/deep.ts"),
            ["packages/shared/src/mid.ts", "packages/web/src/app.ts"]
        );
        assert_eq!(
            importers("packages/shared/src/dates.ts"),
            [
                "packages/shared/src/index.ts",
                "packages/shared/src/local.ts",
                "packages/web/src/app.ts"
            ]
        );
    }

    #[test]
    fn paths_must_stay_in_the_target() {
        assert!(relative_path("../x.ts").is_err());
        assert!(relative_path("/etc/passwd").is_err());
        assert!(relative_path("").is_err());
        assert_eq!(
            relative_path("src/a.ts").unwrap(),
            PathBuf::from("src/a.ts")
        );
    }
}
