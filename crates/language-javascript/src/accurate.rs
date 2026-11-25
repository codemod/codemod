//! Accurate mode implementation for workspace-wide lazy indexing.

use crate::cache::{ExportedSymbol, FileSymbols, ImportedSymbol, Symbol, SymbolCache};
use crate::dataflow::{CallFact, CalleeFact, FileFlow, FlowCache};
use crate::error::JsSemanticError;
use crate::flow_query::{self, CallerIndexSlot, FlowHost};
use crate::oxc_adapter::{analyze, find_symbol_at_range};
use crate::ts_api::{self, TsDecl, TsTypes};
use crate::vfs_fs::VfsFileSystem;
use crate::workspace_packages::{self, WorkspacePackages};
use language_core::{
    filesystem, ByteRange, CallGraphOptions, CallGraphResult, DefinitionKind, DefinitionOptions,
    DefinitionResult, EntryPoint, EntryPointRule, FileReferences, FlowOptions, FlowResult,
    FunctionEntry, FunctionRef, MutationOptions, MutationsResult, ReferencesResult, SemanticResult,
    SpecifierOptions, SpecifierStyle, SymbolLocation, UnparsedFile,
};
use oxc_resolver::{
    Resolution, ResolveError, ResolveOptions, Resolver, ResolverGeneric, TsconfigDiscovery,
    TsconfigOptions, TsconfigReferences,
};
use parking_lot::RwLock;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use vfs::{VfsFileType, VfsPath};

/// Strategy for discovering workspace files during indexing.
///
/// The choice depends entirely on where the source of truth lives:
///
/// - Use [`WorkspaceWalker::Ignore`] when the fs_root is a real disk
///   (the CLI's `PhysicalFS`). `.gitignore` is honored, hidden
///   directories are skipped — the sensible default for on-disk codemod
///   runs.
/// - Use [`WorkspaceWalker::Vfs`] when the fs_root is a virtual
///   filesystem (e.g. pg_ast_grep's `MemoryFS` seeded from a database
///   manifest). `ignore::WalkBuilder` can't see entries that only exist
///   in memory, so we recurse through `VfsPath::read_dir` instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceWalker {
    #[default]
    Ignore,
    Vfs,
}

/// Module-resolution pair (primary + fallback) dispatched by the backing
/// storage. The real-disk variant is the historical default; the VFS
/// variant wires a [`VfsFileSystem`] into [`oxc_resolver::ResolverGeneric`]
/// so tsconfig discovery, `extends` chains, path aliases, and target
/// existence checks all observe the virtual filesystem instead of the
/// real disk. Needed for pg_ast_grep, which runs the analyzer against a
/// `MemoryFS` seeded from a database snapshot at a specific commit.
///
/// `nearest` discovers the tsconfig closest to the importing file. It is
/// consulted for bare specifiers the primary (root-tsconfig) resolver
/// can't resolve, so `paths` declared in a nested tsconfig (e.g.
/// `packages/tsconfig.json` in a monorepo whose root tsconfig only covers
/// tooling) still apply.
/// tsconfig `paths`: each alias pattern with its target patterns.
type TsconfigPaths = Vec<(String, Vec<String>)>;

enum DualResolver {
    Physical {
        primary: Resolver,
        fallback: Resolver,
        nearest: Resolver,
    },
    Vfs {
        primary: ResolverGeneric<VfsFileSystem>,
        fallback: ResolverGeneric<VfsFileSystem>,
        nearest: ResolverGeneric<VfsFileSystem>,
    },
}

impl DualResolver {
    fn resolve_primary(
        &self,
        directory: &Path,
        specifier: &str,
    ) -> Result<Resolution, ResolveError> {
        match self {
            Self::Physical { primary, .. } => primary.resolve(directory, specifier),
            Self::Vfs { primary, .. } => primary.resolve(directory, specifier),
        }
    }

    fn resolve_fallback(
        &self,
        directory: &Path,
        specifier: &str,
    ) -> Result<Resolution, ResolveError> {
        match self {
            Self::Physical { fallback, .. } => fallback.resolve(directory, specifier),
            Self::Vfs { fallback, .. } => fallback.resolve(directory, specifier),
        }
    }

    fn resolve_nearest_tsconfig(
        &self,
        directory: &Path,
        specifier: &str,
    ) -> Result<Resolution, ResolveError> {
        match self {
            Self::Physical { nearest, .. } => nearest.resolve(directory, specifier),
            Self::Vfs { nearest, .. } => nearest.resolve(directory, specifier),
        }
    }

