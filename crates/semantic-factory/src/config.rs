//! Configuration types for semantic analysis.

use std::path::PathBuf;

/// Determines the scope of semantic analysis.
#[derive(Debug, Clone, Default)]
pub enum SemanticScope {
    /// Single-file analysis with no cross-file resolution.
    ///
    /// - Fast startup, no upfront indexing
    /// - Only finds references within the same file
    /// - Best for quick dry runs or high-level analysis
    #[default]
    FileScope,

    /// Workspace-wide analysis with full cross-file support.
    ///
    /// - Full workspace analysis when semantic queries are made
    /// - Indexes files lazily on first query
    /// - Accurate cross-file references
    /// - Higher memory usage, slower initial queries
    WorkspaceScope {
        /// Root directory of the workspace for resolving modules.
        root: PathBuf,
    },
}

/// Configuration for creating semantic providers.
#[derive(Debug, Clone, Default)]
pub struct SemanticConfig {
    /// The scope of semantic analysis.
    pub scope: SemanticScope,
}

impl SemanticConfig {
    /// Create a file-scope configuration (single-file analysis).
    pub fn file_scope() -> Self {
        Self {
            scope: SemanticScope::FileScope,
        }
    }

    /// Create a workspace-scope configuration (workspace-wide analysis).
    pub fn workspace_scope(root: PathBuf) -> Self {
        Self {
            scope: SemanticScope::WorkspaceScope { root },
        }
    }
}
