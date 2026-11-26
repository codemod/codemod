//! Built-in entry points for Python: functions packaging metadata, a
//! `__main__` guard or a framework calls from outside the workspace's code.
//!
//! Framework knowledge is data ([`frameworks`]): which constructors,
//! decorators and registration calls mark an entry point. A name only
//! counts when the file's imports resolve it to the framework's own module
//! (`from fastapi import APIRouter`), never by its spelling alone.
//!
//! The whole workspace is scanned once (per provider, until a file changes)
//! into an index from a function's name position to its entry points, so
//! asking about one function is a lookup.

mod django;
pub(crate) mod frameworks;
mod packaging;
mod summary;

use frameworks::{Framework, Join, Label, Marker, Method, FRAMEWORKS};
use language_core::{EntryPoint, FileSnapshot};
use language_flow::query::FlowHost;
use language_flow::FileFlow;
use parking_lot::{Mutex, RwLock};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use summary::{Binding, Ex, FileSummary};

/// Entry points by file and the position where the function's name starts.
type Index = HashMap<(PathBuf, u32), Vec<EntryPoint>>;

/// A file's text and its summary (None when it does not parse).
type Summarized = (Arc<str>, Option<Arc<FileSummary>>);

/// Built-in entry points, scanned once per workspace state.
#[derive(Default)]
pub(crate) struct BuiltinEntryPoints {
    summaries: RwLock<HashMap<PathBuf, Summarized>>,
    index: RwLock<Option<Arc<Index>>>,
    building: Mutex<()>,
    /// Bumped by every change, so a scan that raced one is not kept.
    generation: AtomicU64,
    /// `pyproject.toml` and `setup.cfg`, pinned on first read like the
    /// provider pins source files.
    manifests: FileSnapshot,
}

impl BuiltinEntryPoints {
    /// A file changed: the next question rescans (unchanged files keep
    /// their summaries).
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.index.write() = None;
    }

    pub fn clear(&self) {
        self.invalidate();
        self.summaries.write().clear();
        self.manifests.clear();
    }

    /// The built-in entry points of function `function` of `path`.
    pub fn of(
        &self,
        host: &dyn FlowHost,
        root: Option<&Path>,
        path: &Path,
        flow: &FileFlow,
        function: u32,
    ) -> Vec<EntryPoint> {
        let Some(start) = flow
            .functions
            .get(function as usize)
            .and_then(|f| f.name_range)
            .map(|r| r.start)
        else {
            return Vec::new();
        };
        self.index(host, root)
            .get(&(path.to_path_buf(), start))
            .cloned()
            .unwrap_or_default()
    }

    fn index(&self, host: &dyn FlowHost, root: Option<&Path>) -> Arc<Index> {
        if let Some(index) = self.index.read().clone() {
            return index;
        }
        let _building = self.building.lock();
        if let Some(index) = self.index.read().clone() {
            return index;
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let index = Arc::new(Build::new(self, host, root).run());
        let mut slot = self.index.write();
        if self.generation.load(Ordering::SeqCst) == generation {
            *slot = Some(Arc::clone(&index));
        }
        index
    }

    fn summary(&self, host: &dyn FlowHost, path: &Path) -> Option<Arc<FileSummary>> {
        let content = host.content(path)?;
        if let Some((text, summary)) = self.summaries.read().get(path) {
            if Arc::ptr_eq(text, &content) || **text == *content {
                return summary.clone();
            }
        }
        let summary = FileSummary::parse(&content).map(Arc::new);
        self.summaries
            .write()
            .insert(path.to_path_buf(), (content, summary.clone()));
        summary
    }
}

/// What a name or expression stands for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Val {
    /// Imported from outside the workspace, fully qualified: `fastapi.APIRouter`.
    External(String),
    Module(PathBuf),
    /// A function or class: the file and its index in the file's summary.
    Def(PathBuf, usize),
    /// A value bound to a name, other than an alias: file, scope, name.
    Bound(PathBuf, usize, String),
    Unknown,
}

/// A piece of a route or command: known text, or the expression it could
/// not be read from.
#[derive(Debug, Clone)]
enum Piece {
    Known(String),
    Unread(String),
}

impl Piece {
    fn render(&self) -> String {
        match self {
            Piece::Known(text) => text.clone(),
            Piece::Unread(text) => format!("<{text}>"),
        }
    }
}

/// A receiver: something whose methods register entry points (`app`,
/// `router`, a Click group).
struct Receiver {
    framework: &'static Framework,
    own: Option<Piece>,
    /// Receivers it is mounted under, with the prefix given there and
    /// whether that prefix replaces its own.
    parents: Vec<(Val, Option<Piece>, bool)>,
}

const DEPTH: u32 = 24;

