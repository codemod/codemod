//! Symbol cache for Python semantic analysis.

use language_core::{ByteRange, SymbolKind};
use parking_lot::RwLock;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Information about a Python symbol (binding).
#[derive(Debug, Clone)]
pub struct PythonSymbol {
    /// Name of the symbol.
    pub name: String,
    /// Byte range where the symbol is defined.
    pub range: ByteRange,
    /// Kind of symbol.
    pub kind: SymbolKind,
    /// Internal binding ID from ruff.
    pub binding_id: u32,
    /// Scope ID where this symbol is defined.
    pub scope_id: u32,
}

/// Information about a reference to a symbol.
#[derive(Debug, Clone)]
pub struct PythonReference {
    /// Byte range of the reference.
    pub range: ByteRange,
    /// The binding ID this reference points to.
    pub binding_id: u32,
}

/// Information about an import in a Python file.
#[derive(Debug, Clone)]
pub struct PythonImport {
    /// The module being imported (e.g., "os.path").
    pub module: String,
    /// The name being imported (e.g., "join" for "from os.path import join").
    pub name: Option<String>,
    /// Local alias if any (e.g., "j" for "from os.path import join as j").
    pub alias: Option<String>,
    /// Byte range of the import statement.
    pub range: ByteRange,
}

/// Cached symbols for a single Python file.
#[derive(Debug, Clone)]
pub struct PythonFileSymbols {
    /// All symbols (bindings) defined in this file.
    pub symbols: Vec<PythonSymbol>,
    /// All references in this file.
    pub references: Vec<PythonReference>,
    /// All imports in this file.
    pub imports: Vec<PythonImport>,
    /// The source content of the file.
    pub content: String,
    /// Hash of the content for cache invalidation.
    pub content_hash: u64,
}

impl PythonFileSymbols {
    /// Create a new empty file symbols cache.
    pub fn new(content: String) -> Self {
        let content_hash = hash_content(&content);
        Self {
            symbols: Vec::new(),
            references: Vec::new(),
            imports: Vec::new(),
            content,
            content_hash,
        }
    }

    /// Find a symbol at the given byte range.
    pub fn find_symbol_at(&self, range: ByteRange) -> Option<&PythonSymbol> {
        self.symbols
            .iter()
            .find(|s| s.range.start <= range.start && s.range.end >= range.end)
    }

    /// Find a reference at the given byte range.
    pub fn find_reference_at(&self, range: ByteRange) -> Option<&PythonReference> {
        self.references
            .iter()
            .find(|r| r.range.start <= range.start && r.range.end >= range.end)
    }

    /// Find all references to a specific binding.
    pub fn find_references_to(&self, binding_id: u32) -> Vec<&PythonReference> {
        self.references
            .iter()
            .filter(|r| r.binding_id == binding_id)
            .collect()
    }

    /// Find a symbol by binding ID.
    pub fn find_symbol_by_id(&self, binding_id: u32) -> Option<&PythonSymbol> {
        self.symbols.iter().find(|s| s.binding_id == binding_id)
    }
}

/// Thread-safe cache for Python file symbols.
#[derive(Debug, Default)]
pub struct PythonSymbolCache {
    files: RwLock<HashMap<PathBuf, PythonFileSymbols>>,
}

impl PythonSymbolCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            files: RwLock::new(HashMap::new()),
        }
    }

    /// Insert or update symbols for a file.
    pub fn insert(&self, path: PathBuf, symbols: PythonFileSymbols) {
        let mut files = self.files.write();
        files.insert(path, symbols);
    }

    /// Get symbols for a file.
    pub fn get(&self, path: &Path) -> Option<PythonFileSymbols> {
        let files = self.files.read();
        files.get(path).cloned()
    }

    /// Check if a file is in the cache.
    pub fn contains(&self, path: &Path) -> bool {
        let files = self.files.read();
        files.contains_key(path)
    }

    /// Check if the cached content is still valid.
    pub fn is_valid(&self, path: &Path, content: &str) -> bool {
        let files = self.files.read();
        if let Some(cached) = files.get(path) {
            cached.content_hash == hash_content(content)
        } else {
            false
        }
    }

    /// Get the number of cached files.
    pub fn len(&self) -> usize {
        let files = self.files.read();
        files.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all cached data.
    pub fn clear(&self) {
        let mut files = self.files.write();
        files.clear();
    }

    /// Get all cached file paths.
    pub fn files(&self) -> Vec<PathBuf> {
        let files = self.files.read();
        files.keys().cloned().collect()
    }
}

/// Hash content for cache invalidation.
fn hash_content(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_symbol_cache_basic() {
        let cache = PythonSymbolCache::new();
        let path = PathBuf::from("test.py");

        let mut symbols = PythonFileSymbols::new("x = 1".to_string());
        symbols.symbols.push(PythonSymbol {
            name: "x".to_string(),
            range: ByteRange::new(0, 1),
            kind: SymbolKind::Variable,
            binding_id: 0,
            scope_id: 0,
        });

        cache.insert(path.clone(), symbols);

        assert!(cache.contains(&path));
        assert_eq!(cache.len(), 1);

        let cached = cache.get(&path).unwrap();
        assert_eq!(cached.symbols.len(), 1);
        assert_eq!(cached.symbols[0].name, "x");
    }

    #[test]
    fn test_find_symbol_at() {
        let mut symbols = PythonFileSymbols::new("x = 1".to_string());
        symbols.symbols.push(PythonSymbol {
            name: "x".to_string(),
            range: ByteRange::new(0, 1),
            kind: SymbolKind::Variable,
            binding_id: 0,
            scope_id: 0,
        });

        let found = symbols.find_symbol_at(ByteRange::new(0, 1));
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "x");
    }

    #[test]
    fn test_find_references_to() {
        let mut symbols = PythonFileSymbols::new("x = 1\ny = x".to_string());
        symbols.references.push(PythonReference {
            range: ByteRange::new(10, 11),
            binding_id: 0,
        });

        let refs = symbols.find_references_to(0);
        assert_eq!(refs.len(), 1);
    }
}
