//! Accurate mode implementation for workspace-wide lazy indexing.

use crate::cache::SymbolCache;
use crate::error::JsSemanticError;
use crate::oxc_adapter::{find_symbol_at_range, parse_and_analyze};
use crate::vfs_fs::VfsFileSystem;
use language_core::{
    filesystem, ByteRange, DefinitionKind, DefinitionOptions, DefinitionResult, FileReferences,
    ReferencesResult, SemanticResult, SymbolLocation,
};
use oxc_resolver::{
    Resolution, ResolveError, ResolveOptions, Resolver, ResolverGeneric, TsconfigDiscovery,
    TsconfigOptions, TsconfigReferences,
};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
enum DualResolver {
    Physical {
        primary: Resolver,
        fallback: Resolver,
    },
    Vfs {
        primary: ResolverGeneric<VfsFileSystem>,
        fallback: ResolverGeneric<VfsFileSystem>,
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
}

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

        DualResolver::Physical { primary, fallback }
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

        DualResolver::Vfs { primary, fallback }
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
        let path_str: std::borrow::Cow<str> = match self.walker {
            WorkspaceWalker::Ignore => {
                let relative_path = file_path
                    .strip_prefix(&self.workspace_root)
                    .unwrap_or(file_path);
                relative_path.to_string_lossy().into_owned().into()
            }
            WorkspaceWalker::Vfs => file_path.to_string_lossy(),
        };

        let vfs_path = self
            .fs_root
            .join(&*path_str)
            .map_err(|e| JsSemanticError::Internal(format!("Failed to join path: {}", e)))?;

