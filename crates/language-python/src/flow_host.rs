//! The flow, mutation and call-graph queries for Python: a [`FlowHost`]
//! over [`crate::flow`] facts, with Python's module resolution.
//!
//! Modules resolve against the workspace root (and `src/` under it), the
//! directories above the importing file, and relative imports against the
//! importing file's package. Installed packages are not looked at: calls into
//! them are unresolved.

use crate::flow::build_file_flow;
use language_core::{
    ByteRange, CallGraphOptions, CallGraphResult, EntryPoint, EntryPointRule, FlowEdgeKind,
    FlowNodeKind, FlowOptions, FlowResult, FunctionEntry, MutationOptions, MutationsResult,
    SemanticResult, UnparsedFile,
};
use language_flow::query::{self, CallerIndexSlot, FlowHost, QueryKind};
use language_flow::FileFlow;
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
        }
    }

    pub fn clear(&self) {
        self.files.write().clear();
        self.callers.clear();
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
    /// code: a selector rule declares it. Python has no package exports
    /// that mark a function public, so only rules count.
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

    /// The declared or evident type of the value at `range`, from
    /// annotations and literals only (there is no type checker behind it).
    pub fn type_of(&self, path: &Path, content: &str, range: ByteRange) -> Option<String> {
        self.update(path, content);
        let path = self.canonical(path);
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
                return Some(declared.to_string());
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
            FlowNodeKind::Array => return Some("list".to_string()),
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
        let content = std::fs::read_to_string(path).ok()?;
        let cached = match build_file_flow(&content) {
            Ok(flow) => Cached::Parsed {
                content: content.into(),
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

    /// Places an absolute module name is looked up from.
    fn search_roots(&self, from: &Path) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Some(root) = &self.root {
            roots.push(root.clone());
            roots.push(root.join("src"));
        }
        let mut current = from.parent();
        let mut steps = 0;
        while let Some(dir) = current {
            if steps > 12 {
                break;
            }
            if !roots.iter().any(|r| r == dir) {
                roots.push(dir.to_path_buf());
            }
            if self.root.as_deref().is_some_and(|root| dir == root) {
                break;
            }
            current = dir.parent();
            steps += 1;
        }
        roots
    }

    fn module_file(base: &Path, parts: &[&str]) -> Option<PathBuf> {
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

    /// Rules only: a top-level function counts as exported under its name.
    fn entry_points_of(&self, path: &Path, flow: &FileFlow, function: u32) -> Vec<EntryPoint> {
        let rules = self.entry_point_rules.read();
        if rules.is_empty() {
            return Vec::new();
        }
        let Some(fact) = flow.functions.get(function as usize) else {
            return Vec::new();
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
        language_core::declared_entry_points(
            &rules,
            &FunctionEntry {
                file: &file,
                name: fact.name.as_deref(),
                exports: &exports,
                decorators: &decorators,
            },
        )
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