struct Build<'a> {
    owner: &'a BuiltinEntryPoints,
    host: &'a dyn FlowHost,
    root: Option<PathBuf>,
    files: Vec<PathBuf>,
    modules: RefCell<HashMap<(PathBuf, String), Option<PathBuf>>>,
    receivers: HashMap<Val, Receiver>,
    out: HashMap<(PathBuf, u32), BTreeSet<EntryPoint>>,
}

impl<'a> Build<'a> {
    fn new(owner: &'a BuiltinEntryPoints, host: &'a dyn FlowHost, root: Option<&Path>) -> Self {
        Build {
            owner,
            host,
            root: root.map(|r| host.canonical(r)),
            files: host.workspace_files(),
            modules: RefCell::default(),
            receivers: HashMap::new(),
            out: HashMap::new(),
        }
    }

    fn run(mut self) -> Index {
        self.find_receivers();
        self.framework_decorators();
        self.framework_registrations();
        self.dependencies();
        self.main_modules();
        self.overrides();
        django::run(&mut self);
        packaging::run(&mut self);
        self.out
            .into_iter()
            .map(|(key, found)| (key, found.into_iter().collect()))
            .collect()
    }

    fn summary(&self, path: &Path) -> Option<Arc<FileSummary>> {
        self.owner.summary(self.host, path)
    }

    fn emit(&mut self, path: &Path, def: usize, kind: &str, label: String) {
        let Some(summary) = self.summary(path) else {
            return;
        };
        let start = summary.defs[def].name_start;
        self.out
            .entry((path.to_path_buf(), start))
            .or_default()
            .insert(EntryPoint {
                kind: kind.to_string(),
                label,
            });
    }

    /// Every (file, summary) in the workspace.
    fn all(&self) -> Vec<(PathBuf, Arc<FileSummary>)> {
        self.files
            .iter()
            .filter_map(|path| Some((path.clone(), self.summary(path)?)))
            .collect()
    }

    // ---- resolution ----------------------------------------------------

    /// The workspace file a module specifier names, seen from `from`.
    ///
    /// Absolute names resolve the way Python's `sys.path` does: against
    /// the workspace root, its `src/`, and the directory holding `from`'s
    /// top-level package. Not against the package's own directories, where
    /// `shopfloor/celery.py` would shadow the `celery` library.
    fn module_path(&self, from: &Path, spec: &str) -> Option<PathBuf> {
        let key = (
            from.parent().map(Path::to_path_buf).unwrap_or_default(),
            spec.to_string(),
        );
        if let Some(found) = self.modules.borrow().get(&key) {
            return found.clone();
        }
        let found = if spec.starts_with('.') {
            self.host.module_path(from, spec)
        } else {
            let parts: Vec<&str> = spec.split('.').filter(|p| !p.is_empty()).collect();
            let mut bases: Vec<PathBuf> = Vec::new();
            if let Some(root) = &self.root {
                bases.push(root.clone());
                bases.push(root.join("src"));
            }
            let mut dir = from.parent();
            while let Some(current) = dir {
                if !current.join("__init__.py").is_file() {
                    bases.push(current.to_path_buf());
                    break;
                }
                dir = current.parent();
            }
            bases
                .iter()
                .find_map(|base| module_file(base, &parts))
                .map(|file| self.host.canonical(&file))
        };
        self.modules.borrow_mut().insert(key, found.clone());
        found
    }

    fn module_val(&self, from: &Path, spec: &str) -> Val {
        match self.module_path(from, spec) {
            Some(path) => Val::Module(path),
            None if spec.starts_with('.') => Val::Unknown,
            None => Val::External(spec.to_string()),
        }
    }

    fn resolve(&self, path: &Path, scope: usize, ex: &Ex, depth: u32) -> Val {
        if depth > DEPTH {
            return Val::Unknown;
        }
        match ex {
            Ex::Ref(parts) => {
                let mut val = self.lookup(path, scope, &parts[0], depth + 1);
                for part in &parts[1..] {
                    val = self.attr(val, part, depth + 1);
                }
                val
            }
            Ex::Attr(value, name) => {
                let value = self.resolve(path, scope, value, depth + 1);
                self.attr(value, name, depth + 1)
            }
            _ => Val::Unknown,
        }
    }

    /// `name` as seen from `scope`: Python's rule, where class bodies are
    /// not visible from functions nested in them.
    fn lookup(&self, path: &Path, scope: usize, name: &str, depth: u32) -> Val {
        let Some(summary) = self.summary(path) else {
            return Val::Unknown;
        };
        let mut current = Some(scope);
        let mut first = true;
        while let Some(index) = current {
            let found = &summary.scopes[index];
            if first || !found.is_class {
                if let Some(binding) = found.names.get(name) {
                    return self.binding_val(path, index, name, binding, depth);
                }
            }
            first = false;
            current = found.parent;
        }
        Val::Unknown
    }

