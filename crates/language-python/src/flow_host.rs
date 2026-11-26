//! The flow, mutation and call-graph queries for Python: a [`FlowHost`]
//! over [`crate::flow`] facts, with Python's module resolution.
//!
//! Modules resolve against the workspace root (and `src/` under it), the
//! directories above the importing file, and relative imports against the
//! importing file's package. Installed packages are not looked at: calls into
//! them are unresolved.

use crate::entry_points::BuiltinEntryPoints;
use crate::flow::build_file_flow;
use crate::ty::TySession;
use language_core::{
    ByteRange, CallGraphOptions, CallGraphResult, EntryPoint, EntryPointRule, FileSnapshot,
    FlowEdgeKind, FlowNodeKind, FlowOptions, FlowResult, FunctionEntry, FunctionRef,
    MutationOptions, MutationsResult, SemanticResult, UnparsedFile,
};
use language_flow::query::{self, CallerIndexSlot, FlowHost, QueryKind};
use language_flow::{CallFact, CalleeFact, FileFlow};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Directories never searched for workspace files.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".venv",
    "venv",
    "env",
    ".env",
    "node_modules",
    "__pycache__",
    "site-packages",
    ".tox",
    ".nox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "build",
    "dist",
];

enum Cached {
    Parsed {
        content: Arc<str>,
        flow: Arc<FileFlow>,
    },
    Failed {
        message: String,
    },
}

/// Python facts by file, and the queries over them.
pub struct PyFlow {
    /// The workspace root. None in file scope: only files seen are known.
    root: Option<PathBuf>,
    files: RwLock<HashMap<PathBuf, Cached>>,
    callers: CallerIndexSlot,
    /// The selector's `semanticAnalysis.entryPoints`.
    entry_point_rules: RwLock<Vec<EntryPointRule>>,
    /// File texts as the step started. None: read each file once, as is.
    snapshot: Option<Arc<FileSnapshot>>,
    /// ty, for types and member calls the values leave unresolved.
    /// Workspace scope on a real file system only.
    ty: Option<TySession>,
    /// The importers table of the module queries.
    pub(crate) modules: crate::modules::PyModules,
    /// Packaging, `__main__` and framework entry points.
    builtin: BuiltinEntryPoints,
    /// `[tool.pytest.ini_options] pythonpath`, read once per step.
    pytest_paths: RwLock<Option<Vec<PathBuf>>>,
}

impl std::fmt::Debug for PyFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyFlow")
            .field("root", &self.root)
            .field("files", &self.files.read().len())
            .finish()
    }
}

impl PyFlow {
    pub fn new(root: Option<PathBuf>) -> Self {
        PyFlow {
            root,
            files: RwLock::default(),
            callers: CallerIndexSlot::default(),
            entry_point_rules: RwLock::default(),
            snapshot: None,
            ty: None,
            modules: Default::default(),
            builtin: BuiltinEntryPoints::default(),
            pytest_paths: RwLock::default(),
        }
    }

    /// A workspace's facts, reading files through `snapshot` (shared with
    /// the provider, so both see the same texts).
    pub fn with_snapshot(root: PathBuf, snapshot: Arc<FileSnapshot>) -> Self {
        PyFlow {
            snapshot: Some(snapshot),
            ..PyFlow::new(Some(root))
        }
    }

    /// Types from ty: `type_of` infers, and member calls the values leave
    /// unresolved resolve by the receiver's type. Needs a workspace read
    /// through a snapshot ([`PyFlow::with_snapshot`]).
    pub fn with_types(mut self, on: bool) -> Self {
        self.ty = match (on, &self.root, &self.snapshot) {
            (true, Some(root), Some(snapshot)) => {
                Some(TySession::new(root.clone(), Arc::clone(snapshot)))
            }
            _ => None,
        };
        self
    }

    pub fn clear(&self) {
        self.files.write().clear();
        self.callers.clear();
        self.modules.clear();
        self.builtin.clear();
        *self.pytest_paths.write() = None;
    }

    /// Use `content` for `path` from now on (the file as the codemod sees it).
    pub fn update(&self, path: &Path, content: &str) {
        let path = self.canonical(path);
        let same = matches!(
            self.files.read().get(&path),
            Some(Cached::Parsed { content: old, .. }) if &**old == content
        );
        if same {
            return;
        }
        let cached = match build_file_flow(content) {
            Ok(flow) => Cached::Parsed {
                content: content.into(),
                flow: Arc::new(flow),
            },
            Err(message) => Cached::Failed { message },
        };
        self.files.write().insert(path, cached);
        self.callers.clear();
        self.builtin.invalidate();
    }

