//! Core types for semantic analysis.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A position in source code (0-indexed line and column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    /// 0-indexed line number
    pub line: u32,
    /// 0-indexed column (byte offset within line)
    pub column: u32,
}

impl Position {
    /// Create a new position.
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

/// A byte range in source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    /// Start byte offset (inclusive)
    pub start: u32,
    /// End byte offset (exclusive)
    pub end: u32,
}

impl ByteRange {
    /// Create a new byte range.
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Check if this range contains the given byte offset.
    pub fn contains(&self, offset: u32) -> bool {
        offset >= self.start && offset < self.end
    }

    /// Check if this range overlaps with another range.
    pub fn overlaps(&self, other: &ByteRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Check if this range is contained within another range.
    pub fn is_within(&self, outer: &ByteRange) -> bool {
        self.start >= outer.start && self.end <= outer.end
    }

    /// Get the length of this range in bytes.
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Check if this range is empty.
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// The kind of symbol (variable, function, class, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SymbolKind {
    /// A variable declaration
    Variable,
    /// A function declaration
    Function,
    /// A class declaration
    Class,
    /// A method (function inside a class)
    Method,
    /// A property (field in a class or object)
    Property,
    /// An import statement/binding
    Import,
    /// An export statement/binding
    Export,
    /// A type alias or type declaration
    Type,
    /// An interface declaration (TypeScript)
    Interface,
    /// An enum declaration
    Enum,
    /// An enum member/variant
    EnumMember,
    /// A namespace or module
    Namespace,
    /// A constant declaration
    Constant,
    /// A parameter in a function signature
    Parameter,
    /// A type parameter (generic)
    TypeParameter,
    /// Unknown or unclassified symbol
    Unknown,
}

impl SymbolKind {
    /// Returns a human-readable name for the symbol kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Variable => "variable",
            SymbolKind::Function => "function",
            SymbolKind::Class => "class",
            SymbolKind::Method => "method",
            SymbolKind::Property => "property",
            SymbolKind::Import => "import",
            SymbolKind::Export => "export",
            SymbolKind::Type => "type",
            SymbolKind::Interface => "interface",
            SymbolKind::Enum => "enum",
            SymbolKind::EnumMember => "enumMember",
            SymbolKind::Namespace => "namespace",
            SymbolKind::Constant => "constant",
            SymbolKind::Parameter => "parameter",
            SymbolKind::TypeParameter => "typeParameter",
            SymbolKind::Unknown => "unknown",
        }
    }
}

/// Raw symbol location data - used internally by providers.
/// This is converted to SgNode in the sandbox layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolLocation {
    /// Path to the file containing the symbol
    pub file_path: PathBuf,
    /// Byte range of the symbol in the file
    pub range: ByteRange,
    /// The kind of symbol
    pub kind: SymbolKind,
    /// The name of the symbol
    pub name: String,
}

impl SymbolLocation {
    /// Create a new symbol location.
    pub fn new(file_path: PathBuf, range: ByteRange, kind: SymbolKind, name: String) -> Self {
        Self {
            file_path,
            range,
            kind,
            name,
        }
    }
}

/// References grouped by file path.
/// Each entry contains the file path, its content, and the symbol locations in that file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferences {
    /// Path to the file
    pub file_path: PathBuf,
    /// Source content of the file (needed to create SgRoot)
    pub content: String,
    /// Symbol locations within this file
    pub locations: Vec<SymbolLocation>,
}

impl FileReferences {
    /// Create a new file references entry.
    pub fn new(file_path: PathBuf, content: String, locations: Vec<SymbolLocation>) -> Self {
        Self {
            file_path,
            content,
            locations,
        }
    }
}

/// Result of finding references - grouped by file for easy SgRoot creation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferencesResult {
    /// References grouped by file
    pub files: Vec<FileReferences>,
}

impl ReferencesResult {
    /// Create a new empty result.
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Add references for a file.
    pub fn add_file(&mut self, file_refs: FileReferences) {
        self.files.push(file_refs);
    }

    /// Get total count of all references across all files.
    pub fn total_count(&self) -> usize {
        self.files.iter().map(|f| f.locations.len()).sum()
    }

    /// Check if there are any references.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() || self.files.iter().all(|f| f.locations.is_empty())
    }
}

/// Result of getting a definition - includes file content for SgRoot creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionResult {
    /// The symbol location
    pub location: SymbolLocation,
    /// Source content of the file (needed to create SgRoot)
    pub content: String,
}

impl DefinitionResult {
    /// Create a new definition result.
    pub fn new(location: SymbolLocation, content: String) -> Self {
        Self { location, content }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_range_contains() {
        let range = ByteRange::new(10, 20);
        assert!(range.contains(10));
        assert!(range.contains(15));
        assert!(range.contains(19));
        assert!(!range.contains(9));
        assert!(!range.contains(20));
    }

    #[test]
    fn test_byte_range_overlaps() {
        let range1 = ByteRange::new(10, 20);
        let range2 = ByteRange::new(15, 25);
        let range3 = ByteRange::new(20, 30);
        let range4 = ByteRange::new(0, 10);

        assert!(range1.overlaps(&range2));
        assert!(!range1.overlaps(&range3));
        assert!(!range1.overlaps(&range4));
    }

    #[test]
    fn test_byte_range_is_within() {
        let outer = ByteRange::new(10, 30);
        let inner = ByteRange::new(15, 25);
        let partial = ByteRange::new(5, 20);

        assert!(inner.is_within(&outer));
        assert!(!partial.is_within(&outer));
    }

    #[test]
    fn test_symbol_kind_as_str() {
        assert_eq!(SymbolKind::Variable.as_str(), "variable");
        assert_eq!(SymbolKind::Function.as_str(), "function");
        assert_eq!(SymbolKind::Class.as_str(), "class");
    }

    #[test]
    fn test_references_result() {
        let mut result = ReferencesResult::new();
        assert!(result.is_empty());
        assert_eq!(result.total_count(), 0);

        result.add_file(FileReferences::new(
            PathBuf::from("test.ts"),
            "const x = 1;".to_string(),
            vec![SymbolLocation::new(
                PathBuf::from("test.ts"),
                ByteRange::new(6, 7),
                SymbolKind::Variable,
                "x".to_string(),
            )],
        ));

        assert!(!result.is_empty());
        assert_eq!(result.total_count(), 1);
    }
}