    fn binding_val(
        &self,
        path: &Path,
        scope: usize,
        name: &str,
        binding: &Binding,
        depth: u32,
    ) -> Val {
        if depth > DEPTH {
            return Val::Unknown;
        }
        match binding {
            Binding::Import { module, name: None } => self.module_val(path, module),
            Binding::Import {
                module,
                name: Some(member),
            } => self.member(path, module, member, depth + 1),
            Binding::Def(index) => Val::Def(path.to_path_buf(), *index),
            Binding::Value(ex @ (Ex::Ref(_) | Ex::Attr(..))) => {
                match self.resolve(path, scope, ex, depth + 1) {
                    Val::Unknown => Val::Bound(path.to_path_buf(), scope, name.to_string()),
                    found => found,
                }
            }
            Binding::Value(_) => Val::Bound(path.to_path_buf(), scope, name.to_string()),
        }
    }

    /// `from module import member`, seen from `from`.
    fn member(&self, from: &Path, module: &str, member: &str, depth: u32) -> Val {
        match self.module_val(from, module) {
            Val::Module(target) => {
                let found = self.module_attr(&target, member, depth);
                if found != Val::Unknown {
                    return found;
                }
                let spec = if module.ends_with('.') {
                    format!("{module}{member}")
                } else {
                    format!("{module}.{member}")
                };
                match self.module_path(from, &spec) {
                    Some(path) => Val::Module(path),
                    None => Val::Unknown,
                }
            }
            Val::External(module) => Val::External(format!("{module}.{member}")),
            _ => Val::Unknown,
        }
    }

    /// A name a workspace module defines or imports, else its submodule.
    fn module_attr(&self, module: &Path, name: &str, depth: u32) -> Val {
        if let Some(summary) = self.summary(module) {
            if let Some(binding) = summary.scopes[0].names.get(name) {
                return self.binding_val(module, 0, name, binding, depth + 1);
            }
        }
        if module.file_name().is_some_and(|f| f == "__init__.py") {
            let dir = module.parent().unwrap_or(Path::new(""));
            for candidate in [
                dir.join(format!("{name}.py")),
                dir.join(name).join("__init__.py"),
            ] {
                if candidate.is_file() {
                    return Val::Module(self.host.canonical(&candidate));
                }
            }
        }
        Val::Unknown
    }

    fn attr(&self, value: Val, name: &str, depth: u32) -> Val {
        if depth > DEPTH {
            return Val::Unknown;
        }
        match value {
            Val::External(qualified) => Val::External(format!("{qualified}.{name}")),
            Val::Module(module) => self.module_attr(&module, name, depth),
            Val::Def(path, index) => self.class_attr(&path, index, name, depth),
            Val::Bound(path, scope, bound) => {
                // An instance of a workspace class: the class's attribute.
                let Some(Ex::Call { func, .. }) = self.bound_ex(&path, scope, &bound) else {
                    return Val::Unknown;
                };
                match self.resolve(&path, scope, &func, depth + 1) {
                    Val::Def(class_path, class) => {
                        self.class_attr(&class_path, class, name, depth + 1)
                    }
                    _ => Val::Unknown,
                }
            }
            Val::Unknown => Val::Unknown,
        }
    }

    fn class_attr(&self, path: &Path, class: usize, name: &str, depth: u32) -> Val {
        let Some(summary) = self.summary(path) else {
            return Val::Unknown;
        };
        let def = &summary.defs[class];
        if !def.is_class {
            return Val::Unknown;
        }
        match summary.scopes[def.body].names.get(name) {
            Some(binding) => self.binding_val(path, def.body, name, binding, depth + 1),
            None => Val::Unknown,
        }
    }

    /// The expression a [`Val::Bound`] name is bound to.
    fn bound_ex(&self, path: &Path, scope: usize, name: &str) -> Option<Ex> {
        let summary = self.summary(path)?;
        match summary.scopes.get(scope)?.names.get(name)? {
            Binding::Value(ex) => Some(ex.clone()),
            _ => None,
        }
    }