    pub fn data_flow(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
        options: FlowOptions,
    ) -> SemanticResult<FlowResult> {
        self.update(path, content);
        query::applicable(self, path, range, QueryKind::Flow)?;
        Ok(query::data_flow(self, path, range, options))
    }

    pub fn call_graph(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
        options: CallGraphOptions,
    ) -> SemanticResult<CallGraphResult> {
        self.update(path, content);
        if options.direction == language_core::CallGraphDirection::Callers {
            query::applicable(self, path, range, QueryKind::Callers)?;
        }
        Ok(query::call_graph(self, path, range, options))
    }

    /// Why the function at `range` is called from outside the workspace's
    /// code: packaging metadata, a `__main__` guard or a framework calls it
    /// ([`crate::entry_points`]), or a selector rule declares it.
    pub fn entry_points(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
    ) -> SemanticResult<Vec<EntryPoint>> {
        self.update(path, content);
        query::applicable(self, path, range, QueryKind::Callers)?;
        Ok(query::entry_points(self, path, range))
    }

    /// Every function in the workspace that a rule declares an entry point.
    pub fn all_entry_points(&self) -> Vec<language_core::FunctionEntryPoints> {
        query::all_entry_points(self)
            .into_iter()
            .map(
                |(file_path, range, entry_points)| language_core::FunctionEntryPoints {
                    file_path,
                    range,
                    entry_points,
                },
            )
            .collect()
    }

    pub fn set_entry_point_rules(&self, rules: Vec<EntryPointRule>) {
        *self.entry_point_rules.write() = rules;
    }

    pub fn mutations(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
        options: MutationOptions,
    ) -> SemanticResult<MutationsResult> {
        self.update(path, content);
        query::applicable(self, path, range, QueryKind::Mutations)?;
        Ok(query::mutations(self, path, range, options))
    }

    /// The type of the value at `range`: what ty infers, else the declared
    /// or evident one, from annotations and literals.
    pub fn type_of(&self, path: &Path, content: &str, range: ByteRange) -> Option<String> {
        self.update(path, content);
        let path = self.canonical(path);
        // `x` of `f(x=1)` is not a value: no type, not the call's.
        if self
            .flow(&path)
            .is_some_and(|flow| flow.is_not_value(range))
        {
            return None;
        }
        if let Some(found) = self.ty.as_ref().and_then(|ty| ty.type_at(&path, range)) {
            return Some(found);
        }
        let flow = self.flow(&path)?;
        let node = flow.exact(range).or_else(|| flow.locate(range))?;
        self.type_of_node(&path, &flow, node, 0)
    }

