//! Core analysis using ruff_python_semantic.

use crate::cache::{
    PythonFileSymbols, PythonImport, PythonReference, PythonSymbol, PythonSymbolCache,
};
use crate::error::{PySemanticError, PySemanticResult};
use language_core::{
    ByteRange, DefinitionKind, DefinitionOptions, DefinitionResult, FileReferences,
    ReferencesResult, SymbolKind, SymbolLocation,
};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextRange};
use std::path::{Path, PathBuf};

/// Convert ruff's TextRange to our ByteRange.
fn text_range_to_byte_range(range: TextRange) -> ByteRange {
    ByteRange::new(range.start().to_u32(), range.end().to_u32())
}

/// Parse a Python file and extract symbols.
pub fn parse_and_extract_symbols(
    path: &Path,
    content: &str,
) -> PySemanticResult<PythonFileSymbols> {
    // Parse the Python source
    let parsed = parse_module(content).map_err(|e| PySemanticError::ParseError {
        path: path.to_path_buf(),
        message: format!("{:?}", e),
    })?;

    let mut file_symbols = PythonFileSymbols::new(content.to_string());

    // For now, we'll do a simpler analysis by walking the AST directly
    // since SemanticModel requires more complex setup
    // This is a basic implementation that can be enhanced later

    // Walk the AST to find definitions
    use ruff_python_ast::visitor::{self, Visitor};
    use ruff_python_ast::{Expr, Stmt};

    struct SymbolExtractor<'a> {
        symbols: &'a mut Vec<PythonSymbol>,
        references: &'a mut Vec<PythonReference>,
        imports: &'a mut Vec<PythonImport>,
        binding_id: u32,
        scope_id: u32,
    }

    impl<'a> Visitor<'a> for SymbolExtractor<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(func) => {
                    let range = text_range_to_byte_range(func.name.range());
                    self.symbols.push(PythonSymbol {
                        name: func.name.to_string(),
                        range,
                        kind: SymbolKind::Function,
                        binding_id: self.binding_id,
                        scope_id: self.scope_id,
                    });
                    self.binding_id += 1;

                    // Visit parameters
                    for param in func.parameters.iter() {
                        let param_range = text_range_to_byte_range(param.name().range());
                        self.symbols.push(PythonSymbol {
                            name: param.name().to_string(),
                            range: param_range,
                            kind: SymbolKind::Parameter,
                            binding_id: self.binding_id,
                            scope_id: self.scope_id + 1,
                        });
                        self.binding_id += 1;
                    }

                    // Enter function scope
                    let old_scope = self.scope_id;
                    self.scope_id += 1;
                    visitor::walk_stmt(self, stmt);
                    self.scope_id = old_scope;
                    return;
                }
                Stmt::ClassDef(class) => {
                    let range = text_range_to_byte_range(class.name.range());
                    self.symbols.push(PythonSymbol {
                        name: class.name.to_string(),
                        range,
                        kind: SymbolKind::Class,
                        binding_id: self.binding_id,
                        scope_id: self.scope_id,
                    });
                    self.binding_id += 1;

                    // Enter class scope
                    let old_scope = self.scope_id;
                    self.scope_id += 1;
                    visitor::walk_stmt(self, stmt);
                    self.scope_id = old_scope;
                    return;
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        if let Expr::Name(name) = target {
                            let range = text_range_to_byte_range(name.range());
                            self.symbols.push(PythonSymbol {
                                name: name.id.to_string(),
                                range,
                                kind: SymbolKind::Variable,
                                binding_id: self.binding_id,
                                scope_id: self.scope_id,
                            });
                            self.binding_id += 1;
                        }
                    }
                }
                Stmt::AnnAssign(assign) => {
                    if let Expr::Name(name) = assign.target.as_ref() {
                        let range = text_range_to_byte_range(name.range());
                        self.symbols.push(PythonSymbol {
                            name: name.id.to_string(),
                            range,
                            kind: SymbolKind::Variable,
                            binding_id: self.binding_id,
                            scope_id: self.scope_id,
                        });
                        self.binding_id += 1;
                    }
                }
                Stmt::Import(import) => {
                    for alias in &import.names {
                        let range = text_range_to_byte_range(alias.range());
                        let name = alias
                            .asname
                            .as_ref()
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| alias.name.to_string());
                        self.symbols.push(PythonSymbol {
                            name: name.clone(),
                            range,
                            kind: SymbolKind::Import,
                            binding_id: self.binding_id,
                            scope_id: self.scope_id,
                        });
                        self.imports.push(PythonImport {
                            module: alias.name.to_string(),
                            name: None,
                            alias: alias.asname.as_ref().map(|n| n.to_string()),
                            range,
                        });
                        self.binding_id += 1;
                    }
                }
                Stmt::ImportFrom(import) => {
                    let module_name = import
                        .module
                        .as_ref()
                        .map(|m| m.to_string())
                        .unwrap_or_default();
                    for alias in &import.names {
                        let range = text_range_to_byte_range(alias.range());
                        let local_name = alias
                            .asname
                            .as_ref()
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| alias.name.to_string());
                        self.symbols.push(PythonSymbol {
                            name: local_name.clone(),
                            range,
                            kind: SymbolKind::Import,
                            binding_id: self.binding_id,
                            scope_id: self.scope_id,
                        });
                        self.imports.push(PythonImport {
                            module: module_name.clone(),
                            name: Some(alias.name.to_string()),
                            alias: alias.asname.as_ref().map(|n| n.to_string()),
                            range,
                        });
                        self.binding_id += 1;
                    }
                }
                _ => {}
            }
            visitor::walk_stmt(self, stmt);
        }

        fn visit_expr(&mut self, expr: &'a Expr) {
            // Track name references
            if let Expr::Name(name) = expr {
                // Check if this is a reference (not a definition)
                // For simplicity, we consider all Name expressions as potential references
                let range = text_range_to_byte_range(name.range());

                // Find if this name refers to a known symbol
                let name_str = name.id.to_string();
                if let Some(symbol) = self.symbols.iter().find(|s| s.name == name_str) {
                    self.references.push(PythonReference {
                        range,
                        binding_id: symbol.binding_id,
                    });
                }
            }
            visitor::walk_expr(self, expr);
        }
    }

    let mut extractor = SymbolExtractor {
        symbols: &mut file_symbols.symbols,
        references: &mut file_symbols.references,
        imports: &mut file_symbols.imports,
        binding_id: 0,
        scope_id: 0,
    };

    for stmt in parsed.suite() {
        extractor.visit_stmt(stmt);
    }

    Ok(file_symbols)
}