    /// The text of a string expression, following names to their values.
    fn eval_str(&self, path: &Path, scope: usize, ex: &Ex, depth: u32) -> Option<String> {
        if depth > DEPTH {
            return None;
        }
        match ex {
            Ex::Str(text) => Some(text.clone()),
            Ex::Add(a, b) => Some(
                self.eval_str(path, scope, a, depth + 1)?
                    + &self.eval_str(path, scope, b, depth + 1)?,
            ),
            Ex::Ref(_) | Ex::Attr(..) => match self.resolve(path, scope, ex, depth + 1) {
                Val::Bound(target, target_scope, name) => {
                    let value = self.bound_ex(&target, target_scope, &name)?;
                    self.eval_str(&target, target_scope, &value, depth + 1)
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn piece(&self, path: &Path, scope: usize, ex: &Ex) -> Piece {
        match self.eval_str(path, scope, ex, 0) {
            Some(text) => Piece::Known(text),
            None => Piece::Unread(ex.text()),
        }
    }

    fn external(&self, path: &Path, scope: usize, ex: &Ex) -> Option<String> {
        match self.resolve(path, scope, ex, 0) {
            Val::External(name) => Some(name),
            _ => None,
        }
    }

    /// A function the expression names (a `Def` that is not a class).
    fn function(&self, val: &Val) -> Option<(PathBuf, usize)> {
        let Val::Def(path, index) = val else {
            return None;
        };
        let summary = self.summary(path)?;
        (!summary.defs[*index].is_class).then(|| (path.clone(), *index))
    }

    /// A class's own methods and those of its workspace bases, nearest first.
    fn methods(&self, path: &Path, class: usize) -> Vec<(String, PathBuf, usize)> {
        let mut found: Vec<(String, PathBuf, usize)> = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(path.to_path_buf(), class)];
        while let Some((path, class)) = stack.pop() {
            if !seen.insert((path.clone(), class)) || seen.len() > 64 {
                continue;
            }
            let Some(summary) = self.summary(&path) else {
                continue;
            };
            let def = &summary.defs[class];
            for &method in &def.methods {
                let name = &summary.defs[method].name;
                if !found.iter().any(|(n, ..)| n == name) {
                    found.push((name.clone(), path.clone(), method));
                }
            }
            // Leftmost base first, as Python's method order mostly is.
            for base in def.bases.iter().rev() {
                if let Val::Def(base_path, base) = self.resolve(&path, def.scope, base, 0) {
                    stack.push((base_path, base));
                }
            }
        }
        found
    }

    /// Every class the workspace class inherits from that is outside the
    /// workspace, fully qualified.
    fn external_bases(&self, path: &Path, class: usize) -> Vec<String> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(path.to_path_buf(), class)];
        while let Some((path, class)) = stack.pop() {
            if !seen.insert((path.clone(), class)) || seen.len() > 64 {
                continue;
            }
            let Some(summary) = self.summary(&path) else {
                continue;
            };
            let def = &summary.defs[class];
            for base in &def.bases {
                match self.resolve(&path, def.scope, base, 0) {
                    Val::External(name) => found.push(name),
                    Val::Def(base_path, base) => stack.push((base_path, base)),
                    _ => {}
                }
            }
        }
        found
    }

    fn inherits(&self, path: &Path, class: usize, prefixes: &[&str]) -> bool {
        self.external_bases(path, class)
            .iter()
            .any(|name| prefixes.iter().any(|p| name.starts_with(p)))
    }

    /// The module a file is imported as: its packages (directories with
    /// `__init__.py`) and its name.
    fn module_name(&self, path: &Path) -> String {
        let mut parts = Vec::new();
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string());
        if let Some(stem) = stem.filter(|s| s != "__init__") {
            parts.push(stem);
        }
        let mut dir = path.parent();
        while let Some(current) = dir {
            if !current.join("__init__.py").is_file() {
                break;
            }
            match current.file_name() {
                Some(name) => parts.push(name.to_string_lossy().to_string()),
                None => break,
            }
            dir = current.parent();
        }
        parts.reverse();
        parts.join(".")
    }

    // ---- frameworks ----------------------------------------------------

    /// Receivers made by a framework constructor: `router = APIRouter(...)`.
    fn find_receivers(&mut self) {
        for (path, summary) in self.all() {
            for (scope_index, scope) in summary.scopes.iter().enumerate() {
                for (name, binding) in &scope.names {
                    let Binding::Value(call @ Ex::Call { func, .. }) = binding else {
                        continue;
                    };
                    let Some(constructor) = self.external(&path, scope_index, func) else {
                        continue;
                    };
                    for framework in FRAMEWORKS {
                        let Some((_, arg, kw)) =
                            framework.receivers.iter().find(|(c, ..)| *c == constructor)
                        else {
                            continue;
                        };
                        let own = arg
                            .and_then(|i| call.arg(i))
                            .or_else(|| call.kwarg(kw))
                            .map(|ex| self.piece(&path, scope_index, ex));
                        self.receivers.insert(
                            Val::Bound(path.clone(), scope_index, name.clone()),
                            Receiver {
                                framework,
                                own,
                                parents: Vec::new(),
                            },
                        );
                    }
                }
            }
        }
        // Functions that decorators make receivers: `@click.group()`, and
        // `@cli.group()` under a group found in the round before.
        for _ in 0..4 {
            let mut added = false;
            for (path, summary) in self.all() {
                for (index, def) in summary.defs.iter().enumerate() {
                    let key = Val::Def(path.clone(), index);
                    if self.receivers.contains_key(&key) {
                        continue;
                    }
                    for decorator in &def.decorators {
                        let found = self.decorator_receiver(&path, def.scope, decorator);
                        if let Some((framework, parent)) = found {
                            let own = Some(command_name(decorator, &def.name));
                            let parents = parent.into_iter().map(|p| (p, None, false)).collect();
                            self.receivers.insert(
                                key.clone(),
                                Receiver {
                                    framework,
                                    own,
                                    parents,
                                },
                            );
                            added = true;
                            break;
                        }
                    }
                }
            }
            if !added {
                break;
            }
        }
        self.mounts();
    }

