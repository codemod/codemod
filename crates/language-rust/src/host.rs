//! The flow, mutation, call-graph and entry-point queries for Rust: a
//! [`FlowHost`] over [`crate::facts`], with calls resolved by
//! rust-analyzer instead of by following values.

use crate::facts::{self, Built};
use crate::workspace::{rust_files, RaWorkspace};
use language_core::{
    ByteRange, CallEdgeKind, CallGraphDirection, CallGraphOptions, CallGraphResult, EntryPoint,
    EntryPointRule, FlowOptions, FlowResult, FunctionEntry, FunctionEntryPoints, MutationOptions,
    MutationsResult, SemanticError, SemanticResult, UnparsedFile,
};
use language_flow::query::{self, CallerIndexSlot, FlowHost, QueryKind};
use language_flow::{CallFact, FileFlow};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Rust facts by file, over one rust-analyzer database per workspace (or,
/// in file scope, per file).
pub struct RsFlow {
    /// The workspace root. None in file scope: each file stands alone.
    root: Option<PathBuf>,
    /// rust-analyzer databases by the root they were loaded from (the
    /// workspace root, or in file scope the file itself).
    workspaces: RwLock<HashMap<PathBuf, Arc<Result<RaWorkspace, String>>>>,
    files: RwLock<HashMap<PathBuf, Arc<Built>>>,
    callers: CallerIndexSlot,
    entry_point_rules: RwLock<Vec<EntryPointRule>>,
}

impl std::fmt::Debug for RsFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RsFlow")
            .field("root", &self.root)
            .field("files", &self.files.read().len())
            .finish()
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl RsFlow {
    pub fn new(root: Option<PathBuf>) -> Self {
        RsFlow {
            root: root.map(|r| canonical(&r)),
            workspaces: RwLock::default(),
            files: RwLock::default(),
            callers: CallerIndexSlot::default(),
            entry_point_rules: RwLock::default(),
        }
    }

    pub fn clear(&self) {
        self.workspaces.write().clear();
        self.files.write().clear();
        self.callers.clear();
    }

    /// The rust-analyzer database that knows `path`.
    pub(crate) fn workspace(
        &self,
        path: &Path,
    ) -> SemanticResult<Arc<Result<RaWorkspace, String>>> {
        let key = match &self.root {
            Some(root) => root.clone(),
            None => path.to_path_buf(),
        };
        if let Some(found) = self.workspaces.read().get(&key) {
            return Ok(Arc::clone(found));
        }
        let loaded = match &self.root {
            Some(root) => RaWorkspace::load(root, &rust_files(root)),
            None => {
                RaWorkspace::load_detached(path.parent().unwrap_or(path), &[path.to_path_buf()])
            }
        };
        if let Err(message) = &loaded {
            log::warn!("rust-analyzer could not load {}: {message}", key.display());
        }
        let loaded = Arc::new(loaded);
        self.workspaces
            .write()
            .entry(key)
            .or_insert_with(|| Arc::clone(&loaded));
        Ok(loaded)
    }

    /// Run `f` with the database that knows `path`.
    pub(crate) fn with_workspace<T>(
        &self,
        path: &Path,
        f: impl FnOnce(&RaWorkspace, &Path) -> T,
    ) -> SemanticResult<T> {
        let path = canonical(path);
        let workspace = self.workspace(&path)?;
        match &*workspace {
            Ok(ws) => Ok(f(ws, &path)),
            Err(message) => Err(SemanticError::Internal(format!(
                "rust-analyzer could not load the workspace: {message}"
            ))),
        }
    }

    /// Use `content` for `path` from now on (the file as the step sees it).
    pub fn update(&self, path: &Path, content: &str) {
        let path = canonical(path);
        let Ok(workspace) = self.workspace(&path) else {
            return;
        };
        let Ok(ws) = &*workspace else {
            return;
        };
        if ws.text(&path).is_some_and(|old| &*old == content) {
            return;
        }
        ws.set_text(&path, content);
        // Facts hold call targets resolved across files: a change anywhere
        // can move what other files call.
        self.files.write().clear();
        self.callers.clear();
    }

    fn built(&self, path: &Path) -> Option<Arc<Built>> {
        if let Some(found) = self.files.read().get(path) {
            return Some(Arc::clone(found));
        }
        let workspace = self.workspace(path).ok()?;
        let ws = workspace.as_ref().as_ref().ok()?;
        let built = Arc::new(facts::build(ws, path)?);
        self.files
            .write()
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::clone(&built));
        Some(built)
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
        if options.direction == CallGraphDirection::Callers {
            query::applicable(self, path, range, QueryKind::Callers)?;
        }
        Ok(query::call_graph(self, path, range, options))
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

    /// Every function in the workspace something outside its code calls.
    pub fn all_entry_points(&self) -> Vec<FunctionEntryPoints> {
        query::all_entry_points(self)
            .into_iter()
            .map(|(file_path, range, entry_points)| FunctionEntryPoints {
                file_path,
                range,
                entry_points,
            })
            .collect()
    }

    pub fn set_entry_point_rules(&self, rules: Vec<EntryPointRule>) {
        *self.entry_point_rules.write() = rules;
    }

    /// The index of the function of `flow` written at exactly `range`.
    fn function_at(flow: &FileFlow, range: ByteRange) -> Option<u32> {
        flow.functions
            .iter()
            .position(|f| f.range == range)
            .map(|i| i as u32)
    }
}

impl FlowHost for RsFlow {
    fn canonical(&self, path: &Path) -> PathBuf {
        canonical(path)
    }

    fn flow(&self, path: &Path) -> Option<Arc<FileFlow>> {
        self.built(path).map(|built| Arc::clone(&built.flow))
    }

    fn content(&self, path: &Path) -> Option<Arc<str>> {
        let workspace = self.workspace(path).ok()?;
        workspace.as_ref().as_ref().ok()?.text(path)
    }

    /// The Rust facts write a reference to a function or constant of
    /// another file as an import of `file` named `start-end`.
    fn resolve_import(
        &self,
        _from: &Path,
        specifier: &str,
        export_name: &str,
    ) -> Option<(PathBuf, ByteRange)> {
        let (start, end) = export_name.split_once('-')?;
        Some((
            PathBuf::from(specifier),
            ByteRange::new(start.parse().ok()?, end.parse().ok()?),
        ))
    }

    fn workspace_files(&self) -> Vec<PathBuf> {
        match &self.root {
            Some(root) => match self.workspace(root).ok().as_deref() {
                Some(Ok(ws)) => ws.local_files(),
                _ => Vec::new(),
            },
            None => self.files.read().keys().cloned().collect(),
        }
    }

    fn import_parse_error(&self, _from: &Path, _specifier: &str) -> Option<UnparsedFile> {
        // rust-analyzer's parser recovers from errors: every file parses.
        None
    }

    fn parse_failures(&self) -> Vec<UnparsedFile> {
        Vec::new()
    }

    fn caller_index(&self) -> Option<&CallerIndexSlot> {
        Some(&self.callers)
    }

    fn resolve_call(
        &self,
        path: &Path,
        flow: &FileFlow,
        call: &CallFact,
    ) -> Option<Vec<(PathBuf, u32, CallEdgeKind)>> {
        let built = self.built(path)?;
        let node = flow.exact(call.range)?;
        let index = *flow.call_by_node.get(&node)?;
        let targets = built.resolved.calls.get(index as usize)?.as_ref()?;
        let mut out = Vec::new();
        for (target, range, kind) in targets {
            let Some(target_flow) = self.flow(target) else {
                continue;
            };
            if let Some(function) = Self::function_at(&target_flow, *range) {
                out.push((target.clone(), function, *kind));
            }
        }
        Some(out)
    }

    fn entry_points_of(&self, path: &Path, flow: &FileFlow, function: u32) -> Vec<EntryPoint> {
        let Some(built) = self.built(path) else {
            return Vec::new();
        };
        let mut out = built
            .resolved
            .entry_points
            .get(&function)
            .cloned()
            .unwrap_or_default();
        let rules = self.entry_point_rules.read();
        if !rules.is_empty() {
            if let Some(fact) = flow.functions.get(function as usize) {
                let exports: Vec<String> = match &fact.name {
                    Some(name) if out.iter().any(|e| e.kind == "export") => vec![name.to_string()],
                    _ => Vec::new(),
                };
                let decorators: Vec<String> = flow
                    .decorators
                    .get(&function)
                    .map(|names| names.iter().map(|n| n.to_string()).collect())
                    .unwrap_or_default();
                let file = self.relative_path(path);
                out.extend(language_core::declared_entry_points(
                    &rules,
                    &FunctionEntry {
                        file: &file,
                        name: fact.name.as_deref(),
                        exports: &exports,
                        decorators: &decorators,
                    },
                ));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn relative_path(&self, path: &Path) -> String {
        let relative = match &self.root {
            Some(root) => path.strip_prefix(root).unwrap_or(path),
            None => path,
        };
        relative.to_string_lossy().replace('\\', "/")
    }
}