/// File-scope analyzer for single-file Python analysis.
#[derive(Debug)]
pub struct FileScopeAnalyzer {
    cache: PythonSymbolCache,
}

impl FileScopeAnalyzer {
    /// Create a new file-scope analyzer.
    pub fn new() -> Self {
        Self {
            cache: PythonSymbolCache::new(),
        }
    }

    /// Get the symbol cache.
    pub fn cache(&self) -> &PythonSymbolCache {
        &self.cache
    }

    /// Clear the cache.
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Process a file and cache its symbols.
    pub fn process_file(&self, path: &Path, content: &str) -> PySemanticResult<()> {
        // Skip if already cached and content hasn't changed
        if self.cache.is_valid(path, content) {
            return Ok(());
        }

        let file_symbols = parse_and_extract_symbols(path, content)?;
        self.cache.insert(path.to_path_buf(), file_symbols);

        Ok(())
    }

    /// Get definition for a symbol at the given range.
    pub fn get_definition(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
        options: DefinitionOptions,
    ) -> PySemanticResult<Option<DefinitionResult>> {
        // Ensure file is processed
        self.process_file(path, content)?;

        let file_symbols = self
            .cache
            .get(path)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: path.to_path_buf(),
            })?;

        // First, check if we're on a reference
        if let Some(reference) = file_symbols.find_reference_at(range) {
            // Find the binding this reference points to
            if let Some(symbol) = file_symbols.find_symbol_by_id(reference.binding_id) {
                // Check if this is an import
                let kind = if symbol.kind == SymbolKind::Import {
                    // If resolve_external is false, return the import directly
                    if !options.resolve_external {
                        return Ok(Some(DefinitionResult::new(
                            SymbolLocation::new(
                                path.to_path_buf(),
                                symbol.range,
                                symbol.kind,
                                symbol.name.clone(),
                            ),
                            content.to_string(),
                            DefinitionKind::Import,
                        )));
                    }
                    // In file-scope mode, we can't resolve imports, so return Import kind
                    DefinitionKind::Import
                } else {
                    DefinitionKind::Local
                };

                return Ok(Some(DefinitionResult::new(
                    SymbolLocation::new(
                        path.to_path_buf(),
                        symbol.range,
                        symbol.kind,
                        symbol.name.clone(),
                    ),
                    content.to_string(),
                    kind,
                )));
            }
        }

        // Check if we're on a symbol definition itself
        if let Some(symbol) = file_symbols.find_symbol_at(range) {
            let kind = if symbol.kind == SymbolKind::Import {
                DefinitionKind::Import
            } else {
                DefinitionKind::Local
            };

            return Ok(Some(DefinitionResult::new(
                SymbolLocation::new(
                    path.to_path_buf(),
                    symbol.range,
                    symbol.kind,
                    symbol.name.clone(),
                ),
                content.to_string(),
                kind,
            )));
        }

        Ok(None)
    }

    /// Find all references to a symbol at the given range.
    pub fn find_references(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
    ) -> PySemanticResult<ReferencesResult> {
        // Ensure file is processed
        self.process_file(path, content)?;

        let file_symbols = self
            .cache
            .get(path)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: path.to_path_buf(),
            })?;

        let mut result = ReferencesResult::new();

        // Find the symbol at the given range
        let binding_id = if let Some(symbol) = file_symbols.find_symbol_at(range) {
            symbol.binding_id
        } else if let Some(reference) = file_symbols.find_reference_at(range) {
            reference.binding_id
        } else {
            return Ok(result);
        };

        // Get the symbol for creating SymbolLocation
        let symbol = file_symbols.find_symbol_by_id(binding_id);

        // Find all references to this binding
        let locations: Vec<SymbolLocation> = file_symbols
            .find_references_to(binding_id)
            .into_iter()
            .map(|r| {
                SymbolLocation::new(
                    path.to_path_buf(),
                    r.range,
                    symbol.map(|s| s.kind).unwrap_or(SymbolKind::Unknown),
                    symbol.map(|s| s.name.clone()).unwrap_or_default(),
                )
            })
            .collect();

        if !locations.is_empty() {
            result.files.push(FileReferences::new(
                path.to_path_buf(),
                content.to_string(),
                locations,
            ));
        }

        Ok(result)
    }
}