    /// The `paths` of the tsconfig file at `path` (after `extends`), with
    /// the directory relative targets are resolved against. None when there
    /// is no such file.
    fn tsconfig_paths(&self, path: &Path) -> Option<(PathBuf, TsconfigPaths)> {
        let tsconfig = match self {
            Self::Physical { nearest, .. } => nearest.resolve_tsconfig(path),
            Self::Vfs { nearest, .. } => nearest.resolve_tsconfig(path),
        }
        .ok()?;
        let base = tsconfig
            .compiler_options
            .base_url
            .clone()
            .unwrap_or_else(|| path.parent().unwrap_or(path).to_path_buf());
        let paths = tsconfig
            .compiler_options
            .paths
            .as_ref()
            .map(|paths| {
                paths
                    .iter()
                    .map(|(key, targets)| (key.clone(), targets.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Some((base, paths))
    }
}

/// Accurate semantic analyzer with workspace-wide lazy indexing.
pub struct AccurateAnalyzer {
    /// Symbol cache for indexed files
    cache: SymbolCache,
    /// Workspace root directory
    workspace_root: PathBuf,
    /// Module resolver pair (primary with tsconfig, plus a tsconfig-free
    /// fallback for relative imports when tsconfig `extends` is broken).
    resolver: DualResolver,
    /// Files that have been fully indexed
    indexed_files: RwLock<HashSet<PathBuf>>,
    /// Files currently being indexed (to prevent cycles)
    indexing_in_progress: RwLock<HashSet<PathBuf>>,
    /// Virtual filesystem root for file operations
    fs_root: VfsPath,
    /// Strategy for discovering workspace files during bulk indexing.
    walker: WorkspaceWalker,
    /// Value-flow facts per file, built on the first data-flow or call-graph query.
    flows: FlowCache,
    /// Every call in the workspace, built on the first callers query.
    callers: CallerIndexSlot,
    /// Every module reference in the workspace, by the file it resolves to:
    /// built on the first `importers()` query and kept, like the callers
    /// table, so files rewritten later in the run still count as importers.
    importers_index: language_flow::once::BuildOnce<ImportersIndex>,
    /// Files that failed to parse, with the parser's message. A file here is
    /// not parsed again until new content for it arrives through `process_file`.
    parse_errors: RwLock<HashMap<PathBuf, String>>,
    /// Set once a query needs facts for every workspace file (the callers
    /// table): from then on, indexing a file builds its facts in the same parse.
    flow_everywhere: AtomicBool,
    /// `canonicalize()` results, so a path is resolved once per run rather
    /// than on every lookup. Only successes are kept: a path that does not
    /// exist yet is tried again.
    canonical_paths: RwLock<HashMap<PathBuf, PathBuf>>,
    /// Files whose content could not be read; not tried again until `clear()`.
    unreadable_files: RwLock<HashSet<PathBuf>>,
    /// Workspace packages by name, read on the first bare specifier the
    /// resolver cannot find (no `node_modules`, or an unbuilt `dist/`).
    workspace_packages: RwLock<Option<Arc<WorkspacePackages>>>,
    /// `resolve_import` answers by (importing directory, specifier, name). Emptied
    /// whenever a file gets new content, since exports may have moved.
    import_targets: RwLock<HashMap<ImportKey, Option<(PathBuf, ByteRange)>>>,
    /// `resolve_module` answers by (importing directory, specifier).
    module_paths: RwLock<HashMap<(PathBuf, String), Option<PathBuf>>>,
    /// The TypeScript checker, found on first use (spike: `ts_api`).
    types: std::sync::OnceLock<Option<TsTypes>>,
    /// Types were turned on (`types: "ts7"`).
    types_enabled: AtomicBool,
    /// Texts the run told this analyzer about, for the TypeScript checker.
    type_texts: Arc<ts_api::FileTexts>,
    /// The selector's `semanticAnalysis.entryPoints`.
    entry_point_rules: RwLock<Vec<EntryPointRule>>,
    /// By package directory: the declarations its public entry modules
    /// export (file, name range), with each one's label (`@acme/ui#Button`).
    package_exports: RwLock<HashMap<PathBuf, Arc<PackageExports>>>,
}

/// Importing file's directory, module specifier, export name.
type ImportKey = (PathBuf, String, String);

impl AccurateAnalyzer {
    /// Create a new accurate analyzer for a workspace.
    ///
    /// Uses the real filesystem (PhysicalFS) with the workspace root.
    #[allow(dead_code)]
    pub fn new(workspace_root: PathBuf) -> Self {
        // Canonicalize workspace root to handle symlinks (e.g., /var -> /private/var on macOS)
        let canonical_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.clone());
        let fs_root = filesystem::physical_path(&canonical_root);
        Self::new_with_fs(canonical_root, fs_root)
    }

    /// Create a new accurate analyzer with a custom virtual filesystem.
    ///
    /// # Arguments
    ///
    /// * `workspace_root` - The workspace root path for module resolution
    /// * `fs_root` - The virtual filesystem root to use for file operations
    pub fn new_with_fs(workspace_root: PathBuf, fs_root: VfsPath) -> Self {
        Self::new_with_fs_and_walker(workspace_root, fs_root, WorkspaceWalker::default())
    }

    /// Create a new accurate analyzer with a custom virtual filesystem and
    /// an explicit workspace walker strategy. Pick [`WorkspaceWalker::Vfs`]
    /// when `fs_root` is a MemoryFS (or any other virtual fs whose entries
    /// aren't visible on disk). Under `Vfs`, tsconfig discovery and all
    /// `oxc_resolver` file I/O go through `fs_root` as well — drop
    /// tsconfig blobs into that VFS before instantiating the analyzer and
    /// path aliases / `extends` chains will resolve correctly.
    pub fn new_with_fs_and_walker(
        workspace_root: PathBuf,
        fs_root: VfsPath,
        walker: WorkspaceWalker,
    ) -> Self {
        let resolver = match walker {
            WorkspaceWalker::Ignore => Self::build_physical_resolver(&workspace_root),
            WorkspaceWalker::Vfs => Self::build_vfs_resolver(&workspace_root, &fs_root),
        };

        Self {
            cache: SymbolCache::new(),
            workspace_root,
            resolver,
            indexed_files: RwLock::new(HashSet::new()),
            indexing_in_progress: RwLock::new(HashSet::new()),
            fs_root,
            walker,
            flows: FlowCache::default(),
            callers: CallerIndexSlot::default(),
            importers_index: Default::default(),
            parse_errors: RwLock::new(HashMap::new()),
            flow_everywhere: AtomicBool::new(false),
            canonical_paths: RwLock::new(HashMap::new()),
            unreadable_files: RwLock::new(HashSet::new()),
            workspace_packages: RwLock::new(None),
            import_targets: RwLock::new(HashMap::new()),
            module_paths: RwLock::new(HashMap::new()),
            types: std::sync::OnceLock::new(),
            types_enabled: AtomicBool::new(false),
            type_texts: Default::default(),
            entry_point_rules: Default::default(),
            package_exports: Default::default(),
        }
    }

    /// Build the real-disk resolver pair. Keeps the historical behavior:
    /// tsconfig is discovered on real disk via `PathBuf::exists`, and the
    /// fallback strips tsconfig entirely so relative imports still
    /// resolve even when `extends` points to a missing package.
    fn build_physical_resolver(workspace_root: &Path) -> DualResolver {
        let tsconfig_path = workspace_root.join("tsconfig.json");
        let tsconfig = if tsconfig_path.exists() {
            Some(TsconfigDiscovery::Manual(TsconfigOptions {
                config_file: tsconfig_path,
                references: TsconfigReferences::Auto,
            }))
        } else {
            Some(TsconfigDiscovery::Auto)
        };

        let primary = Resolver::new(ResolveOptions {
            tsconfig,
            ..default_resolve_options()
        });
        let fallback = Resolver::new(default_resolve_options());
        let nearest = Resolver::new(ResolveOptions {
            tsconfig: Some(TsconfigDiscovery::Auto),
            ..default_resolve_options()
        });

        DualResolver::Physical {
            primary,
            fallback,
            nearest,
        }
    }

    /// Build a VFS-backed resolver pair. Tsconfig discovery checks the
    /// passed-in `fs_root` rather than real disk; when a
    /// `<workspace_root>/tsconfig.json` entry exists in the VFS, the
    /// primary resolver is configured to parse it through
    /// [`VfsFileSystem`]. The fallback remains VFS-backed too — any file
    /// the analyzer touches must live somewhere in `fs_root`, including
    /// package.json siblings consulted during module resolution.
    fn build_vfs_resolver(workspace_root: &Path, fs_root: &VfsPath) -> DualResolver {
        let tsconfig = vfs_tsconfig_discovery(workspace_root, fs_root);

        let primary_opts = ResolveOptions {
            tsconfig,
            ..default_resolve_options()
        };
        let fallback_opts = default_resolve_options();

        let primary = ResolverGeneric::new_with_file_system(
            VfsFileSystem::with_root(fs_root.clone()),
            primary_opts,
        );
        let fallback = ResolverGeneric::new_with_file_system(
            VfsFileSystem::with_root(fs_root.clone()),
            fallback_opts,
        );
        let nearest = ResolverGeneric::new_with_file_system(
            VfsFileSystem::with_root(fs_root.clone()),
            ResolveOptions {
                tsconfig: Some(TsconfigDiscovery::Auto),
                ..default_resolve_options()
            },
        );

        DualResolver::Vfs {
            primary,
            fallback,
            nearest,
        }
    }

    /// The canonical form of a path (symlinks resolved), or the path itself
    /// when it cannot be canonicalized.
    fn canonical_path(&self, path: &Path) -> PathBuf {
        if let Some(canonical) = self.canonical_paths.read().get(path) {
            return canonical.clone();
        }
        match path.canonicalize() {
            Ok(canonical) => {
                self.canonical_paths
                    .write()
                    .insert(path.to_path_buf(), canonical.clone());
                canonical
            }
            Err(_) => path.to_path_buf(),
        }
    }

    /// Read file content using the virtual filesystem.
    ///
    /// Path handling is dispatched by [`Self::walker`] because the two
    /// supported backings have opposite conventions:
    ///
    /// * [`WorkspaceWalker::Ignore`] pairs with a `PhysicalFS` rooted
    ///   at `workspace_root`. PhysicalFS rebases every path to its
    ///   root, so `/home/user/project/src/foo.ts` must be stripped
    ///   down to `src/foo.ts` before being joined — otherwise
    ///   PhysicalFS ends up reading `<root>/<root>/src/foo.ts`.
    ///
    /// * [`WorkspaceWalker::Vfs`] pairs with an in-memory VFS seeded
    ///   by pg_ast_grep at absolute sandboxed paths (e.g.
    ///   `/app/src/foo.ts`). Here the absolute form IS the storage
    ///   key; stripping `workspace_root` would cause every cross-file
    ///   `ensure_indexed` to miss, silently losing barrel re-export
    ///   chains (the insights/CLI parity bug we're fixing).
    fn read_file(&self, file_path: &Path) -> Result<String, JsSemanticError> {
        let vfs_path = self.vfs_path(file_path).ok_or_else(|| {
            JsSemanticError::Internal(format!("Failed to join path: {}", file_path.display()))
        })?;

        filesystem::read_to_string(&vfs_path)
            .map_err(|e| JsSemanticError::Internal(format!("Failed to read file: {}", e)))
    }

    /// Where a path lives in the analyzer's filesystem (see [`Self::read_file`]).
    fn vfs_path(&self, file_path: &Path) -> Option<VfsPath> {
        let path_str: std::borrow::Cow<str> = match self.walker {
            WorkspaceWalker::Ignore => {
                let relative_path = file_path
                    .strip_prefix(&self.workspace_root)
                    .unwrap_or(file_path);
                relative_path.to_string_lossy().into_owned().into()
            }
            WorkspaceWalker::Vfs => file_path.to_string_lossy(),
        };
        self.fs_root.join(&*path_str).ok()
    }

    /// Index a file and its dependencies lazily.
    fn ensure_indexed(&self, file_path: &Path) -> SemanticResult<()> {
        self.ensure_indexed_with(file_path, false)
    }

    /// Index a file and its dependencies lazily. `with_flow` also builds the
    /// file's value-flow facts from the same parse (its dependencies get
    /// theirs only when every file needs them, see `flow_everywhere`).
    fn ensure_indexed_with(&self, file_path: &Path, with_flow: bool) -> SemanticResult<()> {
        let canonical = self.canonical_path(file_path);

        // Claim the file. Another thread may be parsing it: that holds no
        // locks and waits on nothing, so wait for it to finish.
        let mut waits = 0u32;
        loop {
            if self.indexed_files.read().contains(&canonical) {
                return Ok(());
            }
            // A file that could not be read is never retried.
            if self.unreadable_files.read().contains(&canonical) {
                return Ok(());
            }
            // Known not to parse: say so again without re-reading it.
            if let Some(message) = self.parse_errors.read().get(&canonical) {
                return Err(JsSemanticError::ParseError {
                    path: canonical,
                    message: message.clone(),
                }
                .into());
            }
            if self.indexing_in_progress.write().insert(canonical.clone()) {
                break;
            }
            // A parse takes microseconds to milliseconds: yield first, then
            // sleep, so a long wait does not hold a core.
            waits += 1;
            if waits < 64 {
                std::thread::yield_now();
            } else {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        }
        // If the parse panics, give the file up (as if unreadable) rather
        // than leave it claimed for threads waiting on it.
        let claim = Claim {
            analyzer: self,
            path: &canonical,
        };

        // Read and parse the file using the virtual filesystem
        let content = match self.read_file(&canonical) {
            Ok(content) => content,
            Err(error) => {
                std::mem::forget(claim);
                self.unreadable_files.write().insert(canonical.clone());
                self.indexing_in_progress.write().remove(&canonical);
                return Err(error.into());
            }
        };

        let with_flow = with_flow || self.flow_everywhere.load(Ordering::Relaxed);
        let (file_symbols, flow) = match analyze(&canonical, &content, with_flow) {
            Ok(analyzed) => analyzed,
            Err(error) => {
                if let JsSemanticError::ParseError { message, .. } = &error {
                    self.parse_errors
                        .write()
                        .insert(canonical.clone(), message.clone());
                }
                std::mem::forget(claim);
                self.indexing_in_progress.write().remove(&canonical);
                return Err(error.into());
            }
        };
        std::mem::forget(claim);
        let imports: Vec<String> = file_symbols
            .imports
            .iter()
            .map(|import| import.module_specifier.clone())
            .collect();

        // Store in cache and mark as indexed before following imports, so
        // an import cycle (or another thread) finds this file done.
        if let Some(flow) = flow {
            self.flows
                .insert(canonical.clone(), file_symbols.content_hash, flow);
        }
        self.cache.insert(canonical.clone(), file_symbols, content);
        self.indexed_files.write().insert(canonical.clone());
        self.indexing_in_progress.write().remove(&canonical);

        // Index imported files
        for specifier in &imports {
            if let Ok(resolved) = self.resolve_module(specifier, &canonical) {
                let _ = self.ensure_indexed(&resolved);
            }
        }

        Ok(())
    }

    /// Resolve a module specifier to a file path.
    ///
    /// Tries the primary resolver (with tsconfig) first. For relative or absolute
    /// specifiers, falls back to plain file-system resolution when the primary
    /// resolver fails — this handles cases where tsconfig `extends` references
    /// missing packages (e.g., when `node_modules` is not installed) which would
    /// otherwise break resolution of simple local imports like `./utils`.
    ///
    /// Bare specifiers (e.g., `lodash`, `@acme/utils`) are NOT retried with the
    /// tsconfig-free fallback because their resolution intentionally depends on
    /// tsconfig `paths` or `node_modules`. They are retried with the tsconfig
    /// nearest to the importing file, so nested `paths` aliases resolve.
    pub fn resolve_module(
        &self,
        specifier: &str,
        from_path: &Path,
    ) -> Result<PathBuf, JsSemanticError> {
        let from_dir = from_path.parent().unwrap_or(&self.workspace_root);
        let key = (from_dir.to_path_buf(), specifier.to_string());
        let remembered = self.module_paths.read().get(&key).cloned();
        let resolved = match remembered {
            Some(resolved) => resolved,
            None => {
                let resolved = self.resolve_module_uncached(specifier, from_path).ok();
                self.module_paths.write().insert(key, resolved.clone());
                resolved
            }
        };
        resolved.ok_or_else(|| JsSemanticError::ModuleResolution {
            specifier: specifier.to_string(),
            from_path: from_path.to_path_buf(),
        })
    }

    /// [`Self::resolve_module`] without the memo. The answer depends on the
    /// importing file's directory only; the resolvers cache what they read
    /// from disk, so remembering the answer changes nothing.
    fn resolve_module_uncached(
        &self,
        specifier: &str,
        from_path: &Path,
    ) -> Result<PathBuf, JsSemanticError> {
        let from_dir = from_path.parent().unwrap_or(&self.workspace_root);

        match self.resolver.resolve_primary(from_dir, specifier) {
            Ok(resolution) => Ok(resolution.into_path_buf()),
            Err(_) if specifier.starts_with('.') || specifier.starts_with('/') => {
                match self.resolver.resolve_fallback(from_dir, specifier) {
                    Ok(resolution) => Ok(resolution.into_path_buf()),
                    Err(_) => Err(JsSemanticError::ModuleResolution {
                        specifier: specifier.to_string(),
                        from_path: from_path.to_path_buf(),
                    }),
                }
            }
            Err(_) => match self.resolver.resolve_nearest_tsconfig(from_dir, specifier) {
                Ok(resolution) => Ok(resolution.into_path_buf()),
                Err(_) => self.resolve_workspace_package(specifier).ok_or_else(|| {
                    JsSemanticError::ModuleResolution {
                        specifier: specifier.to_string(),
                        from_path: from_path.to_path_buf(),
                    }
                }),
            },
        }
    }

    /// A bare specifier naming a package of this workspace (root
    /// `package.json` `workspaces`, or `pnpm-workspace.yaml`), resolved to
    /// its source when nothing is installed or built.
    fn resolve_workspace_package(&self, specifier: &str) -> Option<PathBuf> {
        let packages = self.workspace_packages();
        let (dir, subpath) = packages.find(specifier)?;
        let read = |path: &Path| self.read_file(path).ok();
        workspace_packages::entry_candidates(dir, subpath, &read)
            .into_iter()
            .find_map(|candidate| {
                self.resolver
                    .resolve_fallback(dir, &format!("./{candidate}"))
                    .ok()
                    .map(|resolution| resolution.into_path_buf())
            })
    }

    fn workspace_packages(&self) -> Arc<WorkspacePackages> {
        if let Some(packages) = self.workspace_packages.read().as_ref() {
            return Arc::clone(packages);
        }
        let read = |path: &Path| self.read_file(path).ok();
        let list_dirs = |path: &Path| self.list_dirs(path);
        let packages = Arc::new(WorkspacePackages::discover(
            &self.workspace_root,
            &read,
            &list_dirs,
        ));
        *self.workspace_packages.write() = Some(Arc::clone(&packages));
        packages
    }

    /// The directories directly inside `dir`, through the analyzer's
    /// filesystem.
    fn list_dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let Some(vfs_dir) = self.vfs_path(dir) else {
            return Vec::new();
        };
        let Ok(children) = vfs_dir.read_dir() else {
            return Vec::new();
        };
        children
            .filter(|child| {
                child
                    .metadata()
                    .is_ok_and(|meta| meta.file_type == VfsFileType::Directory)
            })
            .filter_map(|child| {
                let name = child.filename();
                (!name.is_empty()).then(|| dir.join(name))
            })
            .collect()
    }

    /// Process a file notification (updates the cache).
    ///
    /// Early-outs when the canonical path is already indexed AND the
    /// cached content matches the incoming content. This is what makes
    /// repeated `get_definition` / `find_references` queries on the
    /// same file cheap — otherwise every cross-file query re-runs the
    /// full oxc parse + semantic pass on the originating file, which
    /// on a 7k-file workspace batch stacks up to seconds of redundant
    /// work in the rayon par_iter.
    ///
    /// Content comparison uses [`SymbolCache::content_matches`] so the
    /// byte check happens under the cache's read lock — no clone of
    /// `FileSymbols` or the cached `String` on the hot path. Mismatched
    /// content still triggers a re-parse, preserving the "this
    /// notification is authoritative" contract for callers that
    /// legitimately hand new content in (e.g. editor-driven LSP use).
    pub fn process_file(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        self.type_texts
            .set(&self.canonical_path(file_path), content);
        self.process_file_with(file_path, content, false)
    }

    /// [`Self::process_file`] for a data-flow or call-graph query: new
    /// content gets its value-flow facts from the same parse.
    fn process_file_with(
        &self,
        file_path: &Path,
        content: &str,
        with_flow: bool,
    ) -> SemanticResult<()> {
        let canonical = self.canonical_path(file_path);

        if self.indexed_files.read().contains(&canonical)
            && self.cache.content_matches(&canonical, content)
        {
            return Ok(());
        }

        let (file_symbols, flow) = analyze(file_path, content, with_flow)?;
        self.parse_errors.write().remove(&canonical);
        if let Some(flow) = flow {
            self.flows
                .insert(canonical.clone(), file_symbols.content_hash, flow);
        }
        self.cache
            .insert(canonical.clone(), file_symbols, content.to_string());
        self.indexed_files.write().insert(canonical);
        // New content may move exports.
        self.import_targets.write().clear();
        Ok(())
    }

    /// Get the definition of a symbol at the given range.
    pub fn get_definition(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: DefinitionOptions,
    ) -> SemanticResult<Option<DefinitionResult>> {
        // Ensure the file is indexed
        self.process_file(file_path, content)?;

        let canonical = self.canonical_path(file_path);
        let (file_symbols, _) =
            self.cache
                .get(&canonical)
                .ok_or_else(|| JsSemanticError::FileNotCached {
                    path: canonical.clone(),
                })?;

        // Check if this is a reference
        let reference = file_symbols
            .references
            .iter()
            .find(|r| r.range.start <= range.start && r.range.end >= range.end);

        if let Some(ref_info) = reference {
            if let Some(symbol) = file_symbols.find_symbol_by_id(ref_info.symbol_id) {
                return Ok(Some(DefinitionResult::new(
                    SymbolLocation::new(
                        canonical.clone(),
                        symbol.range,
                        symbol.kind,
                        symbol.name.clone(),
                    ),
                    content.to_string(),
                    DefinitionKind::Local,
                )));
            }
        }

        // Check if this is an import
        if let Some(import) = file_symbols.find_import_at(range) {
            // If resolve_external is false, return the import statement directly
            if !options.resolve_external {
                return Ok(Some(DefinitionResult::new(
                    SymbolLocation::new(
                        canonical,
                        import.range,
                        language_core::SymbolKind::Import,
                        import.local_name.clone(),
                    ),
                    content.to_string(),
                    DefinitionKind::Import,
                )));
            }

            // Try to resolve the import, following re-export chains
            let target = self.resolve_import_target(&canonical, import);
            match target.and_then(|t| self.target_definition(t, DefinitionKind::External)) {
                Some(def) => return Ok(Some(def)),
                None => {
                    // Module couldn't be resolved, return the import statement
                    return Ok(Some(DefinitionResult::new(
                        SymbolLocation::new(
                            canonical,
                            import.range,
                            language_core::SymbolKind::Import,
                            import.local_name.clone(),
                        ),
                        content.to_string(),
                        DefinitionKind::Import,
                    )));
                }
            }
        }

        // Check if this is a name inside a re-export (`export { a as b } from './x'`)
        if let Some(export) = file_symbols.find_reexport_specifier_at(range) {
            let site = DefinitionResult::new(
                SymbolLocation::new(
                    canonical.clone(),
                    export.specifier_range.unwrap_or(export.range),
                    language_core::SymbolKind::Export,
                    export.name.clone(),
                ),
                content.to_string(),
                DefinitionKind::Import,
            );
            if !options.resolve_external {
                return Ok(Some(site));
            }
            let mut visited = HashSet::new();
            return Ok(Some(
                match self.follow_export(&canonical, &file_symbols, export, &mut visited) {
                    ExportTarget::Symbol { path, symbol } => self
                        .target_definition(
                            ExportTarget::Symbol { path, symbol },
                            DefinitionKind::External,
                        )
                        .unwrap_or(site),
                    // Could not follow the source module: stay on the specifier.
                    ExportTarget::Site { .. } => site,
                },
            ));
        }

        // Check if this is a direct symbol
        if let Some(symbol) = find_symbol_at_range(&file_symbols, range) {
            return Ok(Some(DefinitionResult::new(
                SymbolLocation::new(canonical, symbol.range, symbol.kind, symbol.name.clone()),
                content.to_string(),
                DefinitionKind::Local,
            )));
        }

        Ok(None)
    }

    /// Find all references to a symbol at the given range.
    /// Returns references grouped by file.
    pub fn find_references(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        // Ensure the file is indexed
        self.process_file(file_path, content)?;

        let canonical = self.canonical_path(file_path);
        let (file_symbols, _) =
            self.cache
                .get(&canonical)
                .ok_or_else(|| JsSemanticError::FileNotCached {
                    path: canonical.clone(),
                })?;

        // Group locations by file path
        let mut files_map: HashMap<PathBuf, Vec<SymbolLocation>> = HashMap::new();

        // Find the symbol at the given range
        let symbol = find_symbol_at_range(&file_symbols, range);

        // A name inside `export { a } from './x'` has no local symbol: answer
        // for the declaration it re-exports instead.
        if symbol.is_none() {
            if let Some(export) = file_symbols.find_reexport_specifier_at(range) {
                let mut visited = HashSet::new();
                if let ExportTarget::Symbol { path, symbol } =
                    self.follow_export(&canonical, &file_symbols, export, &mut visited)
                {
                    if path != canonical {
                        if let Some((_, target_content)) = self.cache.get(&path) {
                            return self.find_references(&path, &target_content, symbol.range);
                        }
                    }
                }
            }
        }

        if let Some(sym) = symbol {
            // Find all local references (excluding the definition itself)
            for reference in file_symbols.find_references_to(sym.symbol_id) {
                files_map
                    .entry(canonical.clone())
                    .or_default()
                    .push(SymbolLocation::new(
                        canonical.clone(),
                        reference.range,
                        sym.kind,
                        sym.name.clone(),
                    ));
            }

            // Check if this symbol is exported
            let is_exported = file_symbols
                .exports
                .iter()
                .any(|e| e.local_symbol_id == Some(sym.symbol_id));

            if is_exported {
                // Index all files in workspace that might import this
                self.index_workspace_files()?;

                let mut memo: HashMap<(PathBuf, String), Option<ExportTarget>> = HashMap::new();
                let lands_on_sym = |target: Option<ExportTarget>| {
                    matches!(
                        target,
                        Some(ExportTarget::Symbol { path, symbol })
                            if path == canonical && symbol.symbol_id == sym.symbol_id
                    )
                };

                // Search all indexed files for imports / re-exports that
                // reach this symbol, directly or through barrels.
                for cached_path in self.cache.files() {
                    if cached_path == canonical {
                        continue;
                    }

                    let Some((other_symbols, _)) = self.cache.get(&cached_path) else {
                        continue;
                    };

                    for import in &other_symbols.imports {
                        let Ok(module) =
                            self.resolve_module(&import.module_specifier, &cached_path)
                        else {
                            continue;
                        };
                        // `import * as ns`: each `ns.name` / `ns["name"]`
                        // whose export lands on this symbol.
                        if import.is_namespace {
                            let Some((_, other_content)) = self.cache.get(&cached_path) else {
                                continue;
                            };
                            let ns_symbol = other_symbols.symbols.iter().find(|s| {
                                s.name == import.local_name
                                    && import.range.start <= s.range.start
                                    && import.range.end >= s.range.end
                            });
                            let Some(ns_symbol) = ns_symbol else {
                                continue;
                            };
                            for use_site in other_symbols.find_references_to(ns_symbol.symbol_id) {
                                let Some((name, name_range)) =
                                    member_after(&other_content, use_site.range.end)
                                else {
                                    continue;
                                };
                                if lands_on_sym(self.resolve_export_memo(&module, &name, &mut memo))
                                {
                                    files_map.entry(cached_path.clone()).or_default().push(
                                        SymbolLocation::new(
                                            cached_path.clone(),
                                            name_range,
                                            sym.kind,
                                            name,
                                        ),
                                    );
                                }
                            }
                            continue;
                        }
                        let name = import_export_name(import);
                        if !lands_on_sym(self.resolve_export_memo(&module, name, &mut memo)) {
                            continue;
                        }

                        files_map.entry(cached_path.clone()).or_default().push(
                            SymbolLocation::new(
                                cached_path.clone(),
                                import.range,
                                language_core::SymbolKind::Import,
                                import.local_name.clone(),
                            ),
                        );

                        // Also find references to this import in the file
                        let import_symbol = other_symbols.symbols.iter().find(|s| {
                            s.name == import.local_name
                                && import.range.start <= s.range.start
                                && import.range.end >= s.range.end
                        });

                        if let Some(imp_sym) = import_symbol {
                            for ref_in_other in other_symbols.find_references_to(imp_sym.symbol_id)
                            {
                                files_map.entry(cached_path.clone()).or_default().push(
                                    SymbolLocation::new(
                                        cached_path.clone(),
                                        ref_in_other.range,
                                        sym.kind,
                                        import.local_name.clone(),
                                    ),
                                );
                            }
                        }
                    }

                    // Re-exports (`export { a } from './x'`) that forward this symbol
                    for export in &other_symbols.exports {
                        let (Some(source), Some(local_name), Some(local_range)) = (
                            &export.re_export_from,
                            &export.local_name,
                            export.local_range,
                        ) else {
                            continue;
                        };
                        let Ok(module) = self.resolve_module(source, &cached_path) else {
                            continue;
                        };
                        if lands_on_sym(self.resolve_export_memo(&module, local_name, &mut memo)) {
                            files_map.entry(cached_path.clone()).or_default().push(
                                SymbolLocation::new(
                                    cached_path.clone(),
                                    local_range,
                                    language_core::SymbolKind::Export,
                                    local_name.clone(),
                                ),
                            );
                        }
                    }
                }
            }
        }

        // Convert to ReferencesResult
        let mut result = ReferencesResult::new();
        for (path, locations) in files_map {
            // Get content for each file
            let file_content = if path == canonical {
                content.to_string()
            } else {
                self.cache
                    .get(&path)
                    .map(|(_, c)| c.to_string())
                    .unwrap_or_default()
            };

            result.add_file(FileReferences::new(path, file_content, locations));
        }

        Ok(result)
    }

    /// Index all JavaScript/TypeScript files in the workspace. Dispatches
    /// to the real-disk `ignore` walker or a VFS-backed recursion based
    /// on [`Self::walker`].
    fn index_workspace_files(&self) -> SemanticResult<()> {
        match self.walker {
            WorkspaceWalker::Ignore => self.index_workspace_files_ignore(),
            WorkspaceWalker::Vfs => self.index_workspace_files_vfs(),
        }
    }

    /// Real-disk walk using `ignore::WalkBuilder`. Honors `.gitignore` and
    /// hidden-file exclusion. Only useful when the underlying fs_root is
    /// a PhysicalFS whose tree matches `self.workspace_root` on disk.
    fn index_workspace_files_ignore(&self) -> SemanticResult<()> {
        // The same walk, on several threads: directories are read in
        // parallel, the set of files is the same.
        let files = parking_lot::Mutex::new(Vec::new());
        ignore::WalkBuilder::new(&self.workspace_root)
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .build_parallel()
            .run(|| {
                Box::new(|entry| {
                    if let Ok(entry) = entry {
                        let path = entry.path();
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if path.is_file()
                            && matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs")
                        {
                            files.lock().push(entry.into_path());
                        }
                    }
                    ignore::WalkState::Continue
                })
            });
        self.index_files(&files.into_inner());

        Ok(())
    }

    /// Virtual-filesystem walk. Recurses from `self.fs_root` via
    /// `read_dir`, matches indexable extensions, and rebuilds absolute
    /// paths keyed under `self.workspace_root` so downstream cache keys
    /// line up with `ensure_indexed`. Silently skips entries whose
    /// metadata can't be read — a stub created without content still
    /// counts as a file for indexing purposes once the fetcher fills it.
    fn index_workspace_files_vfs(&self) -> SemanticResult<()> {
        let workspace_prefix = self.workspace_root.to_string_lossy().into_owned();
        // Resolve the VFS entry that corresponds to `workspace_root`. We
        // prefer a VFS-aware prefix so codemods seeded from a database
        // manifest whose paths are `/app/src/foo.ts` etc. still line up.
        let start = self
            .fs_root
            .join(workspace_prefix.trim_start_matches('/'))
            .unwrap_or_else(|_| self.fs_root.clone());
        let mut files = Vec::new();
        walk_vfs_tree(&start, &workspace_prefix, &mut |path: &Path| {
            files.push(path.to_path_buf());
        });
        self.index_files(&files);
        Ok(())
    }

    /// `.mdx` and `.md` files in the workspace (honoring `.gitignore`), for
    /// the imports they hold. Only on the real filesystem; a VFS workspace
    /// answers none.
    fn markup_files(&self) -> Vec<PathBuf> {
        if !matches!(self.walker, WorkspaceWalker::Ignore) {
            return Vec::new();
        }
        ignore::WalkBuilder::new(&self.workspace_root)
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .build()
            .flatten()
            .map(|entry| entry.into_path())
            .filter(|path| {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                path.is_file() && matches!(ext, "mdx" | "md")
            })
            .map(|path| self.canonical_path(&path))
            .collect()
    }

    /// Index files in parallel. Each file is parsed once, whichever thread
    /// gets to it first.
    fn index_files(&self, files: &[PathBuf]) {
        files.par_iter().for_each(|path| {
            let _ = self.ensure_indexed(path);
        });
    }

    /// [`Self::resolve_module`] for codemods: the canonical path, or None.
    pub fn resolve_module_path(&self, from_file: &Path, specifier: &str) -> Option<PathBuf> {
        let from = self.canonical_path(from_file);
        self.resolve_module(specifier, &from)
            .ok()
            .map(|path| self.canonical_path(&path))
    }

    /// What `export_name` of the module at `module_path` lands on, following
    /// re-exports: the declaration, or the export site where the chain stops.
    pub fn resolve_export_definition(
        &self,
        module_path: &Path,
        export_name: &str,
    ) -> Option<DefinitionResult> {
        let target = self.resolve_export(module_path, export_name, &mut HashSet::new())?;
        self.target_definition(target, DefinitionKind::External)
    }

    /// The workspace package a file belongs to: the nearest `package.json`
    /// at or below the workspace root, read through the analyzer's
    /// filesystem. None outside any package.
    fn package_of(&self, file: &Path) -> Option<PackageInfo> {
        // Paths here are canonical; compare with the canonical root.
        let root = self.canonical_path(&self.workspace_root);
        let mut dir = file.parent();
        while let Some(current) = dir {
            if !current.starts_with(&root) {
                return None;
            }
            if let Ok(text) = self.read_file(&current.join("package.json")) {
                let json: serde_json::Value = serde_json::from_str(&text).ok()?;
                return Some(PackageInfo {
                    dir: current.to_path_buf(),
                    name: json
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(str::to_string),
                    exports: json.get("exports").cloned(),
                    entries: ["main", "module", "types", "typings", "source"]
                        .iter()
                        .filter_map(|field| json.get(*field).and_then(|v| v.as_str()))
                        .map(str::to_string)
                        .collect(),
                });
            }
            if current == root.as_path() {
                return None;
            }
            dir = current.parent();
        }
        None
    }

    /// Every place in the workspace that refers to the module at `target`.
    ///
    /// Answers from a table of every module reference in the workspace,
    /// built on the first query (in parallel, from the cached facts) and
    /// kept until the cache is cleared: a snapshot of the workspace at that
    /// moment, as for `callers()`. A file a codemod rewrites later still
    /// counts as importing what it imported.
    pub fn importers(&self, target: &Path) -> language_core::ImportersResult {
        let target = self.canonical_path(target);
        let index = self.importers_index();
        let files = index
            .by_target
            .get(&target)
            .map(|refs| {
                let mut by_file: Vec<language_core::ImportersFile> = Vec::new();
                for (file, importer) in refs {
                    match by_file.last_mut() {
                        Some(last) if &last.file_path == file => {
                            last.importers.push(importer.clone())
                        }
                        _ => by_file.push(language_core::ImportersFile {
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
                by_file
            })
            .unwrap_or_default();
        language_core::ImportersResult {
            files,
            unparsed: index.unparsed.clone(),
        }
    }

    /// The importers table, built on first use.
    fn importers_index(&self) -> Arc<ImportersIndex> {
        self.importers_index
            .get_or_build(|| self.build_importers_index())
    }

    fn build_importers_index(&self) -> ImportersIndex {
        use language_core::{ImportKind, Importer};
        use rayon::prelude::*;

        let mut files = FlowHost::workspace_files(self);
        files.sort();
        let per_file: Vec<FileRefs> = files
            .into_par_iter()
            .filter_map(|file| {
                let flow = FlowHost::flow(self, &file)?;
                let content = FlowHost::content(self, &file)?;
                let text = |range: ByteRange| {
                    content
                        .get(range.start as usize..range.end as usize)
                        .unwrap_or_default()
                };
                let mut candidates: Vec<(ByteRange, ImportKind, String)> = Vec::new();
                for (&node, import) in &flow.imports {
                    candidates.push((
                        flow.nodes[node as usize].range,
                        ImportKind::Import,
                        import.module.clone(),
                    ));
                }
                for (&node, module) in &flow.requires {
                    candidates.push((
                        flow.nodes[node as usize].range,
                        ImportKind::Require,
                        module.clone(),
                    ));
                }
                for (&node, module) in &flow.dynamic_imports {
                    candidates.push((
                        flow.nodes[node as usize].range,
                        ImportKind::Dynamic,
                        module.clone(),
                    ));
                }
                for call in &flow.calls {
                    if !MOCK_CALLS.contains(&&*call.callee_text) {
                        continue;
                    }
                    let Some(&first) = call.args.first() else {
                        continue;
                    };
                    let literal = text(flow.nodes[first as usize].range);
                    let quoted = literal.len() >= 2
                        && matches!(literal.as_bytes()[0], b'"' | b'\'' | b'`')
                        && literal.as_bytes()[0] == literal.as_bytes()[literal.len() - 1];
                    if quoted {
                        candidates.push((
                            call.range,
                            ImportKind::Mock,
                            literal[1..literal.len() - 1].to_string(),
                        ));
                    }
                }
                if let Some((symbols, _)) = self.cache.get(&file) {
                    for export in &symbols.exports {
                        if let Some(module) = &export.re_export_from {
                            candidates.push((export.range, ImportKind::Export, module.clone()));
                        }
                    }
                }
                // `import "./x"`: no binding, so not in the facts.
                for (range, module) in side_effect_imports(&content) {
                    candidates.push((range, ImportKind::Import, module));
                }

                let mut resolved: HashMap<String, Option<PathBuf>> = HashMap::new();
                let mut refs: Vec<(PathBuf, Importer)> = candidates
                    .into_iter()
                    .filter_map(|(range, kind, specifier)| {
                        let target = resolved
                            .entry(specifier.clone())
                            .or_insert_with(|| self.resolve_module_path(&file, &specifier))
                            .clone()?;
                        Some((
                            target,
                            Importer {
                                range,
                                kind,
                                specifier,
                            },
                        ))
                    })
                    .collect();
                refs.sort_by_key(|(_, i)| (i.range.start, i.range.end));
                refs.dedup_by_key(|(t, i)| (t.clone(), i.range.start, i.range.end));
                Some((file, content, refs))
            })
            .collect();
        // MDX and Markdown import components with ESM lines.
        let markup: Vec<FileRefs> = self
            .markup_files()
            .into_par_iter()
            .filter_map(|file| {
                let content: Arc<str> = self.read_file(&file).ok()?.into();
                let refs = esm_import_lines(&content)
                    .into_iter()
                    .filter_map(|(range, specifier)| {
                        let target = self.resolve_module_path(&file, &specifier)?;
                        Some((
                            target,
                            Importer {
                                range,
                                kind: ImportKind::Import,
                                specifier,
                            },
                        ))
                    })
                    .collect();
                Some((file, content, refs))
            })
            .collect();
        let mut index = ImportersIndex::default();
        for (file, content, refs) in per_file.into_iter().chain(markup) {
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
        index.unparsed = FlowHost::parse_failures(self);
        index
    }

    /// The specifiers another package can import `file` with: its package's
    /// name and `exports` subpaths that resolve to it (checked by resolving
    /// them from the file). Empty outside a named package or when the file
    /// is not public.
    pub fn public_specifiers_of(&self, file: &Path) -> Vec<String> {
        let file = self.canonical_path(file);
        let Some(package) = self.package_of(&file) else {
            return Vec::new();
        };
        let Some(name) = &package.name else {
            return Vec::new();
        };
        let forms = specifier_forms(&file.to_string_lossy().replace('\\', "/"));
        let mut out: Vec<String> = public_specifiers(name, &package, &forms)
            .into_iter()
            .filter(|specifier| self.resolve_module_path(&file, specifier).as_ref() == Some(&file))
            .collect();
        // Named as an entry in package.json: public by declaration, even
        // when the name does not resolve here (no workspace config, no install).
        let declared = package.entries.iter().any(|entry| {
            let path = normalize_lexically(&package.dir.join(entry));
            let path = path.to_string_lossy().replace('\\', "/");
            forms.contains(&path)
        });
        if declared && package.exports.is_none() {
            out.push(name.clone());
        }
        out.sort();
        out.dedup();
        out
    }

    /// A specifier an import in `from_file` can use to reach `target`.
    ///
    /// Candidates are the relative path and every tsconfig `paths` alias
    /// (from the tsconfig nearest to `from_file` up to the workspace root)
    /// whose target directory holds `target`. Each is checked by resolving
    /// it from `from_file`, so a candidate only survives if it reaches
    /// `target`. Then the style of `options.like` (or `options.prefer`)
    /// wins, then the alias sharing the longest prefix with `like`, then the
    /// shortest.
    pub fn specifier_for(
        &self,
        from_file: &Path,
        target: &Path,
        options: &SpecifierOptions,
    ) -> Option<String> {
        let from = self.canonical_path(from_file);
        let target = self.canonical_path(target);
        let from_dir = from.parent()?.to_path_buf();

        let target_str = target.to_string_lossy().replace('\\', "/");
        let forms = specifier_forms(&target_str);

        let mut candidates: Vec<(String, bool)> = Vec::new();
        for form in &forms {
            candidates.push((relative_specifier(&from_dir, Path::new(form)), false));
        }
        // Paths here are canonical (`/private/var/...` for `/var/...` on
        // macOS); compare with the canonical root.
        let root = self.canonical_path(&self.workspace_root);
        let mut dir = Some(from_dir.as_path());
        while let Some(current) = dir {
            if let Some((base, paths)) =
                self.resolver.tsconfig_paths(&current.join("tsconfig.json"))
            {
                for (key, targets) in &paths {
                    for pattern in targets {
                        let pattern_path = normalize_lexically(&base.join(pattern));
                        let pattern_str = pattern_path.to_string_lossy().replace('\\', "/");
                        match (key.split_once('*'), pattern_str.split_once('*')) {
                            (Some((key_head, key_tail)), Some((head, tail))) => {
                                for form in &forms {
                                    if let Some(middle) = form
                                        .strip_prefix(head)
                                        .and_then(|rest| rest.strip_suffix(tail))
                                    {
                                        candidates
                                            .push((format!("{key_head}{middle}{key_tail}"), true));
                                    }
                                }
                            }
                            (None, None) => candidates.push((key.clone(), true)),
                            _ => {}
                        }
                    }
                }
            }
            if current == root.as_path() {
                break;
            }
            dir = current.parent().filter(|p| p.starts_with(&root));
        }

        // Another workspace package: only what it makes public. A relative
        // path or alias into its files would cross its export boundary.
        if let Some(package) = self.package_of(&target) {
            let same_package = self
                .package_of(&from)
                .is_some_and(|own| own.dir == package.dir);
            if !same_package {
                if let Some(name) = &package.name {
                    candidates = public_specifiers(name, &package, &forms)
                        .into_iter()
                        .map(|specifier| (specifier, true))
                        .collect();
                }
            }
        }

        let mut seen = HashSet::new();
        candidates.retain(|(specifier, _)| {
            seen.insert(specifier.clone())
                && self.resolve_module_path(&from, specifier).as_ref() == Some(&target)
        });

        let like = options.like.as_deref();
        let want_alias = match (like, options.prefer) {
            (Some(like), _) => Some(!like.starts_with('.')),
            (None, Some(style)) => Some(style == SpecifierStyle::Alias),
            (None, None) => None,
        };
        let like_has_extension = like.is_some_and(|like| strip_js_extension(like).is_some());
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
            .min_by_key(|(specifier, alias)| {
                (
                    want_alias.is_some_and(|want| want != *alias),
                    like_has_extension != strip_js_extension(specifier).is_some(),
                    std::cmp::Reverse(common(specifier)),
                    specifier.len(),
                    specifier.clone(),
                )
            })
            .map(|(specifier, _)| specifier)
    }

    /// Resolve an import to the declaration it ultimately binds to,
    /// following re-export chains through barrels.
    fn resolve_import_target(
        &self,
        from_path: &Path,
        import: &ImportedSymbol,
    ) -> Option<ExportTarget> {
        if import.is_namespace {
            return None;
        }
        let module = self
            .resolve_module(&import.module_specifier, from_path)
            .ok()?;
        self.resolve_export(&module, import_export_name(import), &mut HashSet::new())
    }

    /// Like [`Self::resolve_export`] with a per-query memo, used when many
    /// imports across the workspace are checked against one symbol.
    fn resolve_export_memo(
        &self,
        module_path: &Path,
        export_name: &str,
        memo: &mut HashMap<(PathBuf, String), Option<ExportTarget>>,
    ) -> Option<ExportTarget> {
        let key = (module_path.to_path_buf(), export_name.to_string());
        if let Some(hit) = memo.get(&key) {
            return hit.clone();
        }
        let result = self.resolve_export(module_path, export_name, &mut HashSet::new());
        memo.insert(key, result.clone());
        result
    }

    /// Find what `export_name` of the module at `module_path` refers to,
    /// following named re-exports (`export { a as b } from`), `export *`,
    /// and local re-exports of imported bindings (`import { a } …; export { a }`).
    /// Returns `None` when the module does not export the name.
    fn resolve_export(
        &self,
        module_path: &Path,
        export_name: &str,
        visited: &mut HashSet<(PathBuf, String)>,
    ) -> Option<ExportTarget> {
        let path = self.canonical_path(module_path);
        if !visited.insert((path.clone(), export_name.to_string())) {
            return None; // re-export cycle
        }

        let _ = self.ensure_indexed(&path);
        let (symbols, _) = self.cache.get(&path)?;

        if let Some(export) = symbols
            .exports
            .iter()
            .find(|e| e.name == export_name && e.name != "*")
        {
            return Some(self.follow_export(&path, &symbols, export, visited));
        }

        // `export * from './x'` forwards every name except `default`.
        if export_name != "default" {
            for export in symbols.exports.iter().filter(|e| e.name == "*") {
                let Some(source) = &export.re_export_from else {
                    continue;
                };
                let Ok(target) = self.resolve_module(source, &path) else {
                    continue;
                };
                if let Some(found) = self.resolve_export(&target, export_name, visited) {
                    return Some(found);
                }
            }
        }

        None
    }

    /// Follow a single export entry of the module at `path` to its declaration.
    /// Falls back to the export site when the chain can't be followed (unresolvable
    /// module, `export * as ns`, `export default <expression>`).
    fn follow_export(
        &self,
        path: &Path,
        symbols: &FileSymbols,
        export: &ExportedSymbol,
        visited: &mut HashSet<(PathBuf, String)>,
    ) -> ExportTarget {
        let site = ExportTarget::Site {
            path: path.to_path_buf(),
            range: export.specifier_range.unwrap_or(export.range),
            name: export.name.clone(),
        };

        if let Some(source) = &export.re_export_from {
            let Some(local_name) = &export.local_name else {
                return site; // `export * as ns from './x'`
            };
            let Ok(target) = self.resolve_module(source, path) else {
                return site;
            };
            return self
                .resolve_export(&target, local_name, visited)
                .unwrap_or(site);
        }

        let Some(symbol) = export
            .local_symbol_id
            .and_then(|id| symbols.find_symbol_by_id(id))
        else {
            return site;
        };

        // `import { a } from './x'; export { a };` — keep following.
        let import = symbols.imports.iter().find(|i| {
            i.local_name == symbol.name
                && i.range.start <= symbol.range.start
                && i.range.end >= symbol.range.end
        });
        if let Some(import) = import.filter(|i| !i.is_namespace) {
            if let Ok(target) = self.resolve_module(&import.module_specifier, path) {
                if let Some(found) =
                    self.resolve_export(&target, import_export_name(import), visited)
                {
                    return found;
                }
            }
        }

        ExportTarget::Symbol {
            path: path.to_path_buf(),
            symbol: symbol.clone(),
        }
    }

    /// Turn a resolved export target into a [`DefinitionResult`].
    fn target_definition(
        &self,
        target: ExportTarget,
        kind: DefinitionKind,
    ) -> Option<DefinitionResult> {
        let (path, location) = match target {
            ExportTarget::Symbol { path, symbol } => {
                let loc = SymbolLocation::new(path.clone(), symbol.range, symbol.kind, symbol.name);
                (path, loc)
            }
            ExportTarget::Site { path, range, name } => {
                let loc = SymbolLocation::new(
                    path.clone(),
                    range,
                    language_core::SymbolKind::Export,
                    name,
                );
                (path, loc)
            }
        };
        let (_, content) = self.cache.get(&path)?;
        Some(DefinitionResult::new(location, content.to_string(), kind))
    }

    /// Get the symbol cache (for testing/debugging).
    pub fn cache(&self) -> &SymbolCache {
        &self.cache
    }

    /// Clear all caches and indexed files.
    pub fn clear(&self) {
        self.cache.clear();
        self.indexed_files.write().clear();
        self.indexing_in_progress.write().clear();
        self.flows.clear();
        self.callers.clear();
        self.importers_index.clear();
        self.parse_errors.write().clear();
        self.flow_everywhere.store(false, Ordering::Relaxed);
        self.canonical_paths.write().clear();
        self.unreadable_files.write().clear();
        self.import_targets.write().clear();
        self.module_paths.write().clear();
        *self.workspace_packages.write() = None;
    }

    /// The type of the expression or declaration name at exactly this
    /// range, printed by the TypeScript checker. None without a checker,
    /// or when no expression has this range.
    pub fn get_type(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
    ) -> SemanticResult<Option<String>> {
        let Some(types) = self.types() else {
            return Ok(None);
        };
        Ok(types.type_at(&self.canonical_path(file_path), content, range))
    }

    /// Turn types on: the TypeScript 7 checker answers `typeInfo()` and
    /// resolves method calls the values do not. Takes effect if called
    /// before the first question that needs types.
    pub fn enable_types(&self) {
        self.types_enabled.store(true, Ordering::Relaxed);
    }

    pub(crate) fn types(&self) -> Option<&TsTypes> {
        self.types
            .get_or_init(|| {
                let enabled = self.types_enabled.load(Ordering::Relaxed);
                match TsTypes::discover(&self.workspace_root, enabled, Arc::clone(&self.type_texts)) {
                    Ok(types) => Some(types),
                    Err(ts_api::NoChecker::Off) => None,
                    Err(ts_api::NoChecker::NotFound) => {
                        log::warn!(
                            "types: \"ts7\" is on but no TypeScript 7 checker was found: install \
                             typescript@{}.x in the workspace or set CODEMOD_TS_API",
                            ts_api::PINNED_VERSION
                        );
                        None
                    }
                    Err(ts_api::NoChecker::WrongVersion { exe, version }) => {
                        log::warn!(
                            "types: {} is TypeScript {version}, this codemod speaks {}.x; types are off",
                            exe.display(),
                            ts_api::PINNED_VERSION
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    /// Where the property name of a member call starts, for the checker.
    fn member_name_start(content: &str, flow: &FileFlow, call: &CallFact) -> Option<u32> {
        let CalleeFact::Member { object, property } = &call.callee else {
            return None;
        };
        let object_end = flow.nodes[*object as usize].range.end;
        property_name_start(content, object_end, property)
    }

    /// The declarations the checker gives for a member call, asking for
    /// every member call of the file in one round trip the first time.
    fn typed_declarations(&self, path: &Path, flow: &FileFlow, call: &CallFact) -> Vec<TsDecl> {
        if call.is_jsx {
            return Vec::new();
        }
        let Some(types) = self.types() else {
            return Vec::new();
        };
        let Some(content) = self.cache.content(path) else {
            return Vec::new();
        };
        let Some(name_start) = Self::member_name_start(&content, flow, call) else {
            return Vec::new();
        };
        let offsets: Vec<u32> = flow
            .calls
            .iter()
            .filter(|c| !c.is_jsx)
            .filter_map(|c| Self::member_name_start(&content, flow, c))
            .collect();
        types.prefetch(path, &content, &offsets);
        types.declarations_at(path, &content, name_start)
    }

    /// The workspace functions a declaration the checker pointed at stands
    /// for (see [`flow_query::declaration_functions`]).
    fn functions_in_declaration(&self, decl: &TsDecl) -> Vec<(PathBuf, u32)> {
        let path = self.canonical_path(&decl.path);
        if FlowHost::flow(self, &path).is_none() {
            return Vec::new();
        }
        let Some(content) = self.cache.content(&path) else {
            return Vec::new();
        };
        let start = ts_api::skip_trivia(&content, ts_api::byte_of(&content, decl.pos16));
        let range = ByteRange::new(start, ts_api::byte_of(&content, decl.end16));
        flow_query::declaration_functions(self, &path, range)
    }
}

/// Where the property name of `object.property` starts: after the object,
/// `!`, `?.` or `.`, and trivia.
fn property_name_start(content: &str, object_end: u32, property: &str) -> Option<u32> {
    let bytes = content.as_bytes();
    let mut i = ts_api::skip_trivia(content, object_end) as usize;
    while bytes.get(i) == Some(&b'!') {
        i = ts_api::skip_trivia(content, i as u32 + 1) as usize;
    }
    if content.get(i..)?.starts_with("?.") {
        i += 2;
    } else if bytes.get(i) == Some(&b'.') {
        i += 1;
    } else {
        return None;
    }
    let i = ts_api::skip_trivia(content, i as u32) as usize;
    let rest = content.get(i..)?;
    (rest.starts_with(property)
        || rest
            .strip_prefix('#')
            .is_some_and(|r| r.starts_with(property)))
    .then_some(i as u32)
}

impl AccurateAnalyzer {
    /// Follow the value at `range` backward or forward within its module.
    pub fn data_flow(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: FlowOptions,
    ) -> SemanticResult<FlowResult> {
        self.process_file_with(file_path, content, true)?;
        flow_query::applicable(self, file_path, range, flow_query::QueryKind::Flow)?;
        Ok(flow_query::data_flow(self, file_path, range, options))
    }

    /// Walk the workspace call graph from the function or call at `range`.
    pub fn call_graph(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: CallGraphOptions,
    ) -> SemanticResult<CallGraphResult> {
        self.process_file_with(file_path, content, true)?;
        if options.direction == language_core::CallGraphDirection::Callers {
            flow_query::applicable(self, file_path, range, flow_query::QueryKind::Callers)?;
        }
        Ok(flow_query::call_graph(self, file_path, range, options))
    }

    /// Why the function at `range` is called from outside the workspace's
    /// code: its package exports it, or a selector rule declares it.
    pub fn entry_points(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
    ) -> SemanticResult<Vec<EntryPoint>> {
        self.process_file_with(file_path, content, true)?;
        flow_query::applicable(self, file_path, range, flow_query::QueryKind::Callers)?;
        Ok(flow_query::entry_points(self, file_path, range))
    }

    pub fn set_entry_point_rules(&self, rules: Vec<EntryPointRule>) {
        *self.entry_point_rules.write() = rules;
    }

    /// `path` relative to the workspace root, with `/` separators.
    fn workspace_relative(&self, path: &Path) -> String {
        let root = self.canonical_path(&self.workspace_root);
        path.strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// The name `export_name` of `module` lands on, as (file, range).
    fn export_lands_on(&self, module: &Path, export_name: &str) -> Option<(PathBuf, ByteRange)> {
        let found = self.resolve_export_definition(module, export_name)?;
        Some((
            self.canonical_path(&found.location.file_path),
            found.location.range,
        ))
    }

    /// The names `module` exports explicitly (`export *` forwards names it
    /// does not list).
    fn export_names(&self, module: &Path) -> Vec<String> {
        let _ = self.ensure_indexed(module);
        let Some((symbols, _)) = self.cache.get(module) else {
            return Vec::new();
        };
        let mut names: Vec<String> = symbols
            .exports
            .iter()
            .filter(|e| e.name != "*")
            .map(|e| {
                if e.is_default {
                    "default".to_string()
                } else {
                    e.name.clone()
                }
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// What the public entry modules of the package in `dir` export: the
    /// modules its `exports` (without patterns) or, without `exports`, its
    /// `main`/`module`/`source` name.
    fn package_exports(&self, package: &PackageInfo) -> Arc<PackageExports> {
        if let Some(found) = self.package_exports.read().get(&package.dir) {
            return Arc::clone(found);
        }
        let mut targets: Vec<&str> = Vec::new();
        match &package.exports {
            Some(exports) => string_leaves(exports, &mut targets),
            None => targets.extend(package.entries.iter().map(String::as_str)),
        }
        let manifest = package.dir.join("package.json");
        let mut modules: Vec<PathBuf> = targets
            .iter()
            .filter(|target| !target.contains('*') && !target.ends_with(".d.ts"))
            .filter_map(|target| {
                let relative = format!("./{}", target.trim_start_matches("./"));
                self.resolve_module_path(&manifest, &relative)
            })
            .collect();
        modules.sort();
        modules.dedup();
        let mut found = Vec::new();
        for module in modules {
            let specifiers = self.public_specifiers_of(&module);
            if specifiers.is_empty() {
                continue;
            }
            for name in self.export_names(&module) {
                if let Some((file, range)) = self.export_lands_on(&module, &name) {
                    for specifier in &specifiers {
                        found.push((file.clone(), range, format!("{specifier}#{name}")));
                    }
                }
            }
        }
        let found = Arc::new(found);
        self.package_exports
            .write()
            .insert(package.dir.clone(), Arc::clone(&found));
        found
    }

    /// The `export` entry points of a declaration named at `name_range` in
    /// `path`: the package's entry modules that export it, and its own file
    /// when that file is public itself (an `exports` pattern).
    fn export_entry_points(&self, path: &Path, name_range: ByteRange) -> Vec<EntryPoint> {
        let Some(package) = self.package_of(path) else {
            return Vec::new();
        };
        let mut labels: Vec<String> = self
            .package_exports(&package)
            .iter()
            .filter(|(file, range, _)| file == path && *range == name_range)
            .map(|(_, _, label)| label.clone())
            .collect();
        // Its own file, imported directly; also names an entry module
        // forwards with `export *`, which the table does not list.
        if let Some(own) = self.cache.export_name_at(path, name_range) {
            for specifier in self.public_specifiers_of(path) {
                labels.push(format!("{specifier}#{own}"));
            }
        }
        labels.sort();
        labels.dedup();
        labels
            .into_iter()
            .map(|label| EntryPoint {
                kind: "export".to_string(),
                label,
            })
            .collect()
    }

    /// Places that change the binding at `range` or the value it holds.
    pub fn mutations(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: MutationOptions,
    ) -> SemanticResult<MutationsResult> {
        self.process_file_with(file_path, content, true)?;
        flow_query::applicable(self, file_path, range, flow_query::QueryKind::Mutations)?;
        Ok(flow_query::mutations(self, file_path, range, options))
    }
}

impl FlowHost for AccurateAnalyzer {
    fn canonical(&self, path: &Path) -> PathBuf {
        self.canonical_path(path)
    }

    fn flow(&self, path: &Path) -> Option<Arc<FileFlow>> {
        let _ = self.ensure_indexed_with(path, true);
        let hash = self.cache.content_hash(path)?;
        self.flows
            .get_or_build(path, hash, || self.cache.content(path))
    }

    fn content(&self, path: &Path) -> Option<Arc<str>> {
        self.cache.content(path)
    }

    /// Imports are resolved by the same module and re-export resolution that
    /// answers go-to-definition.
    fn resolve_import(
        &self,
        from: &Path,
        specifier: &str,
        export_name: &str,
    ) -> Option<(PathBuf, ByteRange)> {
        // Resolution depends on the importing file's directory only.
        let key = (
            from.parent().unwrap_or(&self.workspace_root).to_path_buf(),
            specifier.to_string(),
            export_name.to_string(),
        );
        if let Some(found) = self.import_targets.read().get(&key) {
            return found.clone();
        }
        let found = self
            .resolve_module(specifier, from)
            .ok()
            .and_then(|module| self.resolve_export(&module, export_name, &mut HashSet::new()))
            .map(|target| match target {
                ExportTarget::Symbol { path, symbol } => (path, symbol.range),
                ExportTarget::Site { path, range, .. } => (path, range),
            });
        self.import_targets.write().insert(key, found.clone());
        found
    }

    fn module_path(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        let module = self.resolve_module(specifier, from).ok()?;
        Some(self.canonical(&module))
    }

    fn workspace_files(&self) -> Vec<PathBuf> {
        // Every file's facts are about to be needed: build them in the
        // indexing parse.
        self.flow_everywhere.store(true, Ordering::Relaxed);
        let _ = self.index_workspace_files();
        self.cache.files()
    }

    fn import_parse_error(&self, from: &Path, specifier: &str) -> Option<UnparsedFile> {
        let module = self.resolve_module(specifier, from).ok()?;
        let path = self.canonical(&module);
        let _ = self.ensure_indexed(&path);
        let message = self.parse_errors.read().get(&path)?.clone();
        Some(UnparsedFile {
            file_path: path,
            message,
        })
    }

    fn parse_failures(&self) -> Vec<UnparsedFile> {
        let mut failures: Vec<UnparsedFile> = self
            .parse_errors
            .read()
            .iter()
            .map(|(path, message)| UnparsedFile {
                file_path: path.clone(),
                message: message.clone(),
            })
            .collect();
        failures.sort_by(|a, b| a.file_path.cmp(&b.file_path));
        failures
    }

    /// Callers answers are a snapshot: the table of every call is built once.
    fn caller_index(&self) -> Option<&CallerIndexSlot> {
        Some(&self.callers)
    }

    fn entry_points_of(&self, path: &Path, flow: &FileFlow, function: u32) -> Vec<EntryPoint> {
        let Some(fact) = flow.functions.get(function as usize) else {
            return Vec::new();
        };
        let Some(name_range) = fact.name_range else {
            return Vec::new();
        };
        let mut found = self.export_entry_points(path, name_range);
        let rules = self.entry_point_rules.read();
        if !rules.is_empty() {
            let file = self.workspace_relative(path);
            let exports: Vec<String> = self
                .cache
                .export_name_at(path, name_range)
                .into_iter()
                .collect();
            found.extend(language_core::declared_entry_points(
                &rules,
                &FunctionEntry {
                    file: &file,
                    name: fact.name.as_deref(),
                    exports: &exports,
                    decorators: flow
                        .decorators
                        .get(&function)
                        .map(|d| d.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                        .as_deref()
                        .unwrap_or(&[]),
                },
            ));
        }
        found
    }

    fn relative_path(&self, path: &Path) -> String {
        self.workspace_relative(path)
    }

    fn typed_method_targets(
        &self,
        path: &Path,
        flow: &FileFlow,
        call: &CallFact,
    ) -> Vec<(PathBuf, u32)> {
        let decls = self.typed_declarations(path, flow, call);
        let Some(types) = self.types() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for decl in decls.iter().filter(|d| !d.library) {
            let found = self.functions_in_declaration(decl);
            if found.is_empty() {
                // A signature with no body (interface, abstract, overload):
                // the members that implement it, unless there are too many.
                for related in types.related_declarations(path, decl).decls {
                    out.extend(self.functions_in_declaration(&related));
                }
            } else {
                out.extend(found);
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn typed_declaration(
        &self,
        path: &Path,
        flow: &FileFlow,
        call: &CallFact,
    ) -> Option<FunctionRef> {
        let decl = self
            .typed_declarations(path, flow, call)
            .into_iter()
            .next()?;
        let content: Arc<str> = if decl.library {
            Arc::from(std::fs::read_to_string(&decl.path).ok()?)
        } else {
            self.cache.content(&self.canonical_path(&decl.path))?
        };
        let start = ts_api::skip_trivia(&content, ts_api::byte_of(&content, decl.pos16));
        let mut end = ts_api::byte_of(&content, decl.end16);
        // The checker's member nodes end with their `;` or `,`; the syntax
        // tree's do not.
        while end > start && matches!(content.as_bytes()[end as usize - 1], b';' | b',') {
            end -= 1;
        }
        let name = match &call.callee {
            CalleeFact::Member { property, .. } => Some(property.to_string()),
            CalleeFact::Node(_) => None,
        };
        Some(FunctionRef {
            file_path: if decl.library {
                decl.path
            } else {
                self.canonical_path(&decl.path)
            },
            range: ByteRange::new(start, end),
            name,
        })
    }
}

/// A file claimed for indexing. Dropped only while unwinding from a panic in
/// the parse (the normal paths `forget` it): the file is then given up as
/// unreadable and released, so threads waiting for it go on.
struct Claim<'a> {
    analyzer: &'a AccurateAnalyzer,
    path: &'a PathBuf,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.analyzer
            .unreadable_files
            .write()
            .insert(self.path.clone());
        self.analyzer.indexing_in_progress.write().remove(self.path);
    }
}

/// Where an exported name lands after following re-exports.
#[derive(Debug, Clone)]
enum ExportTarget {
    /// A declaration in `path`.
    Symbol { path: PathBuf, symbol: Symbol },
    /// The chain stops at an export site in `path` (e.g. the source module
    /// can't be resolved, or `export default <expression>`).
    Site {
        path: PathBuf,
        range: ByteRange,
        name: String,
    },
}

/// The property read right after an expression that ends at `end`:
/// `.name`, `?.name` or `["name"]`, with the property's range.
fn member_after(content: &str, end: u32) -> Option<(String, ByteRange)> {
    let bytes = content.as_bytes();
    let mut at = end as usize;
    let skip_space = |mut at: usize| {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        at
    };
    at = skip_space(at);
    // `?.name` reads like `.name`; `?.["name"]` like `["name"]`.
    let mut dotted = false;
    if bytes.get(at) == Some(&b'?') && bytes.get(at + 1) == Some(&b'.') {
        at = skip_space(at + 2);
        dotted = bytes.get(at) != Some(&b'[');
    } else if bytes.get(at) == Some(&b'.') {
        at = skip_space(at + 1);
        dotted = true;
    }
    match bytes.get(at)? {
        _ if dotted => {
            let start = at;
            let mut stop = start;
            while stop < bytes.len()
                && (bytes[stop].is_ascii_alphanumeric()
                    || bytes[stop] == b'_'
                    || bytes[stop] == b'$'
                    || bytes[stop] >= 0x80)
            {
                stop += 1;
            }
            let name = content.get(start..stop)?;
            (!name.is_empty())
                .then(|| (name.to_string(), ByteRange::new(start as u32, stop as u32)))
        }
        b'[' => {
            let open = skip_space(at + 1);
            let quote = *bytes.get(open)?;
            if quote != b'"' && quote != b'\'' && quote != b'`' {
                return None;
            }
            let start = open + 1;
            let close = start + content.get(start..)?.find(quote as char)?;
            let name = content.get(start..close)?;
            Some((name.to_string(), ByteRange::new(start as u32, close as u32)))
        }
        _ => None,
    }
}

/// The name an import binding asks its source module for.
fn import_export_name(import: &ImportedSymbol) -> &str {
    if import.is_default {
        "default"
    } else {
        import
            .imported_name
            .as_deref()
            .unwrap_or(&import.local_name)
    }
}

impl std::fmt::Debug for AccurateAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccurateAnalyzer")
            .field("workspace_root", &self.workspace_root)
            .field("indexed_files_count", &self.indexed_files.read().len())
            .finish()
    }
}

/// Shared `oxc_resolver` settings used by both the physical- and
/// VFS-backed resolver pairs. Kept in one place so the two variants
/// behave identically for everything except filesystem access.
fn default_resolve_options() -> ResolveOptions {
    ResolveOptions {
        extensions: vec![
            ".ts".to_string(),
            ".tsx".to_string(),
            ".js".to_string(),
            ".jsx".to_string(),
            ".mjs".to_string(),
            ".cjs".to_string(),
            ".mts".to_string(),
            ".cts".to_string(),
        ],
        // TypeScript ESM imports name the emitted file: `./a.js` is
        // `./a.ts` in the source tree.
        extension_alias: vec![
            (
                ".js".to_string(),
                vec![".ts".to_string(), ".tsx".to_string(), ".js".to_string()],
            ),
            (
                ".jsx".to_string(),
                vec![".tsx".to_string(), ".jsx".to_string()],
            ),
            (
                ".mjs".to_string(),
                vec![".mts".to_string(), ".mjs".to_string()],
            ),
            (
                ".cjs".to_string(),
                vec![".cts".to_string(), ".cjs".to_string()],
            ),
        ],
        main_fields: vec!["module".to_string(), "main".to_string()],
        condition_names: vec![
            "import".to_string(),
            "require".to_string(),
            "node".to_string(),
            "default".to_string(),
        ],
        ..Default::default()
    }
}

/// VFS-side analogue of `let p = root.join("tsconfig.json"); if p.exists()`.
/// Returns a `TsconfigDiscovery::Manual` pointing at the absolute path
/// that the VFS-backed resolver will then read through `VfsFileSystem`.
/// Falls back to `Auto` (resolver walks up from each file) when no root
/// tsconfig exists — `Auto` still queries via `VfsFileSystem`, so
/// per-package tsconfigs seeded into the VFS are still discovered.
fn vfs_tsconfig_discovery(workspace_root: &Path, fs_root: &VfsPath) -> Option<TsconfigDiscovery> {
    // Build the `/`-joined relative VFS key from `workspace_root`'s
    // components rather than string-slicing its stringified form. The
    // stringified approach broke on Windows (`C:\app\...` stripped only
    // a missing leading `/`, then `VfsPath::join` rejected the drive
    // prefix and we silently fell back to `Auto`, skipping any root
    // tsconfig the repo actually declared).
    let trimmed = workspace_root_to_vfs_key(workspace_root);
    let root_vp = if trimmed.is_empty() {
        fs_root.clone()
    } else {
        match fs_root.join(&trimmed) {
            Ok(p) => p,
            Err(_) => return Some(TsconfigDiscovery::Auto),
        }
    };

    let candidate = match root_vp.join("tsconfig.json") {
        Ok(p) => p,
        Err(_) => return Some(TsconfigDiscovery::Auto),
    };

    if candidate.exists().unwrap_or(false) {
        Some(TsconfigDiscovery::Manual(TsconfigOptions {
            config_file: workspace_root.join("tsconfig.json"),
            references: TsconfigReferences::Auto,
        }))
    } else {
        Some(TsconfigDiscovery::Auto)
    }
}

/// Collapse a filesystem `workspace_root` (possibly with a drive prefix
/// or backslash separators on Windows) into the `/`-joined relative
/// string VFS keys use. Shared with `VfsFileSystem::resolve_path` in
/// intent but lives here to avoid the crate's module-boundary cycle.
fn workspace_root_to_vfs_key(path: &Path) -> String {
    use std::path::Component;
    let mut out = String::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(&part.to_string_lossy());
            }
            Component::ParentDir => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str("..");
            }
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
        }
    }
    out
}

/// Recursively walk `start` (a VFS directory) and invoke `on_file` for
/// every leaf whose extension is an indexable JS/TS flavor. `prefix` is
/// the absolute path prefix we hand back to callers (typically the
/// workspace root), so cache keys match what `ensure_indexed` expects.
fn walk_vfs_tree(start: &VfsPath, prefix: &str, on_file: &mut dyn FnMut(&Path)) {
    // Treat the root specially: empty-string path needs to stay empty so
    // vfs::join doesn't end up with a leading slash we can't strip.
    fn recurse(entry: &VfsPath, prefix: &str, on_file: &mut dyn FnMut(&Path)) {
        let Ok(meta) = entry.metadata() else {
            return;
        };
        match meta.file_type {
            VfsFileType::Directory => {
                let Ok(children) = entry.read_dir() else {
                    return;
                };
                for child in children {
                    recurse(&child, prefix, on_file);
                }
            }
            VfsFileType::File => {
                let path_str = entry.as_str();
                let ext = Path::new(path_str)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs") {
                    // Map the VFS path ("/app/src/foo.ts" or "src/foo.ts")
                    // back to the absolute form that matches the analyzer's
                    // cache keys. If the VFS already gave us the absolute
                    // form (starts with `/`), use it verbatim; otherwise
                    // prepend `prefix`.
                    let absolute = if path_str.starts_with('/') {
                        path_str.to_string()
                    } else if prefix.ends_with('/') {
                        format!("{prefix}{path_str}")
                    } else {
                        format!("{prefix}/{path_str}")
                    };
                    on_file(Path::new(&absolute));
                }
            }
        }
    }
    recurse(start, prefix, on_file);
}

/// Every module reference in the workspace, by the file it resolves to (see
/// `AccurateAnalyzer::importers`). References of one target are in file order.
/// One file's module references: the file, its content, and (target, reference) pairs.
type FileRefs = (PathBuf, Arc<str>, Vec<(PathBuf, language_core::Importer)>);

#[derive(Default)]
struct ImportersIndex {
    by_target: HashMap<PathBuf, Vec<(PathBuf, language_core::Importer)>>,
    contents: HashMap<PathBuf, Arc<str>>,
    unparsed: Vec<UnparsedFile>,
}

/// Calls that name a module for a test framework's mocks.
const MOCK_CALLS: [&str; 10] = [
    "jest.mock",
    "jest.doMock",
    "jest.unmock",
    "jest.requireActual",
    "jest.requireMock",
    "vi.mock",
    "vi.doMock",
    "vi.unmock",
    "vi.importActual",
    "vi.importMock",
];

/// ESM `import … from "m"` and `import "m"` lines of an MDX or Markdown
/// file: each line's range and specifier. Only single-line imports at the
/// start of a line.
fn esm_import_lines(content: &str) -> Vec<(ByteRange, String)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed.starts_with("import ") {
            let quoted = trimmed
                .rfind(" from ")
                .map(|at| &trimmed[at + 6..])
                .unwrap_or_else(|| trimmed["import ".len()..].trim_start())
                .trim_end_matches(';')
                .trim();
            let quote = quoted.chars().next();
            if matches!(quote, Some('"') | Some('\''))
                && quoted.len() >= 2
                && quoted.ends_with(quote.unwrap())
            {
                out.push((
                    ByteRange::new(offset as u32, (offset + trimmed.len()) as u32),
                    quoted[1..quoted.len() - 1].to_string(),
                ));
            }
        }
        offset += line.len();
    }
    out
}

/// `import "m";` statements (no bindings), by a scan of the source: their
/// range (the statement) and specifier. Comments and strings are skipped by
/// requiring the statement to start a line.
fn side_effect_imports(content: &str) -> Vec<(ByteRange, String)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(rest) = trimmed.strip_prefix("import") {
            let rest_trimmed = rest.trim_start();
            if let Some(quote) = rest_trimmed
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
            {
                if let Some(end) = rest_trimmed[1..].find(quote) {
                    let specifier = &rest_trimmed[1..1 + end];
                    let start = offset + indent;
                    let stmt_len = trimmed.trim_end().len();
                    out.push((
                        ByteRange::new(start as u32, (start + stmt_len) as u32),
                        specifier.to_string(),
                    ));
                }
            }
        }
        offset += line.len();
    }
    out
}

/// `a/./b/../c` as `a/c`, without touching the filesystem (a `paths` or
/// `exports` pattern holds a `*` and does not exist on disk).
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Forms of a file a specifier can name: with its extension, without it,
/// its directory when it is an index file, and the `.js` output name of a
/// TypeScript file.
fn specifier_forms(target_str: &str) -> Vec<String> {
    let mut forms = vec![target_str.to_string()];
    if let Some(stem) = strip_js_extension(target_str) {
        forms.push(stem.to_string());
        for (source, output) in [
            (".tsx", ".js"),
            (".ts", ".js"),
            (".mts", ".mjs"),
            (".cts", ".cjs"),
        ] {
            if target_str.ends_with(source) && !target_str.ends_with(".d.ts") {
                forms.push(format!("{stem}{output}"));
                break;
            }
        }
        if let Some(dir) = stem.strip_suffix("/index") {
            forms.push(dir.to_string());
        }
    }
    forms
}

/// A workspace package: its directory, `name` and `exports`.
/// The declarations a package's entry modules export: (file, name range,
/// label such as `@acme/ui#Button`).
type PackageExports = Vec<(PathBuf, ByteRange, String)>;

struct PackageInfo {
    dir: PathBuf,
    name: Option<String>,
    exports: Option<serde_json::Value>,
    /// `main`, `module`, `types`, `typings` and `source`, as written.
    entries: Vec<String>,
}

/// The specifiers a package makes public that could name one of `forms` (the
/// target with and without extension): the bare name, and each `exports`
/// subpath, with `*` patterns matched against the target. Without `exports`
/// only the bare name (its `main`) is public. Candidates are checked by
/// resolving them afterwards.
fn public_specifiers(name: &str, package: &PackageInfo, forms: &[String]) -> Vec<String> {
    let mut out = vec![name.to_string()];
    let Some(exports @ serde_json::Value::Object(map)) = &package.exports else {
        return out;
    };
    // Conditions (`{ "import": ..., "types": ... }`) at the top: one "." entry.
    let entries: Vec<(&str, &serde_json::Value)> = if map.keys().any(|k| k.starts_with('.')) {
        map.iter().map(|(k, v)| (k.as_str(), v)).collect()
    } else {
        vec![(".", exports)]
    };
    for (key, value) in entries {
        let Some(subpath) = key.strip_prefix('.') else {
            continue;
        };
        let mut targets = Vec::new();
        string_leaves(value, &mut targets);
        for target in targets {
            let pattern = normalize_lexically(&package.dir.join(target));
            let pattern = pattern.to_string_lossy().replace('\\', "/");
            match (subpath.split_once('*'), pattern.split_once('*')) {
                (Some((key_head, key_tail)), Some((head, tail))) => {
                    for form in forms {
                        if let Some(middle) = form
                            .strip_prefix(head)
                            .and_then(|rest| rest.strip_suffix(tail))
                        {
                            out.push(format!("{name}{key_head}{middle}{key_tail}"));
                        }
                    }
                }
                (None, None) => out.push(format!("{name}{subpath}")),
                _ => {}
            }
        }
    }
    out
}

/// Every string in an `exports` value (conditions and fallbacks included).
fn string_leaves<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(s) => out.push(s),
        serde_json::Value::Array(items) => items.iter().for_each(|v| string_leaves(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| string_leaves(v, out)),
        _ => {}
    }
}

/// The path without a JavaScript or TypeScript extension, if it has one.
fn strip_js_extension(path: &str) -> Option<&str> {
    const EXTENSIONS: [&str; 10] = [
        ".d.ts", ".tsx", ".ts", ".mts", ".cts", ".jsx", ".js", ".mjs", ".cjs", ".json",
    ];
    EXTENSIONS.iter().find_map(|ext| path.strip_suffix(ext))
}

/// `./x` / `../x` from `from_dir` to `target`.
fn relative_specifier(from_dir: &Path, target: &Path) -> String {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = target.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_string(); from.len() - shared];
    parts.extend(
        to[shared..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().to_string()),
    );
    let joined = parts.join("/");
    if joined.starts_with("..") {
        joined
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        format!("./{joined}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create a simple test file
        let utils_content = r#"
export function add(a: number, b: number): number {
    return a + b;
}

export const PI = 3.14159;
"#;
        fs::write(dir.path().join("utils.ts"), utils_content).unwrap();

        let main_content = r#"
import { add, PI } from './utils';

const result = add(1, 2);
console.log(PI);
"#;
        fs::write(dir.path().join("main.ts"), main_content).unwrap();

        dir
    }

    #[test]
    fn test_accurate_process_file() {
        let workspace = create_test_workspace();
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());

        let content = fs::read_to_string(workspace.path().join("utils.ts")).unwrap();
        let result = analyzer.process_file(&workspace.path().join("utils.ts"), &content);

        assert!(result.is_ok());
        assert!(!analyzer.cache.is_empty());
    }

    #[test]
    fn test_accurate_process_file_early_out_on_same_content() {
        let workspace = create_test_workspace();
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());

        let path = workspace.path().join("utils.ts");

        let canonical = path.canonicalize().unwrap();
        let content = fs::read_to_string(&path).unwrap();

        analyzer.process_file(&path, &content).unwrap();
        let after_first = analyzer
            .cache
            .get(&canonical)
            .expect("cached after first call");

        analyzer.process_file(&path, &content).unwrap();
        let after_repeat = analyzer
            .cache
            .get(&canonical)
            .expect("still cached after repeat");
        assert_eq!(after_first.1, after_repeat.1, "content preserved on repeat");
        assert_eq!(
            after_first.0.symbols.len(),
            after_repeat.0.symbols.len(),
            "symbols preserved on repeat"
        );

        let modified = format!("{content}\nexport const added = 1;\n");
        analyzer.process_file(&path, &modified).unwrap();
        let after_mod = analyzer
            .cache
            .get(&canonical)
            .expect("still cached after modify");
        assert_eq!(
            &*after_mod.1, modified,
            "content replaced on modified input"
        );
        assert!(
            after_mod.0.symbols.len() > after_first.0.symbols.len(),
            "re-parsed symbols reflect the appended export (before={}, after={})",
            after_first.0.symbols.len(),
            after_mod.0.symbols.len(),
        );
    }

    #[test]
    fn test_accurate_resolve_module() {
        let workspace = create_test_workspace();
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());

        let main_path = workspace.path().join("main.ts");
        let result = analyzer.resolve_module("./utils", &main_path);

        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert!(resolved.to_string_lossy().contains("utils"));
    }

    #[test]
    fn test_accurate_get_definition_local() {
        let workspace = create_test_workspace();
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());

        let file_path = workspace.path().join("utils.ts");
        let content = fs::read_to_string(&file_path).unwrap();

        // Find definition of 'add' function
        // "add" appears at byte 17 in "export function add"
        let result = analyzer.get_definition(
            &file_path,
            &content,
            ByteRange::new(17, 20),
            DefinitionOptions::default(),
        );

        assert!(result.is_ok());
        let definition = result.unwrap();
        assert!(definition.is_some());

        let def = definition.unwrap();
        assert_eq!(def.location.name, "add");
        assert_eq!(def.location.kind, language_core::SymbolKind::Function);
        assert!(!def.content.is_empty());
        assert_eq!(def.kind, DefinitionKind::Local);
    }

    #[test]
    fn test_accurate_find_references_local() {
        let dir = TempDir::new().unwrap();
        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());

        let content = r#"
const x = 1;
const y = x + 2;
console.log(x);
        "#;
        let file_path = dir.path().join("test.ts");
        fs::write(&file_path, content).unwrap();

        // Find references to 'x' (definition at byte 7)
        let result = analyzer.find_references(&file_path, content, ByteRange::new(7, 8));

        assert!(result.is_ok());
        let references = result.unwrap();

        // Should find 2 references (not including the definition itself)
        assert!(
            references.total_count() >= 2,
            "Expected at least 2 references (usages only, not definition), got {}",
            references.total_count()
        );

        // Each file should have content
        for file in &references.files {
            assert!(!file.content.is_empty());
        }
    }

    #[test]
    fn test_accurate_get_definition_imported() {
        let workspace = create_test_workspace();
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());

        let main_path = workspace.path().join("main.ts");
        let content = fs::read_to_string(&main_path).unwrap();

        // Process main.ts first to populate cache
        analyzer.process_file(&main_path, &content).unwrap();

        // Find 'add' in the import statement
        // "add" appears in "import { add, PI } from './utils';"
        // Need to find the exact byte position
        let add_pos = content.find("add").unwrap();
        let result = analyzer.get_definition(
            &main_path,
            &content,
            ByteRange::new(add_pos as u32, (add_pos + 3) as u32),
            DefinitionOptions::default(),
        );

        assert!(result.is_ok());
        let definition = result.unwrap();

        if let Some(def) = definition {
            assert_eq!(def.location.name, "add");
            assert!(def.location.file_path.to_string_lossy().contains("utils"));
            assert!(!def.content.is_empty());
            assert_eq!(def.kind, DefinitionKind::External);
        }
    }

    #[test]
    fn test_accurate_find_references_cross_file() {
        let workspace = create_test_workspace();
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());

        let utils_path = workspace.path().join("utils.ts");
        let utils_content = fs::read_to_string(&utils_path).unwrap();

        // Process both files
        analyzer.process_file(&utils_path, &utils_content).unwrap();

        let main_path = workspace.path().join("main.ts");
        let main_content = fs::read_to_string(&main_path).unwrap();
        analyzer.process_file(&main_path, &main_content).unwrap();

        // Find references to 'add' function (defined in utils.ts)
        let add_pos = utils_content.find("add").unwrap();
        let result = analyzer.find_references(
            &utils_path,
            &utils_content,
            ByteRange::new(add_pos as u32, (add_pos + 3) as u32),
        );

        assert!(result.is_ok());
        let references = result.unwrap();

        // Should find at least the import in main.ts (definition is not included)
        assert!(
            references.total_count() >= 1,
            "Expected at least 1 reference (import), got {}",
            references.total_count()
        );

        // Each file should have content
        for file in &references.files {
            assert!(!file.content.is_empty());
        }
    }

    #[test]
    fn test_accurate_get_type() {
        let workspace = create_test_workspace();
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());

        let file_path = workspace.path().join("utils.ts");
        let content = fs::read_to_string(&file_path).unwrap();

        // Get type of 'PI' constant
        let pi_pos = content.find("PI").unwrap();
        let result = analyzer.get_type(
            &file_path,
            &content,
            ByteRange::new(pi_pos as u32, (pi_pos + 2) as u32),
        );

        // Types come from the TypeScript 7 checker when one is configured
        // (CODEMOD_TS_API); without it there is no answer.
        let result = result.unwrap();
        if analyzer.types().is_some() {
            assert!(result.is_some(), "the checker answers for PI");
        } else {
            assert!(result.is_none());
        }
    }