    /// The framework (and parent receiver) of a decorator that makes the
    /// function a receiver.
    fn decorator_receiver(
        &self,
        path: &Path,
        scope: usize,
        decorator: &Ex,
    ) -> Option<(&'static Framework, Option<Val>)> {
        let callee = decorator.callee();
        if let Some(name) = self.external(path, scope, callee) {
            let framework = FRAMEWORKS
                .iter()
                .find(|f| f.receiver_decorators.contains(&name.as_str()))?;
            return Some((framework, None));
        }
        let (object, method) = split_method(callee)?;
        let parent = self.resolve(path, scope, &object, 0);
        let framework = self.receivers.get(&parent)?.framework;
        framework
            .receiver_methods
            .contains(&method.as_str())
            .then_some((framework, Some(parent)))
    }

    /// `parent.include_router(child, prefix="/x")` and the like.
    fn mounts(&mut self) {
        let mut edges = Vec::new();
        for (path, summary) in self.all() {
            for (scope, call) in &summary.statement_calls {
                let Some((object, method)) = split_method(call.callee()) else {
                    continue;
                };
                let parent = self.resolve(&path, *scope, &object, 0);
                let Some(receiver) = self.receivers.get(&parent) else {
                    continue;
                };
                let Some(mount) = receiver
                    .framework
                    .mounts
                    .iter()
                    .find(|m| m.names.contains(&method.as_str()))
                else {
                    continue;
                };
                let Some(child) = call.arg(0) else {
                    continue;
                };
                let child = self.resolve(&path, *scope, child, 0);
                let prefix = call
                    .kwarg(mount.prefix_kw)
                    .map(|ex| self.piece(&path, *scope, ex));
                edges.push((child, parent, prefix, mount.replaces_own));
            }
        }
        for (child, parent, prefix, replaces) in edges {
            if let Some(receiver) = self.receivers.get_mut(&child) {
                receiver.parents.push((parent, prefix, replaces));
            }
        }
    }

    /// The pieces a receiver's routes or commands start with, once for
    /// each way it is mounted.
    fn receiver_paths(&self, receiver: &Val, seen: &mut Vec<Val>) -> Vec<Vec<Piece>> {
        let Some(found) = self.receivers.get(receiver) else {
            return vec![Vec::new()];
        };
        if seen.contains(receiver) || seen.len() > 16 {
            return vec![found.own.iter().cloned().collect()];
        }
        if found.parents.is_empty() {
            return vec![found.own.iter().cloned().collect()];
        }
        seen.push(receiver.clone());
        let mut paths = Vec::new();
        for (parent, prefix, replaces) in &found.parents {
            for mut path in self.receiver_paths(parent, seen) {
                match (prefix, replaces) {
                    (Some(prefix), true) => path.push(prefix.clone()),
                    (prefix, _) => {
                        path.extend(prefix.iter().cloned());
                        path.extend(found.own.iter().cloned());
                    }
                }
                paths.push(path);
            }
        }
        seen.pop();
        paths
    }

    fn framework_decorators(&mut self) {
        for (path, summary) in self.all() {
            for (index, def) in summary.defs.iter().enumerate() {
                for decorator in &def.decorators {
                    let callee = decorator.callee();
                    if let Some(name) = self.external(&path, def.scope, callee) {
                        let imported_from = |f: &&&Framework| {
                            f.modules.iter().any(|m| name.starts_with(&format!("{m}.")))
                        };
                        for framework in FRAMEWORKS.iter().filter(imported_from) {
                            for marker in framework.decorators {
                                if marker.names.contains(&name.as_str()) {
                                    let short = name.rsplit('.').next().unwrap_or(&name);
                                    let use_ = Use {
                                        path: &path,
                                        scope: def.scope,
                                        call: decorator,
                                        method: short,
                                        receiver: None,
                                        framework,
                                    };
                                    self.mark(&use_, marker, (&path, index));
                                }
                            }
                        }
                        continue;
                    }
                    let Some((object, method)) = split_method(callee) else {
                        continue;
                    };
                    let receiver = self.resolve(&path, def.scope, &object, 0);
                    let Some(framework) = self.receivers.get(&receiver).map(|r| r.framework) else {
                        continue;
                    };
                    for marker in framework.methods {
                        if marker.names.contains(&method.as_str()) {
                            let use_ = Use {
                                path: &path,
                                scope: def.scope,
                                call: decorator,
                                method: &method,
                                receiver: Some(&receiver),
                                framework,
                            };
                            self.mark(&use_, marker, (&path, index));
                        }
                    }
                }
            }
        }
    }

