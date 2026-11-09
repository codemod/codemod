//! Builds a TypeScript orchestration workflow into one self-contained ES
//! module for the QuickJS host (`orchestration-quickjs-host`):
//!
//! - [`build_module`] splits a workflow module: every inline `jssg()`
//!   transform becomes its own bundled artifact, identified by content hash,
//!   and the call is rewritten to reference it.
//! - [`build_quickjs_bundle`] bundles the workflow graph with the portable
//!   orchestration runtime, async lowered, with the artifact table in front.
//! - [`cached_quickjs_bundle`] reuses a bundle until an input file or this
//!   builder changes.
mod bundle;
mod cache;
mod split;

use std::{fmt, path::PathBuf};

pub use bundle::{build_quickjs_bundle, QuickjsBundle, Runtime};
pub use cache::{cached_quickjs_bundle, default_bundle_cache, private_directory, CachedBundle};
pub use split::{build_file, build_module, BuiltModule, JssgArtifact};

/// A workflow outside the supported subset, or a bundling failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    pub file: PathBuf,
    /// 1-based line and column (UTF-16 code units, as editors count them).
    pub position: Option<(usize, usize)>,
    pub message: String,
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some((line, column)) => {
                write!(
                    f,
                    "{}:{line}:{column}: {}",
                    self.file.display(),
                    self.message
                )
            }
            None => write!(f, "{}: {}", self.file.display(), self.message),
        }
    }
}

impl std::error::Error for BuildError {}

pub(crate) fn sha256(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            hash.update(b"\0");
        }
        hash.update(part);
    }
    hex::encode(hash.finalize())
}