    #[test]
    fn test_tsconfig_paths_scoped_package() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Create tsconfig with scoped package path mapping
        let tsconfig = r#"{
            "compilerOptions": {
                "baseUrl": ".",
                "paths": {
                    "@acme/package-b/*": ["./packages/package-b/src/*"]
                }
            }
        }"#;
        fs::write(root.join("tsconfig.json"), tsconfig).unwrap();

        // Create the package files
        fs::create_dir_all(root.join("packages/package-b/src/components")).unwrap();
        fs::write(
            root.join("packages/package-b/src/components/index.ts"),
            "export { Button } from './Button';",
        )
        .unwrap();
        fs::write(
            root.join("packages/package-b/src/components/Button.ts"),
            "export function Button() { return 'button'; }",
        )
        .unwrap();

        // Create consumer
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/App.ts"),
            "import { Button } from '@acme/package-b/components';",
        )
        .unwrap();

        let analyzer = AccurateAnalyzer::new(root.to_path_buf());

        // Test resolution
        let app_path = root.join("src/App.ts");
        let result = analyzer.resolve_module("@acme/package-b/components", &app_path);
        assert!(
            result.is_ok(),
            "Should resolve @acme/package-b/components, got: {:?}",
            result
        );
    }

    #[test]
    fn test_tsconfig_paths_tilde_alias() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        let tsconfig = r#"{
            "compilerOptions": {
                "baseUrl": ".",
                "paths": {
                    "~/*": ["./src/*"]
                }
            }
        }"#;
        fs::write(root.join("tsconfig.json"), tsconfig).unwrap();

        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::write(
            root.join("src/components/index.ts"),
            "export { Button } from './Button';",
        )
        .unwrap();

        let analyzer = AccurateAnalyzer::new(root.to_path_buf());
        let app_path = root.join("src/App.ts");
        let result = analyzer.resolve_module("~/components", &app_path);
        assert!(
            result.is_ok(),
            "Should resolve ~/components, got: {:?}",
            result
        );
    }

    #[test]
    fn test_vfs_resolver_respects_tsconfig_paths() {
        use vfs::MemoryFS;

        // Stand up a MemoryFS snapshot of what pg_ast_grep would seed:
        // a single tsconfig at /app/tsconfig.json declaring a `~/*` alias
        // onto `./src/*`, plus the target file the alias should resolve to.
        // All paths are absolute under /app to mimic SANDBOX_ROOT usage.
        let fs_root: VfsPath = MemoryFS::new().into();
        fs_root.join("app/src").unwrap().create_dir_all().unwrap();

        let tsconfig = r#"{
            "compilerOptions": {
                "baseUrl": ".",
                "paths": {
                    "~/*": ["./src/*"]
                }
            }
        }"#;
        {
            use std::io::Write;
            let mut w = fs_root
                .join("app/tsconfig.json")
                .unwrap()
                .create_file()
                .unwrap();
            w.write_all(tsconfig.as_bytes()).unwrap();
        }
        {
            use std::io::Write;
            let mut w = fs_root
                .join("app/src/utils.ts")
                .unwrap()
                .create_file()
                .unwrap();
            w.write_all(b"export const x = 1;").unwrap();
        }

        let analyzer = AccurateAnalyzer::new_with_fs_and_walker(
            PathBuf::from("/app"),
            fs_root,
            WorkspaceWalker::Vfs,
        );

        // The analyzer should read tsconfig from the VFS, parse the `~`
        // alias, and resolve `~/utils` to `/app/src/utils.ts` — all
        // without touching the real disk.
        let from = PathBuf::from("/app/src/App.ts");
        let resolved = analyzer
            .resolve_module("~/utils", &from)
            .expect("~/utils should resolve via tsconfig.paths that was seeded into the VFS");
        assert!(
            resolved.to_string_lossy().contains("utils"),
            "unexpected resolution: {resolved:?}"
        );
    }

    #[test]
    fn test_vfs_walker_resolves_import_through_barrel() {
        use vfs::MemoryFS;

        // Mirrors pg_ast_grep's layout: `/app` is the workspace root
        // and all files are stored at their absolute sandboxed paths in
        // a MemoryFS. A consumer imports via a relative barrel
        // (`./utils`), the barrel re-exports from a sibling module
        // (`./math`), and the analyzer must successfully index the
        // barrel. Before the `read_file` path-mapping fix, the
        // analyzer's strip-prefix lookup couldn't find the barrel's
        // content in the VFS and `get_definition` returned `None` —
        // silently losing every cross-file query (the insights/CLI
        // parity bug).
        //
        // The definition follows the barrel's re-export through to the
        // declaration in `math.ts`, flagged as an external (cross-file) kind.
        let fs_root: VfsPath = MemoryFS::new().into();
        fs_root.join("app/utils").unwrap().create_dir_all().unwrap();

        let files = [
            (
                "app/utils/math.ts",
                "export function add(a: number, b: number): number { return a + b; }\n",
            ),
            ("app/utils/index.ts", "export { add } from './math';\n"),
            (
                "app/main.ts",
                "import { add } from './utils';\nconst x = add(1, 2);\n",
            ),
        ];
        for (path, content) in files {
            use std::io::Write;
            let mut w = fs_root.join(path).unwrap().create_file().unwrap();
            w.write_all(content.as_bytes()).unwrap();
        }

        let analyzer = AccurateAnalyzer::new_with_fs_and_walker(
            PathBuf::from("/app"),
            fs_root,
            WorkspaceWalker::Vfs,
        );

        let main_path = PathBuf::from("/app/main.ts");
        let main_content = "import { add } from './utils';\nconst x = add(1, 2);\n";
        let add_pos = main_content.find("add").unwrap();
        analyzer
            .process_file(&main_path, main_content)
            .expect("process_file on main.ts must succeed");

        let def = analyzer
            .get_definition(
                &main_path,
                main_content,
                ByteRange::new(add_pos as u32, (add_pos + 3) as u32),
                DefinitionOptions::default(),
            )
            .expect("get_definition must not error")
            .expect(
                "get_definition must find a cross-file definition for `add` — \
                 when this returns None the analyzer is unable to read the \
                 barrel from the VFS (the path-mapping regression)",
            );

        let file_str = def.location.file_path.to_string_lossy().into_owned();
        assert!(
            file_str.ends_with("utils/math.ts"),
            "expected definition followed through the barrel to utils/math.ts, got {file_str:?}"
        );
        assert_eq!(def.location.name, "add");
        assert_eq!(def.location.kind, language_core::SymbolKind::Function);
        assert_eq!(def.kind, DefinitionKind::External);
    }

    #[test]
    fn test_resolve_local_import_with_broken_tsconfig_extends() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // tsconfig extends a package that doesn't exist (simulates missing node_modules)
        fs::create_dir_all(root.join("packages/api/src/utils")).unwrap();
        fs::write(
            root.join("packages/api/tsconfig.json"),
            r#"{
                "extends": "@acme/tsconfig/internal-package.json",
                "compilerOptions": {
                    "rootDir": "."
                }
            }"#,
        )
        .unwrap();

        fs::write(
            root.join("packages/api/src/utils/index.ts"),
            "export { helper } from './helper';",
        )
        .unwrap();
        fs::write(
            root.join("packages/api/src/utils/helper.ts"),
            "export function helper() { return 1; }",
        )
        .unwrap();
        fs::write(
            root.join("packages/api/src/main.ts"),
            "import { helper } from './utils';",
        )
        .unwrap();

        let analyzer = AccurateAnalyzer::new(root.to_path_buf());
        let main_path = root.join("packages/api/src/main.ts");

        // Should resolve even without node_modules thanks to fallback resolver
        let result = analyzer.resolve_module("./utils", &main_path);
        assert!(
            result.is_ok(),
            "Should resolve ./utils via fallback when tsconfig extends is broken, got: {:?}",
            result
        );
    }

    /// Lays out the demo monorepo shape: a root tsconfig without `paths`,
    /// `packages/tsconfig.json` mapping `@demo/shared` to the barrel, and a
    /// barrel re-exporting from `./dates`.
    fn create_monorepo(with_node_modules_link: bool, with_nested_paths: bool) -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{ "compilerOptions": { "strict": true }, "include": ["workflows"] }"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("packages/shared/src")).unwrap();
        fs::create_dir_all(root.join("packages/web/src")).unwrap();
        if with_nested_paths {
            fs::write(
                root.join("packages/tsconfig.json"),
                r#"{
                    "compilerOptions": {
                        "baseUrl": ".",
                        "paths": { "@demo/shared": ["./shared/src/index.ts"] }
                    },
                    "include": ["*/src"]
                }"#,
            )
            .unwrap();
        }
        fs::write(
            root.join("packages/shared/package.json"),
            r#"{ "name": "@demo/shared", "main": "src/index.ts" }"#,
        )
        .unwrap();
        fs::write(
            root.join("packages/shared/src/dates.ts"),
            "export function toMoment(d: Date) { return d; }\n",
        )
        .unwrap();
        fs::write(
            root.join("packages/shared/src/index.ts"),
            "export { toMoment } from \"./dates\";\n",
        )
        .unwrap();
        fs::write(
            root.join("packages/web/src/app.ts"),
            "import { toMoment } from \"@demo/shared\";\ntoMoment(new Date());\n",
        )
        .unwrap();
        if with_node_modules_link {
            fs::create_dir_all(root.join("node_modules/@demo")).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(
                "../../packages/shared",
                root.join("node_modules/@demo/shared"),
            )
            .unwrap();
        }
        dir
    }

    fn assert_import_reaches_dates(workspace: &TempDir) {
        let analyzer = AccurateAnalyzer::new(workspace.path().to_path_buf());
        let app = workspace.path().join("packages/web/src/app.ts");
        let content = fs::read_to_string(&app).unwrap();
        let pos = content.find("toMoment").unwrap() as u32;
        let def = analyzer
            .get_definition(
                &app,
                &content,
                ByteRange::new(pos, pos + 8),
                DefinitionOptions::default(),
            )
            .unwrap()
            .expect("definition for imported toMoment");
        assert_eq!(def.kind, DefinitionKind::External, "{def:?}");
        assert!(
            def.location
                .file_path
                .ends_with("packages/shared/src/dates.ts"),
            "expected dates.ts, got {:?}",
            def.location.file_path
        );
        assert_eq!(def.location.kind, language_core::SymbolKind::Function);
    }

    #[test]
    #[cfg(unix)]
    fn test_workspace_package_name_follows_barrel() {
        let workspace = create_monorepo(true, false);
        assert_import_reaches_dates(&workspace);
    }

    #[test]
    fn test_nested_tsconfig_paths_follow_barrel() {
        let workspace = create_monorepo(false, true);
        assert_import_reaches_dates(&workspace);
    }

    #[test]
    fn test_reexport_cycle_terminates() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.ts"), "export * from './b';\n").unwrap();
        fs::write(dir.path().join("b.ts"), "export * from './a';\n").unwrap();
        let main = dir.path().join("main.ts");
        let content = "import { missing } from './a';\nmissing();\n";
        fs::write(&main, content).unwrap();

        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
        let pos = content.find("missing").unwrap() as u32;
        let def = analyzer
            .get_definition(
                &main,
                content,
                ByteRange::new(pos, pos + 7),
                DefinitionOptions::default(),
            )
            .unwrap()
            .expect("falls back to the import statement");
        assert_eq!(def.kind, DefinitionKind::Import);
    }

    #[test]
    fn test_reexport_ignores_same_named_local_in_barrel() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("impl.ts"), "export function thing() {}\n").unwrap();
        fs::write(
            dir.path().join("index.ts"),
            "function helper() { const thing = 1; return thing; }\nexport { thing } from './impl';\n",
        )
        .unwrap();
        let main = dir.path().join("main.ts");
        let content = "import { thing } from './index';\nthing();\n";
        fs::write(&main, content).unwrap();

        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
        let pos = content.find("thing").unwrap() as u32;
        let def = analyzer
            .get_definition(
                &main,
                content,
                ByteRange::new(pos, pos + 5),
                DefinitionOptions::default(),
            )
            .unwrap()
            .unwrap();
        assert!(
            def.location.file_path.ends_with("impl.ts"),
            "{:?}",
            def.location.file_path
        );
    }

    /// TypeScript ESM style: `./impl.js` names the emitted file of `impl.ts`.
    #[test]
    fn js_extension_import_finds_the_ts_source() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("impl.ts"), "export function thing() {}\n").unwrap();
        fs::write(
            dir.path().join("view.tsx"),
            "export const View = () => null;\n",
        )
        .unwrap();
        let main = dir.path().join("main.ts");
        let content = "import { thing } from './impl.js';\nimport { View } from './view.jsx';\nfunction run() { thing(); View(); }\n";
        fs::write(&main, content).unwrap();

        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
        for (needle, file) in [("thing", "impl.ts"), ("View", "view.tsx")] {
            let def = analyzer
                .get_definition(
                    &main,
                    content,
                    range_of(content, needle),
                    DefinitionOptions::default(),
                )
                .unwrap()
                .unwrap();
            assert!(
                def.location.file_path.ends_with(file),
                "{needle}: {:?}",
                def.location.file_path
            );
        }
        let callees = analyzer
            .call_graph(
                &main,
                content,
                range_of(content, "run"),
                CallGraphOptions::default(),
            )
            .unwrap();
        assert_eq!(entry_names(&callees), ["thing", "View"]);
    }

    /// A dense call graph (each function calls the next several times, the
    /// last calls the first) multiplies call contexts. An interprocedural
    /// walk over it must stop quickly, bounded and marked truncated.
    #[test]
    fn interprocedural_fan_out_terminates() {
        let dir = TempDir::new().unwrap();
        let count = 30;
        let mut lib = String::new();
        for i in 0..count {
            let next = (i + 1) % count;
            let calls: Vec<String> = (0..6).map(|_| format!("f{next}(x)")).collect();
            lib.push_str(&format!(
                "export function f{i}(x) {{ return {} + sink(x); }}\n",
                calls.join(" + ")
            ));
        }
        fs::write(dir.path().join("lib.js"), &lib).unwrap();
        let main = dir.path().join("main.js");
        let mut content = String::from("import { f0, f5, f9 } from './lib.js';\n");
        for i in 0..20 {
            content.push_str(&format!(
                "const a{i} = f0(input{i}) + f5(a{i}) + f9(a{i});\n"
            ));
        }
        fs::write(&main, &content).unwrap();
        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
        let range = range_of(&content, "input0");
        for direction in [
            language_core::FlowDirection::Forward,
            language_core::FlowDirection::Backward,
        ] {
            let started = std::time::Instant::now();
            let result = analyzer
                .data_flow(
                    &main,
                    &content,
                    range,
                    FlowOptions {
                        direction,
                        interprocedural: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            let steps: usize = result.files.iter().map(|f| f.steps.len()).sum();
            assert!(
                started.elapsed() < std::time::Duration::from_secs(20),
                "{direction:?} took {:?}",
                started.elapsed()
            );
            assert!(steps <= 50_000, "{direction:?}: {steps} steps");
        }
        // Backward from inside the cycle fans out to every caller.
        let lib_path = dir.path().join("lib.js");
        let started = std::time::Instant::now();
        let result = analyzer
            .data_flow(
                &lib_path,
                &lib,
                range_of(&lib, "sink(x)"),
                FlowOptions {
                    interprocedural: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert!(result.truncated, "the caps should have cut this answer");
    }

    /// The stale-flags demo deleted a live email template: its only uses
    /// were through default imports, and references() did not see them.
    #[test]
    fn references_follow_default_imports() {
        let dir = TempDir::new().unwrap();
        let cases = [
            (
                "decl.tsx",
                "export default function WelcomeEmail() { return null; }\n",
                "WelcomeEmail",
            ),
            (
                "named.ts",
                "function resetEmail() { return 1; }\nexport default resetEmail;\n",
                "resetEmail",
            ),
            (
                "aliased.ts",
                "const invoiceEmail = () => 2;\nexport { invoiceEmail as default };\n",
                "invoiceEmail",
            ),
        ];
        for (file, source, _) in &cases {
            fs::write(dir.path().join(file), source).unwrap();
        }
        fs::write(
            dir.path().join("barrel.ts"),
            "export { default as Invoice } from './aliased';\nexport { default } from './named';\n",
        )
        .unwrap();
        let send = dir.path().join("send.tsx");
        fs::write(
            &send,
            "import Welcome from './decl';\nimport reset from './named';\nimport { Invoice } from './barrel';\nimport barrelDefault from './barrel';\nexport const a = <Welcome />;\nexport const b = reset();\nexport const c = Invoice();\nexport const d = barrelDefault();\n",
        )
        .unwrap();
        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
        let expected: [&[&str]; 3] = [
            &["Welcome", "Welcome"],
            &[
                "reset",
                "reset",
                "barrelDefault",
                "barrelDefault",
                "default",
            ],
            &["Invoice", "Invoice", "default"],
        ];
        for ((file, source, name), expected) in cases.iter().zip(expected) {
            let path = dir.path().join(file);
            let result = analyzer
                .find_references(&path, source, range_of(source, name))
                .unwrap();
            let mut found: Vec<String> = result
                .files
                .iter()
                .filter(|f| !f.file_path.ends_with(file))
                .flat_map(|f| {
                    f.locations.iter().map(|l| {
                        f.content[l.range.start as usize..l.range.end as usize].to_string()
                    })
                })
                .collect();
            found.sort();
            let mut expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
            expected.sort();
            assert_eq!(found, expected, "references of {name}");
        }
    }

    fn entry_names(result: &CallGraphResult) -> Vec<String> {
        result
            .files
            .iter()
            .flat_map(|file| file.entries.iter().filter_map(|e| e.name.clone()))
            .collect()
    }

    fn range_of(source: &str, needle: &str) -> ByteRange {
        let start = source.find(needle).unwrap() as u32;
        ByteRange::new(start, start + needle.len() as u32)
    }

    /// forEachFile workers share one analyzer and may all ask callers() at
    /// once: one table is built, everyone gets the answer a lone query gets.
    #[test]
    fn concurrent_callers_queries_share_one_table() {
        let dir = TempDir::new().unwrap();
        let mut sources = Vec::new();
        for index in 0..8 {
            let lib = dir.path().join(format!("lib{index}.js"));
            let lib_src = format!("export function target{index}() {{}}\n");
            fs::write(&lib, &lib_src).unwrap();
            for user in 0..5 {
                fs::write(
                    dir.path().join(format!("use{index}_{user}.js")),
                    format!(
                        "import {{ target{index} }} from './lib{index}.js';\nexport function caller{index}_{user}() {{ target{index}(); }}\n"
                    ),
                )
                .unwrap();
            }
            sources.push((lib, lib_src, format!("target{index}")));
        }
        let options = CallGraphOptions {
            direction: language_core::CallGraphDirection::Callers,
            ..Default::default()
        };
        let ask = |analyzer: &AccurateAnalyzer, (lib, src, name): &(PathBuf, String, String)| {
            let mut names = entry_names(
                &analyzer
                    .call_graph(lib, src, range_of(src, name), options)
                    .unwrap(),
            );
            names.sort();
            names
        };
        let alone: Vec<Vec<String>> = {
            let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
            sources.iter().map(|site| ask(&analyzer, site)).collect()
        };
        assert_eq!(
            alone[3],
            (0..5).map(|u| format!("caller3_{u}")).collect::<Vec<_>>()
        );

        for _ in 0..3 {
            let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
            let together: Vec<Vec<String>> = std::thread::scope(|scope| {
                let handles: Vec<_> = sources
                    .iter()
                    .map(|site| scope.spawn(|| ask(&analyzer, site)))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert_eq!(together, alone);
        }
    }

    #[test]
    fn callees_follow_an_export_that_moves_during_the_run() {
        let dir = TempDir::new().unwrap();
        let lib = dir.path().join("lib.js");
        fs::write(&lib, "export function target() {}\n").unwrap();
        let a = dir.path().join("a.js");
        let a_src = "import { target } from './lib.js';\nexport function first() { target(); }\n";
        fs::write(&a, a_src).unwrap();
        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());
        let callees = |analyzer: &AccurateAnalyzer| {
            let result = analyzer
                .call_graph(
                    &a,
                    a_src,
                    range_of(a_src, "first"),
                    CallGraphOptions::default(),
                )
                .unwrap();
            let resolved: Vec<bool> = result
                .files
                .iter()
                .flat_map(|f| f.entries.iter().map(|e| e.resolved))
                .collect();
            (entry_names(&result), resolved)
        };
        assert_eq!(callees(&analyzer), (vec!["target".to_string()], vec![true]));

        // lib.js stops exporting target; another function now sits where
        // target's name was, so a remembered answer would resolve to it.
        let edited = "export function zzzzzz() {}\n";
        fs::write(&lib, edited).unwrap();
        analyzer.process_file(&lib, edited).unwrap();
        assert_eq!(
            callees(&analyzer),
            (vec!["target".to_string()], vec![false])
        );
    }

    #[test]
    fn files_that_do_not_parse_are_reported() {
        let dir = TempDir::new().unwrap();
        let lib = dir.path().join("lib.js");
        let broken = "export function target( {\n";
        fs::write(&lib, broken).unwrap();
        let a = dir.path().join("a.js");
        let a_src = "import { target } from './lib.js';\nexport function run() { target(); }\n";
        fs::write(&a, a_src).unwrap();
        let analyzer = AccurateAnalyzer::new(dir.path().to_path_buf());

        // Asking about the file itself is an error, not an empty answer.
        let flow = analyzer.data_flow(
            &lib,
            broken,
            range_of(broken, "target"),
            FlowOptions::default(),
        );
        assert!(flow.is_err());

        // Its callees: the call into it is unresolved, and the file is listed.
        let callees = analyzer
            .call_graph(
                &a,
                a_src,
                range_of(a_src, "run"),
                CallGraphOptions::default(),
            )
            .unwrap();
        assert_eq!(entry_names(&callees), ["target"]);
        assert!(callees.files[0].entries.iter().all(|e| !e.resolved));
        assert_eq!(callees.unparsed.len(), 1);
        assert!(callees.unparsed[0].file_path.ends_with("lib.js"));
        assert!(!callees.unparsed[0].message.is_empty());

        // Callers walk the workspace, so they list it too.
        let callers = analyzer
            .call_graph(
                &a,
                a_src,
                range_of(a_src, "run"),
                CallGraphOptions {
                    direction: language_core::CallGraphDirection::Callers,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(callers
            .unparsed
            .iter()
            .any(|u| u.file_path.ends_with("lib.js")));
    }
}
