//! Accurate mode implementation for workspace-wide lazy indexing.

use crate::cache::SymbolCache;
use crate::error::JsSemanticError;
use crate::oxc_adapter::{find_symbol_at_range, parse_and_analyze};
use language_core::{
    filesystem, ByteRange, DefinitionKind, DefinitionOptions, DefinitionResult, FileReferences,
    ReferencesResult, SemanticResult, SymbolLocation,
};
use oxc_resolver::{ResolveOptions, Resolver};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use vfs::VfsPath;

/// Accurate semantic analyzer with workspace-wide lazy indexing.
pub struct AccurateAnalyzer {
    /// Symbol cache for indexed files
    cache: SymbolCache,
    /// Workspace root directory
    workspace_root: PathBuf,
    /// Module resolver
    resolver: Resolver,
    /// Files that have been fully indexed
    indexed_files: RwLock<HashSet<PathBuf>>,
    /// Files currently being indexed (to prevent cycles)
    indexing_in_progress: RwLock<HashSet<PathBuf>>,
    /// Virtual filesystem root for file operations
    fs_root: VfsPath,
}

impl AccurateAnalyzer {
    /// Create a new accurate analyzer for a workspace.
    ///
    /// Uses the real filesystem (PhysicalFS) with the workspace root.
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
        let resolve_options = ResolveOptions {
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
        };

        Self {
            cache: SymbolCache::new(),
            workspace_root,
            resolver: Resolver::new(resolve_options),
            indexed_files: RwLock::new(HashSet::new()),
            indexing_in_progress: RwLock::new(HashSet::new()),
            fs_root,
        }
    }

    /// Read file content using the virtual filesystem.
    fn read_file(&self, file_path: &Path) -> Result<String, JsSemanticError> {
        // For PhysicalFS, we need to convert absolute paths to paths relative to the workspace root.
        // For MemoryFS, paths are virtual and should be used as-is.
        let relative_path = file_path
            .strip_prefix(&self.workspace_root)
            .unwrap_or(file_path);

        let path_str = relative_path.to_string_lossy();
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
    pub fn resolve_module(
        &self,
        specifier: &str,
        from_path: &Path,
    ) -> Result<PathBuf, JsSemanticError> {
        let from_dir = from_path.parent().unwrap_or(&self.workspace_root);

        match self.resolver.resolve(from_dir, specifier) {
            Ok(resolution) => Ok(resolution.into_path_buf()),
            Err(_) => Err(JsSemanticError::ModuleResolution {
                specifier: specifier.to_string(),
                from_path: from_path.to_path_buf(),
            }),
        }
    }

    /// Process a file notification (updates the cache).
    pub fn process_file(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        let file_symbols = parse_and_analyze(file_path, content)?;
        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());
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

    /// Index all JavaScript/TypeScript files in the workspace.
    fn index_workspace_files(&self) -> SemanticResult<()> {
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
}