    /// `app.add_api_route("/x", handler)`, `post_save.connect(handler)`.
    fn framework_registrations(&mut self) {
        for (path, summary) in self.all() {
            for (scope, call) in &summary.statement_calls {
                let Some((object, method)) = split_method(call.callee()) else {
                    continue;
                };
                let receiver = self.resolve(&path, *scope, &object, 0);
                let framework = match self.receivers.get(&receiver) {
                    Some(found) => found.framework,
                    None => match &receiver {
                        Val::External(name)
                            if frameworks::DJANGO_BUILTIN_SIGNALS
                                .iter()
                                .any(|p| name.starts_with(p)) =>
                        {
                            &frameworks::DJANGO_SIGNALS
                        }
                        _ => continue,
                    },
                };
                for registration in framework.registrations {
                    if !registration.marker.names.contains(&method.as_str()) {
                        continue;
                    }
                    let Some(target) =
                        call.param(registration.function_arg, registration.function_kw)
                    else {
                        continue;
                    };
                    let use_ = Use {
                        path: &path,
                        scope: *scope,
                        call,
                        method: &method,
                        receiver: Some(&receiver),
                        framework,
                    };
                    // `add_url_rule("/x", view_func=View.as_view("x"))`.
                    if let Some(class) = self.as_view_class(&path, *scope, target) {
                        let Label::Route { .. } = registration.marker.label else {
                            continue;
                        };
                        let paths = self.route_paths(&use_, &registration.marker.label);
                        for handler in self.view_handlers(&class.0, class.1) {
                            for route in &paths {
                                let label = match &handler.0 {
                                    Some(method) => format!("{method} {route}"),
                                    None => format!("path {route}"),
                                };
                                self.emit(&handler.1, handler.2, registration.marker.kind, label);
                            }
                        }
                        continue;
                    }
                    let target = self.resolve(&path, *scope, target, 0);
                    if let Some(function) = self.function(&target) {
                        self.mark(&use_, &registration.marker, (&function.0, function.1));
                    }
                }
            }
        }
    }

    /// The class of `Class.as_view(...)`.
    fn as_view_class(&self, path: &Path, scope: usize, ex: &Ex) -> Option<(PathBuf, usize)> {
        let Ex::Call { func, .. } = ex else {
            return None;
        };
        let (object, method) = split_method(func)?;
        if method != "as_view" {
            return None;
        }
        match self.resolve(path, scope, &object, 0) {
            Val::Def(path, class) => Some((path, class)),
            _ => None,
        }
    }

    /// A view class's handlers: (HTTP method or None for all, file, def).
    fn view_handlers(&self, path: &Path, class: usize) -> Vec<(Option<String>, PathBuf, usize)> {
        self.methods(path, class)
            .into_iter()
            .filter_map(|(name, path, def)| {
                if frameworks::django::HANDLERS.contains(&name.as_str()) {
                    Some((Some(name.to_uppercase()), path, def))
                } else if frameworks::django::DISPATCH.contains(&name.as_str()) {
                    Some((None, path, def))
                } else {
                    None
                }
            })
            .collect()
    }

    /// The full route paths a route marker names, under each mount of its receiver.
    fn route_paths(&self, use_: &Use, label: &Label) -> Vec<String> {
        let Label::Route {
            path_arg, path_kw, ..
        } = label
        else {
            return Vec::new();
        };
        let own = use_
            .call
            .param(*path_arg, path_kw)
            .map(|ex| self.piece(use_.path, use_.scope, ex));
        let prefixes = match use_.receiver {
            Some(receiver) => self.receiver_paths(receiver, &mut Vec::new()),
            None => vec![Vec::new()],
        };
        prefixes
            .into_iter()
            .map(|mut pieces| {
                pieces.extend(own.iter().cloned());
                join(&pieces, use_.framework.join)
            })
            .collect()
    }