        filesystem::read_to_string(&vfs_path)
            .map_err(|e| JsSemanticError::Internal(format!("Failed to read file: {}", e)))
    }

    /// Index a file and its dependencies lazily.
    fn ensure_indexed(&self, file_path: &Path) -> SemanticResult<()> {
        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());

        // Check if already indexed
        if self.indexed_files.read().contains(&canonical) {
            return Ok(());
        }

        // Check if currently being indexed (cycle detection)
        if self.indexing_in_progress.read().contains(&canonical) {
            return Ok(());
        }

        // Mark as in progress
        self.indexing_in_progress.write().insert(canonical.clone());

        // Read and parse the file using the virtual filesystem
        let content = self.read_file(&canonical)?;

        let file_symbols = parse_and_analyze(&canonical, &content)?;

        // Index imported files
        for import in &file_symbols.imports {
            if let Ok(resolved) = self.resolve_module(&import.module_specifier, &canonical) {
                // Recursively index the imported file
                let _ = self.ensure_indexed(&resolved);
            }
        }

        // Store in cache
        self.cache.insert(canonical.clone(), file_symbols, content);

        // Mark as indexed
        self.indexing_in_progress.write().remove(&canonical);
        self.indexed_files.write().insert(canonical);

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
    /// fallback because their resolution intentionally depends on tsconfig `paths`
    /// or `node_modules`.
    pub fn resolve_module(
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
            Err(_) => Err(JsSemanticError::ModuleResolution {
                specifier: specifier.to_string(),
                from_path: from_path.to_path_buf(),
            }),
        }
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
    /// We match content by `(len, bytes)` rather than hashing because
    /// the cache already holds the string — a byte-slice comparison
    /// is O(n) but skips re-parsing which is much more expensive.
    /// Mismatched content still triggers a re-parse, preserving the
    /// "this notification is authoritative" contract for callers that
    /// legitimately hand new content in (e.g. editor-driven LSP use).
    pub fn process_file(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());

        if self.indexed_files.read().contains(&canonical) {
            if let Some((_, cached_content)) = self.cache.get(&canonical) {
                if cached_content.len() == content.len() && cached_content == content {
                    return Ok(());
                }
            }
        }

        let file_symbols = parse_and_analyze(file_path, content)?;
        self.cache
            .insert(canonical.clone(), file_symbols, content.to_string());
        self.indexed_files.write().insert(canonical);
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

        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());
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

            // Try to resolve the import
            match self.resolve_import_definition(
                &import.module_specifier,
                &canonical,
                &import.local_name,
                import.imported_name.as_deref(),
                import.is_default,
            )? {
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

        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());
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

                // Search all indexed files for imports of this module
                for cached_path in self.cache.files() {
                    if cached_path == canonical {
                        continue;
                    }

                    if let Some((other_symbols, _)) = self.cache.get(&cached_path) {
                        // Check imports from this file
                        for import in &other_symbols.imports {
                            // Try to resolve the import
                            if let Ok(resolved) =
                                self.resolve_module(&import.module_specifier, &cached_path)
                            {
                                if resolved == canonical {
                                    // This file imports from our file
                                    let matches_name = import
                                        .imported_name
                                        .as_ref()
                                        .map(|n| n == &sym.name)
                                        .unwrap_or(false)
                                        || import.local_name == sym.name;

                                    if matches_name {
                                        files_map.entry(cached_path.clone()).or_default().push(
                                            SymbolLocation::new(
                                                cached_path.clone(),
                                                import.range,
                                                language_core::SymbolKind::Import,
                                                import.local_name.clone(),
                                            ),
                                        );

                                        // Also find references to this import in the file
                                        let import_symbol = other_symbols
                                            .symbols
                                            .iter()
                                            .find(|s| s.name == import.local_name);

                                        if let Some(imp_sym) = import_symbol {
                                            for ref_in_other in
                                                other_symbols.find_references_to(imp_sym.symbol_id)
                                            {
                                                files_map
                                                    .entry(cached_path.clone())
                                                    .or_default()
                                                    .push(SymbolLocation::new(
                                                        cached_path.clone(),
                                                        ref_in_other.range,
                                                        sym.kind,
                                                        import.local_name.clone(),
                                                    ));
                                            }
                                        }
                                    }
                                }
                            }
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
                self.cache.get(&path).map(|(_, c)| c).unwrap_or_default()
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
        let walker = ignore::WalkBuilder::new(&self.workspace_root)
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if path.is_file() {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs") {
                    let _ = self.ensure_indexed(path);
                }
            }
        }

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
        walk_vfs_tree(&start, &workspace_prefix, &mut |path: &Path| {
            let _ = self.ensure_indexed(path);
        });
        Ok(())
    }

    /// Resolve an import to its definition.
    /// Returns `DefinitionKind::External` for successfully resolved cross-file definitions.
    fn resolve_import_definition(
        &self,
        module_specifier: &str,
        from_path: &Path,
        local_name: &str,
        imported_name: Option<&str>,
        is_default: bool,
    ) -> SemanticResult<Option<DefinitionResult>> {
        // Try to resolve the module
        let resolved_path = match self.resolve_module(module_specifier, from_path) {
            Ok(path) => path,
            Err(_) => return Ok(None), // External module, can't resolve
        };

        // Ensure the target file is indexed
        let _ = self.ensure_indexed(&resolved_path);

        let (file_symbols, file_content) = match self.cache.get(&resolved_path) {
            Some(entry) => entry,
            None => return Ok(None),
        };

        // Find the exported symbol
        let export_name = if is_default {
            "default"
        } else {
            imported_name.unwrap_or(local_name)
        };

        if let Some(export) = file_symbols.find_export_by_name(export_name) {
            if let Some(local_id) = export.local_symbol_id {
                if let Some(symbol) = file_symbols.find_symbol_by_id(local_id) {
                    return Ok(Some(DefinitionResult::new(
                        SymbolLocation::new(
                            resolved_path,
                            symbol.range,
                            symbol.kind,
                            symbol.name.clone(),
                        ),
                        file_content,
                        DefinitionKind::External,
                    )));
                }
            }
            return Ok(Some(DefinitionResult::new(
                SymbolLocation::new(
                    resolved_path,
                    export.range,
                    language_core::SymbolKind::Export,
                    export.name.clone(),
                ),
                file_content,
                DefinitionKind::External,
            )));
        }

        // Check for default export
        if is_default {
            if let Some(export) = file_symbols.get_default_export() {
                return Ok(Some(DefinitionResult::new(
                    SymbolLocation::new(
                        resolved_path,
                        export.range,
                        language_core::SymbolKind::Export,
                        "default".to_string(),
                    ),
                    file_content,
                    DefinitionKind::External,
                )));
            }
        }

        Ok(None)
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
    }

    /// Gets the type of a symbol at the given byte range.
    /// Type inference is not yet fully implemented, returns None.
    #[allow(dead_code)]
    pub fn get_type(
        &self,
        _file_path: &Path,
        _content: &str,
        _range: ByteRange,
    ) -> SemanticResult<Option<String>> {
        // TODO: Implement type inference using oxc's type system
        Ok(None)
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

        // Type inference not yet implemented, should return None
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
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
        // The analyzer's own contract is to land the definition on the
        // barrel's re-export site; downstream codemod logic (e.g.
        // debarrel's `resolveSpecifier`) walks the re-export chain
        // from there. So "success" here means: a definition is found,
        // it lives in the barrel, and it's flagged as an external
        // (cross-file) kind.
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
            file_str.contains("utils") && file_str.ends_with("index.ts"),
            "expected definition in the barrel (utils/index.ts), got {file_str:?}"
        );
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
}
