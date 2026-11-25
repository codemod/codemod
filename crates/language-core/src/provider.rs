//! Core trait for semantic analysis providers.

use crate::{
    ByteRange, CallGraphOptions, CallGraphResult, DefinitionOptions, DefinitionResult, EntryPoint,
    EntryPointRule, FlowOptions, FlowResult, ImportersResult, MutationOptions, MutationsResult,
    ReferencesResult, SemanticError, SemanticResult, SpecifierOptions,
};
use std::path::{Path, PathBuf};

/// Provider mode determines the analysis strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderMode {
    /// FileScope mode: Single-file analysis with no cross-file resolution.
    /// - Fast startup, no upfront indexing
    /// - Caches symbols as files are processed
    /// - Only finds references within the same file
    /// - Best for quick dry runs or high-level analysis
    #[default]
    FileScope,

    /// WorkspaceScope mode: Workspace-wide analysis with full cross-file support.
    /// - Full workspace analysis when semantic queries are made
    /// - Indexes files lazily on first query
    /// - Accurate cross-file references
    /// - Higher memory usage, slower initial queries
    WorkspaceScope,
}

/// Core trait for semantic analysis providers.
///
/// Implementations provide language-specific symbol indexing and lookup
/// capabilities. The provider can operate in different modes depending
/// on the accuracy/performance tradeoff desired.
///
/// # Thread Safe
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
    /// * `options` - Options controlling definition resolution behavior
    ///
    /// # Returns
    ///
    /// * `Ok(Some(result))` - The definition was found, includes file content and kind
    /// * `Ok(None)` - No definition found (e.g., external/built-in symbol)
    /// * `Err(e)` - An error occurred during lookup
    ///
    /// # Definition Kinds
    ///
    /// The returned `DefinitionResult` includes a `kind` field:
    /// - `Local`: Definition is in the same file
    /// - `Import`: Definition traced to an import statement (module couldn't be resolved)
    /// - `External`: Definition resolved to a different file in the workspace
    fn get_definition(
        &self,
        file_path: &Path,
        range: ByteRange,
        options: DefinitionOptions,
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

    /// Follow the value at the given byte range through the value-flow graph:
    /// backward to what it depends on, or forward to where it goes.
    ///
    /// The default is an empty result, for providers without data-flow analysis.
    fn get_data_flow(
        &self,
        _file_path: &Path,
        _range: ByteRange,
        _options: FlowOptions,
    ) -> SemanticResult<FlowResult> {
        Ok(FlowResult::default())
    }

    /// Walk the call graph from the function (or call) at the given byte range.
    ///
    /// The default is an empty result, for providers without a call graph.
    fn get_call_graph(
        &self,
        _file_path: &Path,
        _range: ByteRange,
        _options: CallGraphOptions,
    ) -> SemanticResult<CallGraphResult> {
        Ok(CallGraphResult::default())
    }

    /// Find the places that change the binding at the given byte range, or
    /// the value it holds.
    ///
    /// The default is an empty result, for providers without mutation analysis.
    fn get_mutations(
        &self,
        _file_path: &Path,
        _range: ByteRange,
        _options: MutationOptions,
    ) -> SemanticResult<MutationsResult> {
        Ok(MutationsResult::default())
    }

    /// Resolve `specifier` the way an import in `from_file` would: tsconfig
    /// `paths` (with `extends`), workspace packages, `node_modules`, index
    /// files and extensions. `Ok(None)` when it does not resolve.
    ///
    /// The default is an `Unsupported` error, for providers without module
    /// resolution.
    fn resolve_module(
        &self,
        _from_file: &Path,
        _specifier: &str,
    ) -> SemanticResult<Option<PathBuf>> {
        Err(unsupported("resolveModule"))
    }

    /// What `export_name` (`"default"` for the default export) of the module
    /// at `module_path` lands on, following re-exports. `Ok(None)` when the
    /// module does not export the name.
    ///
    /// The default is an `Unsupported` error.
    fn resolve_export(
        &self,
        _module_path: &Path,
        _export_name: &str,
    ) -> SemanticResult<Option<DefinitionResult>> {
        Err(unsupported("resolveExport"))
    }

    /// A specifier that an import in `from_file` can use to reach `target`,
    /// checked by resolving it. `Ok(None)` when none is found.
    ///
    /// The default is an `Unsupported` error.
    fn specifier_for(
        &self,
        _from_file: &Path,
        _target: &Path,
        _options: &SpecifierOptions,
    ) -> SemanticResult<Option<String>> {
        Err(unsupported("specifierFor"))
    }

    /// Every place in the workspace that refers to the module at `target`:
    /// static imports and re-exports, `require`, `import()`, and test mocks
    /// (`jest.mock`, `vi.importActual`, ...), each resolved to check it
    /// reaches `target`.
    ///
    /// The default is an `Unsupported` error.
    fn importers(&self, _target: &Path) -> SemanticResult<ImportersResult> {
        Err(unsupported("importers"))
    }

    /// The specifiers other packages can import `file` with: its package's
    /// name and `exports` subpaths that resolve to it. Empty when the file
    /// is not part of its package's public API (or not in a named package).
    ///
    /// The default is an `Unsupported` error.
    fn public_specifiers(&self, _file: &Path) -> SemanticResult<Vec<String>> {
        Err(unsupported("publicSpecifiers"))
    }

    /// Rules from the selector's `semanticAnalysis.entryPoints`: functions
    /// they match count as entry points, besides the package's exports.
    ///
    /// The default ignores them, for providers without entry points.
    fn set_entry_point_rules(&self, _rules: Vec<EntryPointRule>) {}

    /// Why the function at the given byte range is called from outside the
    /// workspace's code: its package exports it, or a rule declares it.
    /// Empty when neither applies.
    ///
    /// The default is an `Unsupported` error.
    fn get_entry_points(
        &self,
        _file_path: &Path,
        _range: ByteRange,
    ) -> SemanticResult<Vec<EntryPoint>> {
        Err(no_call_graph("entryPoints"))
    }

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

    /// The text of `file_path` this provider answers about: the file as it
    /// was when the step started, pinned on first read or by
    /// [`Self::notify_file_processed`]. A root whose text differs was edited
    /// in this step, and queries on it throw instead of mapping offsets into
    /// the wrong text.
    ///
    /// The default is `None`: the provider keeps no snapshot, and nothing is
    /// checked.
    fn snapshot_content(&self, _file_path: &Path) -> Option<std::sync::Arc<str>> {
        None
    }

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

fn unsupported(operation: &str) -> SemanticError {
    SemanticError::Unsupported {
        operation: operation.to_string(),
        reason: "this language's semantic provider does not resolve modules".to_string(),
    }
}

fn no_call_graph(operation: &str) -> SemanticError {
    SemanticError::Unsupported {
        operation: operation.to_string(),
        reason: "this provider has no workspace call graph".to_string(),
    }
}
