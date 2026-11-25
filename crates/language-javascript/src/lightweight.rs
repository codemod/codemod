//! Lightweight mode implementation for incremental per-file analysis.

use crate::cache::SymbolCache;
use crate::dataflow::{FileFlow, FlowCache};
use crate::error::JsSemanticError;
use crate::flow_query::{self, CallerIndexSlot, FlowHost};
use crate::oxc_adapter::{analyze, find_symbol_at_range, parse_and_analyze};
use language_core::{
    ByteRange, CallGraphOptions, CallGraphResult, DefinitionKind, DefinitionOptions,
    DefinitionResult, FileReferences, FlowOptions, FlowResult, MutationOptions, MutationsResult,
    ReferencesResult, SemanticResult, SymbolLocation, UnparsedFile,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Lightweight semantic analyzer that builds symbol cache incrementally.
#[derive(Debug, Default)]
pub struct LightweightAnalyzer {
    /// Symbol cache for all processed files
    cache: SymbolCache,
    /// Value-flow facts per file, built on the first data-flow or call-graph query.
    flows: FlowCache,
}

impl LightweightAnalyzer {
    /// Create a new lightweight analyzer.
    pub fn new() -> Self {
        Self {
            cache: SymbolCache::new(),
            flows: FlowCache::default(),
        }
    }

    /// Process a file and add its symbols to the cache.
    pub fn process_file(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        let file_symbols = parse_and_analyze(file_path, content)?;
        self.cache
            .insert(file_path.to_path_buf(), file_symbols, content.to_string());
        Ok(())
    }

    /// [`Self::process_file`] for a data-flow or call-graph query: the
    /// value-flow facts come from the same parse, unless they are cached.
    fn process_file_with_flow(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        let with_flow = !self
            .flows
            .has(file_path, crate::cache::hash_content(content));
        let (file_symbols, flow) = analyze(file_path, content, with_flow)?;
        if let Some(flow) = flow {
            self.flows
                .insert(file_path.to_path_buf(), file_symbols.content_hash, flow);
        }
        self.cache
            .insert(file_path.to_path_buf(), file_symbols, content.to_string());
        Ok(())
    }

    /// Get the definition of a symbol at the given range.
    ///
    /// In lightweight mode, this will:
    /// 1. Look for the symbol in the current file
    /// 2. If it's an import, try to resolve from cached files (if `resolve_external` is true)
    /// 3. Return the import statement if the module couldn't be resolved
    /// 4. Return None if the definition is not in the cache
    pub fn get_definition(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: DefinitionOptions,
    ) -> SemanticResult<Option<DefinitionResult>> {
        // Ensure file is in cache
        if !self.cache.contains(file_path) {
            self.process_file(file_path, content)?;
        }

        let (file_symbols, _) =
            self.cache
                .get(file_path)
                .ok_or_else(|| JsSemanticError::FileNotCached {
                    path: file_path.to_path_buf(),
                })?;

        // First, check if this is a reference to a symbol
        let reference = file_symbols
            .references
            .iter()
            .find(|r| r.range.start <= range.start && r.range.end >= range.end);

        if let Some(ref_info) = reference {
            // Find the symbol definition
            if let Some(symbol) = file_symbols.find_symbol_by_id(ref_info.symbol_id) {
                return Ok(Some(DefinitionResult::new(
                    SymbolLocation::new(
                        file_path.to_path_buf(),
                        symbol.range,
                        symbol.kind,
                        symbol.name.clone(),
                    ),
                    content.to_string(),
                    DefinitionKind::Local,
                )));
            }
        }

        // Check if this is an import reference
        if let Some(import) = file_symbols.find_import_at(range) {
            // If resolve_external is false, return the import statement directly
            if !options.resolve_external {
                return Ok(Some(DefinitionResult::new(
                    SymbolLocation::new(
                        file_path.to_path_buf(),
                        import.range,
                        language_core::SymbolKind::Import,
                        import.local_name.clone(),
                    ),
                    content.to_string(),
                    DefinitionKind::Import,
                )));
            }

            // Try to resolve the import from cached files
            match self.resolve_import_definition(
                &import.module_specifier,
                file_path,
                &import.local_name,
            )? {
                Some(def) => return Ok(Some(def)),
                None => {
                    // Module couldn't be resolved, return the import statement
                    return Ok(Some(DefinitionResult::new(
                        SymbolLocation::new(
                            file_path.to_path_buf(),
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

        // Check if this is a direct symbol definition
        if let Some(symbol) = find_symbol_at_range(&file_symbols, range) {
            return Ok(Some(DefinitionResult::new(
                SymbolLocation::new(
                    file_path.to_path_buf(),
                    symbol.range,
                    symbol.kind,
                    symbol.name.clone(),
                ),
                content.to_string(),
                DefinitionKind::Local,
            )));
        }

        Ok(None)
    }

    /// Find all references to a symbol at the given range.
    ///
    /// In lightweight mode, this only searches files that have been processed.
    /// Returns references grouped by file.
    pub fn find_references(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        // Ensure file is in cache
        if !self.cache.contains(file_path) {
            self.process_file(file_path, content)?;
        }

        let (file_symbols, _) =
            self.cache
                .get(file_path)
                .ok_or_else(|| JsSemanticError::FileNotCached {
                    path: file_path.to_path_buf(),
                })?;

        // Group locations by file path
        let mut files_map: HashMap<std::path::PathBuf, Vec<SymbolLocation>> = HashMap::new();

        // Find the symbol at the given range
        let symbol = find_symbol_at_range(&file_symbols, range);

        if let Some(sym) = symbol {
            // Find all references to this symbol in the same file (excluding the definition)
            for reference in file_symbols.find_references_to(sym.symbol_id) {
                files_map
                    .entry(file_path.to_path_buf())
                    .or_default()
                    .push(SymbolLocation::new(
                        file_path.to_path_buf(),
                        reference.range,
                        sym.kind,
                        sym.name.clone(),
                    ));
            }

            // Search other cached files for potential references
            // (This is limited in lightweight mode - we only find references
            // in files that have already been processed)
            for cached_path in self.cache.files() {
                if cached_path == file_path {
                    continue;
                }

                if let Some((other_symbols, _)) = self.cache.get(&cached_path) {
                    // Look for imports of this symbol
                    for import in &other_symbols.imports {
                        if import.local_name == sym.name
                            || import
                                .imported_name
                                .as_ref()
                                .map(|n| n == &sym.name)
                                .unwrap_or(false)
                        {
                            // Check if this import could be from our file
                            // (simplified check - real impl would resolve paths)
                            files_map.entry(cached_path.clone()).or_default().push(
                                SymbolLocation::new(
                                    cached_path.clone(),
                                    import.range,
                                    language_core::SymbolKind::Import,
                                    import.local_name.clone(),
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
            let file_content = if path == file_path {
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

    /// Try to resolve an import to its definition.
    /// Returns `DefinitionKind::External` for successfully resolved cross-file definitions.
    fn resolve_import_definition(
        &self,
        module_specifier: &str,
        from_path: &Path,
        symbol_name: &str,
    ) -> SemanticResult<Option<DefinitionResult>> {
        // For relative imports, try to find the file in the cache
        if module_specifier.starts_with('.') {
            let from_dir = from_path.parent().unwrap_or(Path::new(""));

            // Try common extensions
            let extensions = [
                "",
                ".ts",
                ".tsx",
                ".js",
                ".jsx",
                "/index.ts",
                "/index.tsx",
                "/index.js",
            ];

            for ext in extensions {
                let potential_path = from_dir.join(format!(
                    "{}{}",
                    module_specifier.trim_start_matches("./"),
                    ext
                ));

                if let Some((file_symbols, file_content)) = self.cache.get(&potential_path) {
                    // Look for the exported symbol
                    if let Some(export) = file_symbols.find_export_by_name(symbol_name) {
                        if let Some(local_id) = export.local_symbol_id {
                            if let Some(symbol) = file_symbols.find_symbol_by_id(local_id) {
                                return Ok(Some(DefinitionResult::new(
                                    SymbolLocation::new(
                                        potential_path,
                                        symbol.range,
                                        symbol.kind,
                                        symbol.name.clone(),
                                    ),
                                    file_content.to_string(),
                                    DefinitionKind::External,
                                )));
                            }
                        }
                        // Return the export location if we can't find the actual symbol
                        return Ok(Some(DefinitionResult::new(
                            SymbolLocation::new(
                                potential_path,
                                export.range,
                                language_core::SymbolKind::Export,
                                export.name.clone(),
                            ),
                            file_content.to_string(),
                            DefinitionKind::External,
                        )));
                    }

                    // Check for default export
                    if symbol_name == "default" {
                        if let Some(export) = file_symbols.get_default_export() {
                            return Ok(Some(DefinitionResult::new(
                                SymbolLocation::new(
                                    potential_path,
                                    export.range,
                                    language_core::SymbolKind::Export,
                                    "default".to_string(),
                                ),
                                file_content.to_string(),
                                DefinitionKind::External,
                            )));
                        }
                    }
                }
            }
        }

        // Module not in cache
        Ok(None)
    }

    /// Get the symbol cache (for testing/debugging).
    pub fn cache(&self) -> &SymbolCache {
        &self.cache
    }

    /// Clear the symbol cache.
    pub fn clear_cache(&self) {
        self.cache.clear();
        self.flows.clear();
    }

    /// Follow the value at `range` backward or forward within its module.
    pub fn data_flow(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: FlowOptions,
    ) -> SemanticResult<FlowResult> {
        self.process_file_with_flow(file_path, content)?;
        Ok(flow_query::data_flow(self, file_path, range, options))
    }

    /// Walk the call graph from the function or call at `range`. File scope
    /// does not resolve imports: calls of imported functions are unresolved.
    pub fn call_graph(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: CallGraphOptions,
    ) -> SemanticResult<CallGraphResult> {
        self.process_file_with_flow(file_path, content)?;
        Ok(flow_query::call_graph(self, file_path, range, options))
    }

    /// Places that change the binding at `range` or the value it holds.
    pub fn mutations(
        &self,
        file_path: &Path,
        content: &str,
        range: ByteRange,
        options: MutationOptions,
    ) -> SemanticResult<MutationsResult> {
        self.process_file_with_flow(file_path, content)?;
        Ok(flow_query::mutations(self, file_path, range, options))
    }
}

impl FlowHost for LightweightAnalyzer {
    fn canonical(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    fn flow(&self, path: &Path) -> Option<Arc<FileFlow>> {
        let hash = self.cache.content_hash(path)?;
        self.flows
            .get_or_build(path, hash, || self.cache.content(path))
    }

    fn content(&self, path: &Path) -> Option<Arc<str>> {
        self.cache.content(path)
    }

    fn resolve_import(&self, _: &Path, _: &str, _: &str) -> Option<(PathBuf, ByteRange)> {
        None
    }

    fn workspace_files(&self) -> Vec<PathBuf> {
        self.cache.files()
    }

    fn import_parse_error(&self, _: &Path, _: &str) -> Option<UnparsedFile> {
        None
    }

    fn parse_failures(&self) -> Vec<UnparsedFile> {
        Vec::new()
    }

    /// File scope sees only the files it was given so far, so the callers
    /// table is rebuilt on each query rather than frozen at the first one.
    fn caller_index(&self) -> Option<&CallerIndexSlot> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lightweight_process_file() {
        let analyzer = LightweightAnalyzer::new();

        let content = r#"
const x = 1;
const y = x + 2;
        "#;

        let result = analyzer.process_file(Path::new("test.ts"), content);
        assert!(result.is_ok());
        assert!(analyzer.cache.contains(Path::new("test.ts")));
    }

    #[test]
    fn test_lightweight_get_definition_same_file() {
        let analyzer = LightweightAnalyzer::new();

        let content = r#"const x = 1;
const y = x + 2;"#;

        // Process the file first
        analyzer
            .process_file(Path::new("test.ts"), content)
            .unwrap();

        // The reference to 'x' on line 2 should resolve to the definition on line 1
        // x is at bytes 6-7 in the definition
        let result = analyzer.get_definition(
            Path::new("test.ts"),
            content,
            ByteRange::new(6, 7), // "x" in "const x"
            DefinitionOptions::default(),
        );

        assert!(result.is_ok());
        let definition = result.unwrap();
        assert!(definition.is_some());
        let def = definition.unwrap();
        assert_eq!(def.location.name, "x");
        assert!(!def.content.is_empty());
        assert_eq!(def.kind, DefinitionKind::Local);
    }

    #[test]
    fn test_lightweight_find_references() {
        let analyzer = LightweightAnalyzer::new();

        let content = r#"const x = 1;
const y = x + 2;
console.log(x);"#;

        analyzer
            .process_file(Path::new("test.ts"), content)
            .unwrap();

        // Find references to 'x'
        let result = analyzer.find_references(
            Path::new("test.ts"),
            content,
            ByteRange::new(6, 7), // "x" in "const x"
        );

        assert!(result.is_ok());
        let refs = result.unwrap();
        // Should find at least the definition and one reference
        assert!(!refs.is_empty());
        assert!(!refs.files.is_empty());
        // Each file should have content
        for file in &refs.files {
            assert!(!file.content.is_empty());
        }
    }
}