    /// Record what `marker` says about `function`, as `use_` uses it.
    fn mark(&mut self, use_: &Use, marker: &Marker, function: (&Path, usize)) {
        let (fn_path, fn_index) = function;
        let Some(fn_summary) = self.summary(fn_path) else {
            return;
        };
        let def = &fn_summary.defs[fn_index];
        let labels: Vec<String> = match marker.label {
            Label::Route { method, .. } => {
                let methods: Vec<String> = match method {
                    Method::FromName => vec![use_.method.to_uppercase()],
                    Method::Fixed(method) => vec![method.to_string()],
                    Method::Keyword(keyword, default) => match use_.call.kwarg(keyword) {
                        Some(Ex::List(items)) => items
                            .iter()
                            .map(|m| match m {
                                Ex::Str(m) => m.to_uppercase(),
                                other => format!("<{}>", other.text()),
                            })
                            .collect(),
                        _ => vec![default.to_string()],
                    },
                };
                let paths = self.route_paths(use_, &marker.label);
                methods
                    .iter()
                    .flat_map(|m| paths.iter().map(move |p| format!("{m} {p}")))
                    .collect()
            }
            Label::Task => {
                let name = use_
                    .call
                    .kwarg("name")
                    .and_then(|ex| self.eval_str(use_.path, use_.scope, ex, 0));
                vec![name
                    .unwrap_or_else(|| format!("{}.{}", self.module_name(fn_path), def.qualname))]
            }
            Label::Command => {
                let prefixes = match use_.receiver {
                    Some(receiver) => self.receiver_paths(receiver, &mut Vec::new()),
                    None => vec![Vec::new()],
                };
                let own = if use_.method == "callback" {
                    None
                } else {
                    Some(command_name(use_.call, &def.name))
                };
                let tool = use_.framework.name.to_lowercase();
                prefixes
                    .into_iter()
                    .map(|mut pieces| {
                        pieces.extend(own.iter().cloned());
                        let words = join(&pieces, use_.framework.join);
                        let words = if own.is_none() {
                            format!("{words} (callback)")
                        } else {
                            words
                        };
                        format!("{tool} {}", words.trim())
                    })
                    .collect()
            }
            Label::Hook => {
                let argument = use_.call.arg(0).map(|a| a.text());
                vec![match argument {
                    Some(argument) => format!("{} {argument}", use_.method),
                    None => use_.method.to_string(),
                }]
            }
            Label::Signal => {
                // `@receiver(sig, sender=X)` names the signal in argument 0;
                // `sig.connect(fn, sender=X)` is called on it.
                let signals: Vec<String> = match (use_.receiver, use_.call.arg(0)) {
                    (Some(_), _) => vec![match split_method(use_.call.callee()) {
                        Some((object, _)) => last_name(&object),
                        None => "signal".to_string(),
                    }],
                    (None, Some(Ex::List(items))) => items.iter().map(last_name).collect(),
                    (None, Some(signal)) => vec![last_name(signal)],
                    (None, None) => vec!["signal".to_string()],
                };
                let sender = use_
                    .call
                    .kwarg("sender")
                    .map(|s| format!(" sender={}", last_name(s)))
                    .unwrap_or_default();
                signals.iter().map(|s| format!("{s}{sender}")).collect()
            }
        };
        for label in labels {
            self.emit(fn_path, fn_index, marker.kind, label);
        }
    }

    // ---- dependencies, __main__, commands --------------------------------

    /// `Depends(fn)`: FastAPI calls `fn` (or the class, or an instance's
    /// `__call__`) to build an argument.
    fn dependencies(&mut self) {
        for (path, summary) in self.all() {
            for marked in &summary.marked_calls {
                let Some(name) = self.external(&path, marked.scope, marked.call.callee()) else {
                    continue;
                };
                if !frameworks::FASTAPI_DEPENDENCIES.contains(&name.as_str()) {
                    continue;
                }
                let Some(dependency) = marked.call.param(0, "dependency") else {
                    continue;
                };
                let target = match dependency {
                    Ex::Call { func, .. } => match self.resolve(&path, marked.scope, func, 0) {
                        Val::Def(class_path, class) => {
                            self.method_named(&class_path, class, "__call__")
                        }
                        _ => None,
                    },
                    other => match self.resolve(&path, marked.scope, other, 0) {
                        Val::Def(def_path, def) => {
                            let is_class = self
                                .summary(&def_path)
                                .is_some_and(|s| s.defs[def].is_class);
                            if is_class {
                                self.method_named(&def_path, def, "__init__")
                            } else {
                                Some((def_path, def))
                            }
                        }
                        _ => None,
                    },
                };
                if let Some((target_path, target)) = target {
                    let label = format!("Depends {}:{}", self.module_name(&path), marked.context);
                    self.emit(&target_path, target, "dependency", label);
                }
            }
        }
    }

