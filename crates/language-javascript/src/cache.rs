//! Symbol cache for incremental indexing.

use language_core::{ByteRange, SymbolKind};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Information about a symbol defined in a file.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The name of the symbol
    pub name: String,
    /// The kind of symbol
    pub kind: SymbolKind,
    /// Byte range in the source file
    pub range: ByteRange,
    /// Unique symbol ID (from oxc)
    pub symbol_id: u32,
    /// Scope ID this symbol belongs to
    #[allow(dead_code)]
    pub scope_id: u32,
}

/// Information about an imported symbol.
#[derive(Debug, Clone)]
pub struct ImportedSymbol {
    /// Local name (how it's used in this file)
    pub local_name: String,
    /// Original name from the module
    pub imported_name: Option<String>,
    /// Module specifier (e.g., "./utils" or "react")
    pub module_specifier: String,
    /// Byte range of the import statement
    pub range: ByteRange,
    /// Whether this is a default import
    pub is_default: bool,
    /// Whether this is a namespace import (import * as X)
    #[allow(dead_code)]
    pub is_namespace: bool,
}

/// Information about an exported symbol.
#[derive(Debug, Clone)]
pub struct ExportedSymbol {
    /// Name of the export
    pub name: String,
    /// Local symbol ID that this export refers to
    pub local_symbol_id: Option<u32>,
    /// Byte range of the export
    pub range: ByteRange,
    /// Whether this is the default export
    pub is_default: bool,
    /// Whether this is a re-export (export { x } from './other')
    #[allow(dead_code)]
    pub re_export_from: Option<String>,
}

/// Information about a reference to a symbol.
#[derive(Debug, Clone)]
pub struct SymbolReference {
    /// The symbol ID being referenced
    pub symbol_id: u32,
    /// Byte range of the reference
    pub range: ByteRange,
    /// Whether this is a write reference
    #[allow(dead_code)]
    pub is_write: bool,
}

/// Cached symbols for a single file.
#[derive(Debug, Clone, Default)]
pub struct FileSymbols {
    /// All symbols defined in this file
    pub symbols: Vec<Symbol>,
    /// All imports in this file
    pub imports: Vec<ImportedSymbol>,
    /// All exports from this file
    pub exports: Vec<ExportedSymbol>,
    /// All references in this file
    pub references: Vec<SymbolReference>,
    /// Source content hash (for invalidation)
    #[allow(dead_code)]
    pub content_hash: u64,
}

impl FileSymbols {
    /// Create empty file symbols.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Find a symbol by its byte range.
    pub fn find_symbol_at(&self, range: ByteRange) -> Option<&Symbol> {
        // Find the tightest symbol that contains or matches the range
        self.symbols
            .iter()
            .filter(|s| s.range.start <= range.start && s.range.end >= range.end)
            .min_by_key(|s| s.range.len())
    }

    /// Find a symbol by its ID.
    pub fn find_symbol_by_id(&self, symbol_id: u32) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.symbol_id == symbol_id)
    }

    /// Find an import at the given byte range.
    pub fn find_import_at(&self, range: ByteRange) -> Option<&ImportedSymbol> {
        self.imports.iter().find(|i| i.range.overlaps(&range))
    }

    /// Find all references to a symbol.
    pub fn find_references_to(&self, symbol_id: u32) -> Vec<&SymbolReference> {
        self.references
            .iter()
            .filter(|r| r.symbol_id == symbol_id)
            .collect()
    }

    /// Find an export by name.
    pub fn find_export_by_name(&self, name: &str) -> Option<&ExportedSymbol> {
        self.exports.iter().find(|e| e.name == name)
    }

    /// Get the default export.
    pub fn get_default_export(&self) -> Option<&ExportedSymbol> {
        self.exports.iter().find(|e| e.is_default)
    }
}

/// Cached entry containing both symbols and source content.
#[derive(Debug, Clone)]
struct CacheEntry {
    symbols: FileSymbols,
    content: String,
}

/// Thread-safe symbol cache for multiple files.
#[derive(Debug, Default)]
pub struct SymbolCache {
    /// Map from file path to cached symbols and content
    files: RwLock<HashMap<PathBuf, CacheEntry>>,
}

impl SymbolCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get cached symbols and content for a file.
    pub fn get(&self, path: &Path) -> Option<(FileSymbols, String)> {
        self.files
            .read()
            .get(path)
            .map(|e| (e.symbols.clone(), e.content.clone()))
    }

    /// Check if a file is in the cache.
    pub fn contains(&self, path: &Path) -> bool {
        self.files.read().contains_key(path)
    }

    /// Insert or update cached symbols and content for a file.
    pub fn insert(&self, path: PathBuf, symbols: FileSymbols, content: String) {
        self.files
            .write()
            .insert(path, CacheEntry { symbols, content });
    }

    /// Remove a file from the cache.
    #[allow(dead_code)]
    pub fn remove(&self, path: &Path) -> Option<(FileSymbols, String)> {
        self.files
            .write()
            .remove(path)
            .map(|e| (e.symbols, e.content))
    }

    /// Clear the entire cache.
    pub fn clear(&self) {
        self.files.write().clear();
    }

    /// Get the number of cached files.
    pub fn len(&self) -> usize {
        self.files.read().len()
    }

    /// Check if the cache is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.files.read().is_empty()
    }

    /// Get all cached file paths.
    pub fn files(&self) -> Vec<PathBuf> {
        self.files.read().keys().cloned().collect()
    }
}

/// Simple hash function for content (for cache invalidation).
pub fn hash_content(content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_cache_basic() {
        let cache = SymbolCache::new();
        let path = PathBuf::from("test.ts");

        assert!(!cache.contains(&path));
        assert!(cache.get(&path).is_none());

        let symbols = FileSymbols {
            symbols: vec![Symbol {
                name: "foo".to_string(),
                kind: SymbolKind::Function,
                range: ByteRange::new(0, 10),
                symbol_id: 1,
                scope_id: 0,
            }],
            ..Default::default()
        };

        cache.insert(path.clone(), symbols, "function foo() {}".to_string());

        assert!(cache.contains(&path));
        let (retrieved, content) = cache.get(&path).unwrap();
        assert_eq!(retrieved.symbols.len(), 1);
        assert_eq!(retrieved.symbols[0].name, "foo");
        assert_eq!(content, "function foo() {}");
    }

    #[test]
    fn test_file_symbols_find_at() {
        let symbols = FileSymbols {
            symbols: vec![
                Symbol {
                    name: "outer".to_string(),
                    kind: SymbolKind::Function,
                    range: ByteRange::new(0, 100),
                    symbol_id: 1,
                    scope_id: 0,
                },
                Symbol {
                    name: "inner".to_string(),
                    kind: SymbolKind::Variable,
                    range: ByteRange::new(20, 40),
                    symbol_id: 2,
                    scope_id: 1,
                },
            ],
            ..Default::default()
        };

        // Should find the tightest match
        let found = symbols.find_symbol_at(ByteRange::new(25, 30));
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "inner");

        // Should find outer when range is outside inner
        let found = symbols.find_symbol_at(ByteRange::new(50, 60));
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "outer");
    }

    #[test]
    fn test_hash_content() {
        let content1 = "function foo() {}";
        let content2 = "function bar() {}";
        let content1_copy = "function foo() {}";

        let hash1 = hash_content(content1);
        let hash2 = hash_content(content2);
        let hash1_copy = hash_content(content1_copy);

        assert_ne!(hash1, hash2);
        assert_eq!(hash1, hash1_copy);
    }
}