impl Default for FileScopeAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Workspace-scope analyzer for cross-file Python analysis.
#[derive(Debug)]
pub struct WorkspaceScopeAnalyzer {
    workspace_root: PathBuf,
    cache: PythonSymbolCache,
}

impl WorkspaceScopeAnalyzer {
    /// Create a new workspace-scope analyzer.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            cache: PythonSymbolCache::new(),
        }
    }

    /// Get the workspace root.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Get the symbol cache.
    pub fn cache(&self) -> &PythonSymbolCache {
        &self.cache
    }

    /// Clear the cache.
    pub fn clear(&self) {
        self.cache.clear();
    }

    /// Process a file and cache its symbols.
    pub fn process_file(&self, path: &Path, content: &str) -> PySemanticResult<()> {
        // Skip if already cached and content hasn't changed
        if self.cache.is_valid(path, content) {
            return Ok(());
        }

        let file_symbols = parse_and_extract_symbols(path, content)?;
        self.cache.insert(path.to_path_buf(), file_symbols);

        Ok(())
    }

    /// Get definition for a symbol at the given range.
    /// In workspace mode, this can resolve cross-file definitions.
    pub fn get_definition(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
        options: DefinitionOptions,
    ) -> PySemanticResult<Option<DefinitionResult>> {
        // Ensure file is processed
        self.process_file(path, content)?;

        let file_symbols = self
            .cache
            .get(path)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: path.to_path_buf(),
            })?;

        // First, check if we're on a reference
        if let Some(reference) = file_symbols.find_reference_at(range) {
            // Find the binding this reference points to
            if let Some(symbol) = file_symbols.find_symbol_by_id(reference.binding_id) {
                // Check if this is an import - if so, try to resolve to the source
                if symbol.kind == SymbolKind::Import {
                    // If resolve_external is false, return the import statement directly
                    if !options.resolve_external {
                        return Ok(Some(DefinitionResult::new(
                            SymbolLocation::new(
                                path.to_path_buf(),
                                symbol.range,
                                symbol.kind,
                                symbol.name.clone(),
                            ),
                            content.to_string(),
                            DefinitionKind::Import,
                        )));
                    }

                    // Try to resolve the import
                    if let Some(import) = file_symbols.imports.iter().find(|i| {
                        i.range.start <= symbol.range.start && i.range.end >= symbol.range.end
                    }) {
                        // Try to resolve the import to a file
                        if let Some(resolved) = self.resolve_import(&import.module, path) {
                            if let Ok(resolved_content) = std::fs::read_to_string(&resolved) {
                                // Process the resolved file
                                let _ = self.process_file(&resolved, &resolved_content);

                                // Try to find the symbol in the resolved file
                                if let Some(resolved_symbols) = self.cache.get(&resolved) {
                                    let import_name = import.name.as_ref().unwrap_or(&symbol.name);
                                    if let Some(target_symbol) = resolved_symbols
                                        .symbols
                                        .iter()
                                        .find(|s| &s.name == import_name)
                                    {
                                        return Ok(Some(DefinitionResult::new(
                                            SymbolLocation::new(
                                                resolved.clone(),
                                                target_symbol.range,
                                                target_symbol.kind,
                                                target_symbol.name.clone(),
                                            ),
                                            resolved_content,
                                            DefinitionKind::External,
                                        )));
                                    }
                                }
                            }
                        }
                    }

                    // Module couldn't be resolved, return the import statement
                    return Ok(Some(DefinitionResult::new(
                        SymbolLocation::new(
                            path.to_path_buf(),
                            symbol.range,
                            symbol.kind,
                            symbol.name.clone(),
                        ),
                        content.to_string(),
                        DefinitionKind::Import,
                    )));
                }

                return Ok(Some(DefinitionResult::new(
                    SymbolLocation::new(
                        path.to_path_buf(),
                        symbol.range,
                        symbol.kind,
                        symbol.name.clone(),
                    ),
                    content.to_string(),
                    DefinitionKind::Local,
                )));
            }
        }

        // Check if we're on a symbol definition itself
        if let Some(symbol) = file_symbols.find_symbol_at(range) {
            let kind = if symbol.kind == SymbolKind::Import {
                DefinitionKind::Import
            } else {
                DefinitionKind::Local
            };

            return Ok(Some(DefinitionResult::new(
                SymbolLocation::new(
                    path.to_path_buf(),
                    symbol.range,
                    symbol.kind,
                    symbol.name.clone(),
                ),
                content.to_string(),
                kind,
            )));
        }

        Ok(None)
    }

    /// Find all references to a symbol at the given range.
    /// In workspace mode, this searches across all cached files.
    pub fn find_references(
        &self,
        path: &Path,
        content: &str,
        range: ByteRange,
    ) -> PySemanticResult<ReferencesResult> {
        // Ensure file is processed
        self.process_file(path, content)?;

        let file_symbols = self
            .cache
            .get(path)
            .ok_or_else(|| PySemanticError::FileNotCached {
                path: path.to_path_buf(),
            })?;

        let mut result = ReferencesResult::new();

        // Find the symbol at the given range
        let (binding_id, symbol_name, symbol_kind) =
            if let Some(symbol) = file_symbols.find_symbol_at(range) {
                (symbol.binding_id, symbol.name.clone(), symbol.kind)
            } else if let Some(reference) = file_symbols.find_reference_at(range) {
                let sym = file_symbols.find_symbol_by_id(reference.binding_id);
                let name = sym.map(|s| s.name.clone()).unwrap_or_default();
                let kind = sym.map(|s| s.kind).unwrap_or(SymbolKind::Unknown);
                (reference.binding_id, name, kind)
            } else {
                return Ok(result);
            };

        // Find references in the current file
        let locations: Vec<SymbolLocation> = file_symbols
            .find_references_to(binding_id)
            .into_iter()
            .map(|r| {
                SymbolLocation::new(
                    path.to_path_buf(),
                    r.range,
                    symbol_kind,
                    symbol_name.clone(),
                )
            })
            .collect();

        if !locations.is_empty() {
            result.files.push(FileReferences::new(
                path.to_path_buf(),
                content.to_string(),
                locations,
            ));
        }

        // Search other cached files for references by name
        for cached_path in self.cache.files() {
            if cached_path == path {
                continue;
            }

            if let Some(other_symbols) = self.cache.get(&cached_path) {
                // Look for imports of this symbol
                for import in &other_symbols.imports {
                    if import.name.as_ref() == Some(&symbol_name)
                        || import.alias.as_ref() == Some(&symbol_name)
                    {
                        // IMPORTANT: Verify that this import actually comes from the file
                        // where the symbol is defined, not just any file with the same symbol name
                        if let Some(resolved_import_path) =
                            self.resolve_import(&import.module, &cached_path)
                        {
                            // Normalize both paths for comparison
                            let resolved_canonical = resolved_import_path
                                .canonicalize()
                                .unwrap_or(resolved_import_path);
                            let path_canonical =
                                path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

                            if resolved_canonical != path_canonical {
                                // This import is from a different file, skip it
                                continue;
                            }
                        } else {
                            // Could not resolve the import, skip it to avoid false positives
                            continue;
                        }

                        // Find references to this import in the other file
                        if let Some(import_symbol) = other_symbols.symbols.iter().find(|s| {
                            s.range.start >= import.range.start && s.range.end <= import.range.end
                        }) {
                            let refs: Vec<SymbolLocation> = other_symbols
                                .find_references_to(import_symbol.binding_id)
                                .into_iter()
                                .map(|r| {
                                    SymbolLocation::new(
                                        cached_path.clone(),
                                        r.range,
                                        symbol_kind,
                                        symbol_name.clone(),
                                    )
                                })
                                .collect();

                            if !refs.is_empty() {
                                result.files.push(FileReferences::new(
                                    cached_path.clone(),
                                    other_symbols.content.clone(),
                                    refs,
                                ));
                            }
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Try to resolve a Python import to a file path.
    fn resolve_import(&self, module: &str, from_path: &Path) -> Option<PathBuf> {
        // Convert module path to file path (e.g., "os.path" -> "os/path.py")
        let module_path = module.replace('.', "/");

        // Try relative import first
        if let Some(parent) = from_path.parent() {
            let relative_path = parent.join(&module_path).with_extension("py");
            if relative_path.exists() {
                return Some(relative_path);
            }

            // Try as package (__init__.py)
            let package_path = parent.join(&module_path).join("__init__.py");
            if package_path.exists() {
                return Some(package_path);
            }
        }

        // Try from workspace root
        let workspace_path = self.workspace_root.join(&module_path).with_extension("py");
        if workspace_path.exists() {
            return Some(workspace_path);
        }

        let workspace_package = self.workspace_root.join(&module_path).join("__init__.py");
        if workspace_package.exists() {
            return Some(workspace_package);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_file_scope_analyzer_process_file() {
        let analyzer = FileScopeAnalyzer::new();
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");

        let content = r#"
x = 1
y = x + 2
"#;
        fs::write(&file_path, content).unwrap();

        let result = analyzer.process_file(&file_path, content);
        assert!(result.is_ok());
        assert!(analyzer.cache().contains(&file_path));
    }

    #[test]
    fn test_workspace_scope_analyzer_new() {
        let dir = TempDir::new().unwrap();
        let analyzer = WorkspaceScopeAnalyzer::new(dir.path().to_path_buf());
        assert_eq!(analyzer.workspace_root(), dir.path());
    }
}