    /// Methods overriding what a framework base class calls.
    fn overrides(&mut self) {
        for (path, summary) in self.all() {
            for (class, def) in summary.defs.iter().enumerate() {
                if !def.is_class || def.methods.is_empty() {
                    continue;
                }
                let bases = self.external_bases(&path, class);
                for (prefixes, methods) in frameworks::OVERRIDES {
                    let Some(base) = bases
                        .iter()
                        .find(|b| prefixes.iter().any(|p| b.starts_with(p)))
                    else {
                        continue;
                    };
                    for &method in &def.methods {
                        let name = &summary.defs[method].name;
                        let matches = methods.iter().any(|m| match m.strip_suffix('*') {
                            Some(prefix) => name.starts_with(prefix) && name.len() > prefix.len(),
                            None => name == m,
                        });
                        if matches {
                            let label = format!("{name} of {base}");
                            self.emit(&path, method, frameworks::OVERRIDE_KIND, label);
                        }
                    }
                }
            }
        }
    }

    fn method_named(&self, path: &Path, class: usize, name: &str) -> Option<(PathBuf, usize)> {
        self.methods(path, class)
            .into_iter()
            .find(|(n, ..)| n == name)
            .map(|(_, path, def)| (path, def))
    }

    /// Functions a module calls when run as a program: from its
    /// `if __name__ == "__main__":` block, or anywhere at the top level of
    /// a `__main__.py`.
    fn main_modules(&mut self) {
        for (path, summary) in self.all() {
            let is_main_file = path.file_name().is_some_and(|f| f == "__main__.py");
            let calls = if is_main_file {
                &summary.module_calls
            } else {
                &summary.main_guard_calls
            };
            if calls.is_empty() {
                continue;
            }
            let label = self.run_label(&path);
            for callee in calls {
                let target = self.resolve(&path, 0, callee, 0);
                if let Some((target_path, target)) = self.function(&target) {
                    self.emit(&target_path, target, frameworks::MAIN_KIND, label.clone());
                }
            }
        }
    }

    /// How the file is run: `python -m pkg.mod`, or `python script.py`
    /// for a file outside any package.
    fn run_label(&self, path: &Path) -> String {
        let module = self.module_name(path);
        let in_package = path
            .parent()
            .is_some_and(|dir| dir.join("__init__.py").is_file());
        if let Some(package) = module.strip_suffix(".__main__") {
            return format!("python -m {package}");
        }
        if in_package {
            format!("python -m {module}")
        } else {
            format!("python {}", self.host.relative_path(path))
        }
    }
}

/// One use of a marker: a decorator or a registration call.
struct Use<'u> {
    path: &'u Path,
    scope: usize,
    /// The decorator or registration call, for its arguments.
    call: &'u Ex,
    /// The method or decorator name used: `get`, `task`, `shared_task`.
    method: &'u str,
    receiver: Option<&'u Val>,
    framework: &'static Framework,
}

/// `base/a/b.py`, `base/a/b/__init__.py` or `base/a/b.pyi` for `a.b`.
fn module_file(base: &Path, parts: &[&str]) -> Option<PathBuf> {
    if parts.is_empty() {
        return None;
    }
    let mut path = base.to_path_buf();
    path.extend(parts);
    [
        path.with_extension("py"),
        path.join("__init__.py"),
        path.with_extension("pyi"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// `a.b.m` → (`a.b`, `m`).
fn split_method(callee: &Ex) -> Option<(Ex, String)> {
    match callee {
        Ex::Ref(parts) if parts.len() >= 2 => Some((
            Ex::Ref(parts[..parts.len() - 1].to_vec()),
            parts[parts.len() - 1].clone(),
        )),
        Ex::Attr(object, name) => Some(((**object).clone(), name.clone())),
        _ => None,
    }
}

fn last_name(ex: &Ex) -> String {
    match ex {
        Ex::Ref(parts) => parts.last().cloned().unwrap_or_default(),
        other => other.text(),
    }
}

/// A command's name: argument 0 or `name=`, else the function's name with
/// `_` as `-` (what Click and Typer do).
fn command_name(decorator: &Ex, function: &str) -> Piece {
    match decorator.param(0, "name") {
        Some(Ex::Str(name)) => Piece::Known(name.clone()),
        Some(other) => Piece::Unread(other.text()),
        None => Piece::Known(function.replace('_', "-")),
    }
}

fn join(pieces: &[Piece], how: Join) -> String {
    match how {
        Join::Words => pieces
            .iter()
            .map(Piece::render)
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        Join::Url => {
            let mut out = String::new();
            for piece in pieces {
                let piece = piece.render();
                if piece.is_empty() {
                    continue;
                }
                out = format!(
                    "{}/{}",
                    out.trim_end_matches('/'),
                    piece.trim_start_matches('/')
                );
            }
            if out.is_empty() {
                "/".to_string()
            } else {
                out
            }
        }
    }
}