    fn type_of_node(
        &self,
        path: &Path,
        flow: &Arc<FileFlow>,
        node: u32,
        depth: u32,
    ) -> Option<String> {
        if depth > 8 {
            return None;
        }
        let content = self.content(path)?;
        let text = |range: ByteRange| content[range.start as usize..range.end as usize].to_string();
        if let Some(declared) = flow.declared_types.get(&node) {
            if flow.nodes[node as usize].kind != FlowNodeKind::Function {
                return unqualified(declared);
            }
        }
        let fact = &flow.nodes[node as usize];
        match fact.kind {
            FlowNodeKind::Literal => {
                return Some(literal_type(&text(fact.range)).to_string());
            }
            FlowNodeKind::Expression if literal_type(&text(fact.range)) == "str" => {
                return Some("str".to_string());
            }
            // Lists, tuples and sets (and their comprehensions) are arrays.
            FlowNodeKind::Array => {
                let kind = match text(fact.range).as_bytes().first() {
                    Some(b'[') => "list",
                    Some(b'{') => "set",
                    _ => "tuple",
                };
                return Some(kind.to_string());
            }
            FlowNodeKind::Member => {
                if let Some(found) = self.self_attribute_type(path, flow, node, depth) {
                    return Some(found);
                }
            }
            FlowNodeKind::Object => return Some("dict".to_string()),
            FlowNodeKind::Class => {
                return Some(format!("type[{}]", fact.name.as_deref().unwrap_or("?")))
            }
            FlowNodeKind::This => {
                let class = flow.class_of(fact.function)?;
                return flow.nodes[class as usize]
                    .name
                    .as_deref()
                    .map(str::to_string);
            }
            FlowNodeKind::Function => {
                let function = flow.fn_by_node.get(&node)?;
                let range = flow.functions[*function as usize].range;
                let header = text(range);
                let signature = header.split(':').next().unwrap_or(&header).to_string();
                let returns = flow.declared_types.get(&node).map(|t| t.to_string());
                let params = signature
                    .find('(')
                    .map(|start| signature[start..].trim().to_string())
                    .unwrap_or_default();
                // `def f(a: int) -> str` gives `(a: int) -> str`.
                let params = params.split("->").next().unwrap_or("").trim().to_string();
                let params = if params.is_empty() {
                    header
                        .find('(')
                        .and_then(|s| {
                            header[s..]
                                .find(')')
                                .map(|e| header[s..s + e + 1].to_string())
                        })
                        .unwrap_or_else(|| "()".to_string())
                } else {
                    params
                };
                return Some(match returns {
                    Some(returns) => format!("{params} -> {returns}"),
                    None => params,
                });
            }
            FlowNodeKind::Call => {
                if let Some(call) = flow.call_by_node.get(&node) {
                    let call = &flow.calls[*call as usize];
                    for (target_path, target, via) in query::call_targets(self, path, flow, call) {
                        let target_flow = self.flow(&target_path)?;
                        let function = &target_flow.functions[target as usize];
                        // `Repo()`: an instance of the class.
                        if via == language_core::CallEdgeKind::Constructor {
                            if let Some(class) = function.class_node {
                                return target_flow.nodes[class as usize]
                                    .name
                                    .as_deref()
                                    .map(str::to_string);
                            }
                        }
                        if let Some(returns) = target_flow.declared_types.get(&function.node) {
                            return Some(returns.to_string());
                        }
                    }
                    // A class with no `__init__` of its own.
                    if let language_flow::CalleeFact::Node(callee) = &call.callee {
                        if let Some(class) = self.class_named(path, flow, *callee) {
                            return Some(class);
                        }
                    }
                }
                return None;
            }
            _ => {}
        }
        // A use of a binding: the binding's type; a binding: what it was assigned.
        let sources: Vec<(u32, FlowEdgeKind)> = flow.in_edges[node as usize].to_vec();
        for (source, edge) in &sources {
            if *edge == FlowEdgeKind::Read {
                return self.type_of_node(path, flow, *source, depth + 1);
            }
        }
        if fact.binding {
            if let Some(import) = flow.imports.get(&node) {
                if !import.namespace {
                    if let Some((target, range)) =
                        self.resolve_import(path, &import.module, &import.name)
                    {
                        let target_flow = self.flow(&target)?;
                        let target_node = target_flow.exact(range)?;
                        return self.type_of_node(&target, &target_flow, target_node, depth + 1);
                    }
                }
                return None;
            }
            for (source, edge) in &sources {
                if *edge == FlowEdgeKind::Assign {
                    if let Some(found) = self.type_of_node(path, flow, *source, depth + 1) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    /// The type of `self.name` read in a method: the class attribute's
    /// declared type (`name: ClassVar[dict] = ...` is `dict`), else what the
    /// class body or a method stores in it.
    fn self_attribute_type(
        &self,
        path: &Path,
        flow: &Arc<FileFlow>,
        node: u32,
        depth: u32,
    ) -> Option<String> {
        let name = flow.nodes[node as usize].name.as_deref()?;
        let object = flow.in_edges[node as usize]
            .iter()
            .find(|(_, edge)| *edge == FlowEdgeKind::Property)?
            .0;
        let object = &flow.nodes[object as usize];
        if object.kind != FlowNodeKind::This {
            return None;
        }
        let class = flow.class_of(object.function)?;
        let fields: Vec<u32> = flow
            .class_fields
            .get(&class)?
            .iter()
            .filter(|(field, field_node)| &**field == name && *field_node != node)
            .map(|(_, field_node)| *field_node)
            .collect();
        if let Some(declared) = fields
            .iter()
            .find_map(|field| flow.declared_types.get(field))
        {
            return unqualified(declared);
        }
        fields.iter().find_map(|field| {
            flow.in_edges[*field as usize]
                .iter()
                .filter(|(_, edge)| matches!(edge, FlowEdgeKind::Store | FlowEdgeKind::Assign))
                .find_map(|(value, _)| self.type_of_node(path, flow, *value, depth + 1))
        })
    }

    /// The class a callee expression names, when it is a workspace class.
    fn class_named(&self, path: &Path, flow: &Arc<FileFlow>, callee: u32) -> Option<String> {
        let found = self.type_of_node(path, flow, callee, 1)?;
        found
            .strip_prefix("type[")
            .and_then(|rest| rest.strip_suffix(']'))
            .map(str::to_string)
    }

    fn read(&self, path: &Path) -> Option<()> {
        if self.files.read().contains_key(path) {
            return Some(());
        }
        let content: Arc<str> = match &self.snapshot {
            Some(snapshot) => snapshot
                .get_or_read(path, || std::fs::read_to_string(path))
                .ok()?,
            None => std::fs::read_to_string(path).ok()?.into(),
        };
        let cached = match build_file_flow(&content) {
            Ok(flow) => Cached::Parsed {
                content,
                flow: Arc::new(flow),
            },
            Err(message) => Cached::Failed { message },
        };
        self.files
            .write()
            .entry(path.to_path_buf())
            .or_insert(cached);
        Some(())
    }

    /// ty's declarations for the member a call names: `call` must be
    /// `object.property(…)` with a ty session behind this host.
    fn member_declarations(
        &self,
        path: &Path,
        flow: &FileFlow,
        call: &CallFact,
    ) -> Vec<(PathBuf, ByteRange)> {
        let (Some(ty), CalleeFact::Member { object, property }) = (&self.ty, &call.callee) else {
            return Vec::new();
        };
        let Some(content) = self.content(path) else {
            return Vec::new();
        };
        // The property's name: the first time it is written after the
        // object (only `.`, spaces and comments come between).
        let after = flow.nodes[*object as usize].range.end as usize;
        let end = (call.range.end as usize).min(content.len());
        let Some(offset) = content
            .get(after..end)
            .and_then(|between| between.find(&**property))
        else {
            return Vec::new();
        };
        ty.member_declarations(path, (after + offset) as u32)
    }

    /// Whether `path` is the workspace's own code: under the root, outside
    /// virtual environments and other skipped directories.
    pub(crate) fn in_workspace(&self, path: &Path) -> bool {
        let Some(root) = &self.root else {
            return true;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        !relative.components().any(|part| {
            let part = part.as_os_str().to_string_lossy();
            SKIPPED_DIRS.contains(&part.as_ref()) || part.starts_with('.')
        })
    }

    /// The workspace root; None in file scope.
    pub(crate) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Places an absolute module name is looked up from, as `sys.path`
    /// has them: the workspace root, its `src/`, and the directory that
    /// holds the file's top-level package (the file's own directory for a
    /// script outside any package). Never a directory inside a package, so
    /// `shopfloor/celery.py` does not shadow `celery` for
    /// `shopfloor/orders/tasks.py`.
    fn search_roots(&self, from: &Path) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        let mut add = |dir: PathBuf| {
            if !roots.contains(&dir) {
                roots.push(dir);
            }
        };
        if let Some(root) = &self.root {
            add(root.clone());
            add(root.join("src"));
            for dir in self.pytest_paths() {
                add(dir);
            }
        }
        let mut base = from.parent();
        while let Some(dir) = base {
            let at_root = self.root.as_deref().is_some_and(|root| dir == root);
            if at_root || !dir.join("__init__.py").is_file() {
                break;
            }
            base = dir.parent();
        }
        if let Some(base) = base {
            add(base.to_path_buf());
        }
        // pytest puts the directory of each conftest.py outside a package
        // on sys.path, so tests import helpers that sit next to it.
        if let Some(root) = &self.root {
            for dir in from.ancestors().skip(1) {
                if !dir.starts_with(root) {
                    break;
                }
                if dir.join("conftest.py").is_file() && !dir.join("__init__.py").is_file() {
                    add(dir.to_path_buf());
                }
            }
        }
        roots
    }

    /// The workspace's `[tool.pytest.ini_options] pythonpath` directories.
    fn pytest_paths(&self) -> Vec<PathBuf> {
        if let Some(paths) = &*self.pytest_paths.read() {
            return paths.clone();
        }
        let paths: Vec<PathBuf> = self
            .root
            .as_deref()
            .map(|root| {
                let text = std::fs::read_to_string(root.join("pyproject.toml")).unwrap_or_default();
                let table = toml::from_str::<toml::Table>(&text).unwrap_or_default();
                let listed = table
                    .get("tool")
                    .and_then(|tool| tool.get("pytest"))
                    .and_then(|pytest| pytest.get("ini_options"))
                    .and_then(|options| options.get("pythonpath"))
                    .cloned();
                let entries: Vec<String> = match listed {
                    Some(toml::Value::Array(items)) => items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect(),
                    Some(toml::Value::String(one)) => vec![one],
                    _ => Vec::new(),
                };
                entries
                    .iter()
                    .map(|entry| root.join(entry))
                    .filter(|dir| dir.starts_with(root))
                    .collect()
            })
            .unwrap_or_default();
        *self.pytest_paths.write() = Some(paths.clone());
        paths
    }

    pub(crate) fn module_file(base: &Path, parts: &[&str]) -> Option<PathBuf> {
        let mut path = base.to_path_buf();
        for part in parts {
            path.push(part);
        }
        if parts.is_empty() {
            let init = path.join("__init__.py");
            return init.is_file().then_some(init);
        }
        let file = path.with_extension("py");
        if file.is_file() {
            return Some(file);
        }
        let init = path.join("__init__.py");
        if init.is_file() {
            return Some(init);
        }
        let stub = path.with_extension("pyi");
        stub.is_file().then_some(stub)
    }
}

/// A declared type without its qualifier: `ClassVar[dict]` and
/// `Final[int]` are `dict` and `int`. None for a bare qualifier, which
/// declares no type.
fn unqualified(declared: &str) -> Option<String> {
    const QUALIFIERS: &[&str] = &["ClassVar", "Final", "Required", "NotRequired", "ReadOnly"];
    let declared = declared.trim();
    let open = declared.find('[');
    let head = &declared[..open.unwrap_or(declared.len())];
    let qualifier = head.rsplit('.').next().unwrap_or(head);
    if !QUALIFIERS.contains(&qualifier) {
        return Some(declared.to_string());
    }
    let inner = declared[open? + 1..].strip_suffix(']')?;
    unqualified(inner)
}

/// The type of a literal's text: `"a"` and `f"{x}"` are str, `b""` bytes.
/// "?" for text that is not a literal.
fn literal_type(literal: &str) -> &'static str {
    let unprefixed = literal.trim_start_matches(|c: char| "rRbBuUfF".contains(c));
    let quoted = unprefixed.starts_with(['"', '\'']);
    match literal {
        "True" | "False" => "bool",
        "None" => "None",
        "..." => "ellipsis",
        _ if quoted && literal[..literal.len() - unprefixed.len()].contains(['b', 'B']) => "bytes",
        _ if quoted => "str",
        _ if literal.starts_with(|c: char| c.is_ascii_digit()) => {
            if literal.ends_with(['j', 'J']) {
                "complex"
            } else if !literal.starts_with("0x")
                && (literal.contains('.') || literal.contains(['e', 'E']))
            {
                "float"
            } else {
                "int"
            }
        }
        _ => "?",
    }
}

impl FlowHost for PyFlow {
    fn canonical(&self, path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    fn flow(&self, path: &Path) -> Option<Arc<FileFlow>> {
        self.read(path)?;
        match self.files.read().get(path)? {
            Cached::Parsed { flow, .. } => Some(Arc::clone(flow)),
            Cached::Failed { .. } => None,
        }
    }

    fn content(&self, path: &Path) -> Option<Arc<str>> {
        self.read(path)?;
        match self.files.read().get(path)? {
            Cached::Parsed { content, .. } => Some(Arc::clone(content)),
            Cached::Failed { .. } => None,
        }
    }

    fn resolve_import(
        &self,
        from: &Path,
        specifier: &str,
        export_name: &str,
    ) -> Option<(PathBuf, ByteRange)> {
        let target = self.module_path(from, specifier)?;
        let flow = self.flow(&target)?;
        let binding = flow.module_bindings.get(export_name)?;
        Some((target, flow.nodes[*binding as usize].range))
    }

    fn module_path(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        let dots = specifier.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = specifier[dots..]
            .split('.')
            .filter(|p| !p.is_empty())
            .collect();
        let found = if dots > 0 {
            let mut base = from.parent()?;
            for _ in 1..dots {
                base = base.parent()?;
            }
            Self::module_file(base, &parts)
        } else {
            if parts.is_empty() {
                return None;
            }
            self.search_roots(from)
                .iter()
                .find_map(|root| Self::module_file(root, &parts))
        }?;
        Some(self.canonical(&found))
    }

    fn submodule(&self, from: &Path, specifier: &str, name: &str) -> Option<String> {
        let joined = if specifier.ends_with('.') {
            format!("{specifier}{name}")
        } else {
            format!("{specifier}.{name}")
        };
        self.module_path(from, &joined).map(|_| joined)
    }

    fn workspace_files(&self) -> Vec<PathBuf> {
        let Some(root) = &self.root else {
            return self.files.read().keys().cloned().collect();
        };
        let mut found = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if kind.is_dir() {
                    if !SKIPPED_DIRS.contains(&name.as_ref()) && !name.starts_with('.') {
                        stack.push(path);
                    }
                } else if kind.is_file() && name.ends_with(".py") {
                    found.push(self.canonical(&path));
                }
            }
        }
        found.sort();
        found
    }

    fn import_parse_error(&self, from: &Path, specifier: &str) -> Option<UnparsedFile> {
        let target = self.module_path(from, specifier)?;
        self.read(&target)?;
        match self.files.read().get(&target)? {
            Cached::Failed { message } => Some(UnparsedFile {
                file_path: target.clone(),
                message: message.clone(),
            }),
            Cached::Parsed { .. } => None,
        }
    }

    fn parse_failures(&self) -> Vec<UnparsedFile> {
        self.files
            .read()
            .iter()
            .filter_map(|(path, cached)| match cached {
                Cached::Failed { message } => Some(UnparsedFile {
                    file_path: path.clone(),
                    message: message.clone(),
                }),
                Cached::Parsed { .. } => None,
            })
            .collect()
    }

    fn caller_index(&self) -> Option<&CallerIndexSlot> {
        Some(&self.callers)
    }

    /// `repo.save()` the values left unresolved (`repo` is a parameter, a
    /// field, a call's result): the workspace methods ty says `save` is.
    fn typed_method_targets(
        &self,
        path: &Path,
        flow: &FileFlow,
        call: &CallFact,
    ) -> Vec<(PathBuf, u32)> {
        let mut found = Vec::new();
        for (target, range) in self.member_declarations(path, flow, call) {
            if !self.in_workspace(&target) {
                continue;
            }
            let target = self.canonical(&target);
            let Some(target_flow) = self.flow(&target) else {
                continue;
            };
            let function = target_flow
                .functions
                .iter()
                .position(|f| f.name_range == Some(range));
            if let Some(function) = function {
                found.push((target, function as u32));
            }
        }
        found
    }

    /// Where ty says the member a still-unresolved call names is declared,
    /// in the workspace or a library (`QuerySet.filter` in Django's code).
    fn typed_declaration(
        &self,
        path: &Path,
        flow: &FileFlow,
        call: &CallFact,
    ) -> Option<FunctionRef> {
        let CalleeFact::Member { property, .. } = &call.callee else {
            return None;
        };
        let (file_path, range) = self
            .member_declarations(path, flow, call)
            .into_iter()
            .next()?;
        Some(FunctionRef {
            file_path,
            range,
            name: Some(property.to_string()),
        })
    }

    /// Built-in entry points, then the selector's rules (a top-level
    /// function counts as exported under its name).
    fn entry_points_of(&self, path: &Path, flow: &FileFlow, function: u32) -> Vec<EntryPoint> {
        let mut found = self
            .builtin
            .of(self, self.root.as_deref(), path, flow, function);
        let rules = self.entry_point_rules.read();
        if rules.is_empty() {
            return found;
        }
        let Some(fact) = flow.functions.get(function as usize) else {
            return found;
        };
        let exports: Vec<String> = match (&fact.name, fact.parent, fact.class_node) {
            (Some(name), None, None) => vec![name.to_string()],
            _ => Vec::new(),
        };
        let decorators: Vec<String> = flow
            .decorators
            .get(&function)
            .map(|names| names.iter().map(|n| n.to_string()).collect())
            .unwrap_or_default();
        let file = self.relative_path(path);
        found.extend(language_core::declared_entry_points(
            &rules,
            &FunctionEntry {
                file: &file,
                name: fact.name.as_deref(),
                exports: &exports,
                decorators: &decorators,
            },
        ));
        found
    }

    fn relative_path(&self, path: &Path) -> String {
        let root = self.root.as_ref().map(|root| self.canonical(root));
        let relative = match &root {
            Some(root) => path.strip_prefix(root).unwrap_or(path),
            None => path,
        };
        relative.to_string_lossy().replace('\\', "/")
    }
}
