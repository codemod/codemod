//! Configuration types for semantic analysis.

use std::path::PathBuf;
use vfs::VfsPath;

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

/// Strategy for enumerating the workspace when building the cross-file
/// symbol index. Only relevant for [`SemanticScope::WorkspaceScope`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceWalker {
    /// Use `ignore::WalkBuilder` against the real filesystem; honors
    /// `.gitignore`. Required when the backing storage is a real disk.
    #[default]
    Ignore,
    /// Recursively read the configured VFS via `VfsPath::read_dir`. Use
    /// when the backing storage is virtual (e.g. a MemoryFS seeded from a
    /// database manifest) — the `ignore` crate can't see these entries.
    Vfs,
}

/// Configuration for creating semantic providers.
#[derive(Clone, Default)]
pub struct SemanticConfig {
    /// The scope of semantic analysis.
    pub scope: SemanticScope,
    /// Optional virtual filesystem root for file operations.
    /// If None, the provider will use the real filesystem (PhysicalFS).
    pub fs_root: Option<VfsPath>,
    /// How to enumerate workspace files during indexing.
    pub walker: WorkspaceWalker,
}

impl std::fmt::Debug for SemanticConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticConfig")
            .field("scope", &self.scope)
            .field("fs_root", &self.fs_root.as_ref().map(|_| "<VfsPath>"))
            .field("walker", &self.walker)
            .finish()
    }
}

impl SemanticConfig {
    /// Create a file-scope configuration (single-file analysis).
    pub fn file_scope() -> Self {
        Self {
            scope: SemanticScope::FileScope,
            fs_root: None,
            walker: WorkspaceWalker::Ignore,
        }
    }

    /// Create a file-scope configuration with a custom virtual filesystem.
    pub fn file_scope_with_fs(fs_root: VfsPath) -> Self {
        Self {
            scope: SemanticScope::FileScope,
            fs_root: Some(fs_root),
            walker: WorkspaceWalker::Ignore,
        }
    }

    /// Create a workspace-scope configuration (workspace-wide analysis).
    pub fn workspace_scope(root: PathBuf) -> Self {
        Self {
            scope: SemanticScope::WorkspaceScope { root },
            fs_root: None,
            walker: WorkspaceWalker::Ignore,
        }
    }

    /// Create a workspace-scope configuration with a custom virtual filesystem.
    pub fn workspace_scope_with_fs(root: PathBuf, fs_root: VfsPath) -> Self {
        Self {
            scope: SemanticScope::WorkspaceScope { root },
            fs_root: Some(fs_root),
            walker: WorkspaceWalker::Ignore,
        }
    }

    /// Override the workspace walker used during indexing. No-op for
    /// file-scope configurations since they don't walk.
    pub fn with_walker(mut self, walker: WorkspaceWalker) -> Self {
        self.walker = walker;
        self
    }
}
