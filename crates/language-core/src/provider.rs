//! Core trait for semantic analysis providers.

use crate::{ByteRange, DefinitionResult, ReferencesResult, SemanticResult};
use std::path::Path;

/// Provider mode determines the analysis strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderMode {
    /// Lightweight mode: Per-file analysis with incremental caching.
    /// - Fast startup, no upfront indexing
    /// - Caches symbols as files are processed
    /// - Cross-file references resolved on-demand
    /// - May miss some references if files haven't been processed
    #[default]
    Lightweight,

    /// Accurate mode: Workspace-wide lazy indexing.
    /// - Full workspace analysis when semantic queries are made
    /// - Indexes files lazily on first query
    /// - More accurate cross-file references
    /// - Higher memory usage, slower initial queries
    Accurate,
}

/// Core trait for semantic analysis providers.
///
/// Implementations provide language-specific symbol indexing and lookup
/// capabilities. The provider can operate in different modes depending
/// on the accuracy/performance tradeoff desired.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow concurrent access
/// from multiple threads during codemod execution.
pub trait SemanticProvider: Send + Sync {
    /// Get the definition for a symbol at the given byte range.
    ///
    /// Returns the definition location along with the file content,
    /// which allows the caller to create an SgRoot for the target file.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the file containing the symbol reference
    /// * `range` - Byte range of the symbol in the source file
    ///
    /// # Returns
    ///
    /// * `Ok(Some(result))` - The definition was found, includes file content
    /// * `Ok(None)` - No definition found (e.g., external/built-in symbol)
    /// * `Err(e)` - An error occurred during lookup
    fn get_definition(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<Option<DefinitionResult>>;

    /// Find all references to a symbol at the given byte range.
    ///
    /// Returns references grouped by file, with each file including its content.
    /// This allows the caller to create SgRoot objects for each file and
    /// use ast-grep's range selector to find the actual nodes.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the file containing the symbol
    /// * `range` - Byte range of the symbol in the source file
    ///
    /// # Returns
    ///
    /// References grouped by file, each with file content and locations.
    /// In lightweight mode, this may only include references from
    /// files that have been processed. In accurate mode, this will
    /// include all references in the workspace.
    fn find_references(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult>;

    /// Get type information for a symbol at the given byte range.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the file containing the symbol
    /// * `range` - Byte range of the symbol in the source file
    ///
    /// # Returns
    ///
    /// * `Ok(Some(type_string))` - The type was resolved
    /// * `Ok(None)` - Type information not available
    /// * `Err(e)` - An error occurred during lookup
    fn get_type(&self, file_path: &Path, range: ByteRange) -> SemanticResult<Option<String>>;

    /// Notify the provider that a file has been processed.
    ///
    /// This is called after each file is processed during codemod execution.
    /// In lightweight mode, this allows the provider to build up its symbol
    /// cache incrementally.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the processed file
    /// * `content` - The source content of the file
    fn notify_file_processed(&self, file_path: &Path, content: &str) -> SemanticResult<()>;

    /// Check if this provider supports the given language.
    ///
    /// # Arguments
    ///
    /// * `lang` - Language identifier (e.g., "javascript", "typescript", "css")
    ///
    /// # Returns
    ///
    /// `true` if this provider can handle the language, `false` otherwise.
    fn supports_language(&self, lang: &str) -> bool;

    /// Get the current provider mode.
    fn mode(&self) -> ProviderMode;
}
