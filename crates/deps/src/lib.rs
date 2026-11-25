//! One dependency graph across lockfile formats.
//!
//! Every supported lockfile (Yarn 1 and Berry, pnpm, package-lock.json and
//! npm-shrinkwrap.json, bun.lock, Cargo.lock, uv.lock, poetry.lock,
//! Pipfile.lock) maps into one [`Graph`]: nodes are `name@version` per
//! ecosystem, edges keep the declared range, the dependency kind and the
//! workspace. [`Walk`] reads it; [`Registry`] supplies release metadata.
//!
//! Without a lockfile, [`project`] reads the repository's own packages from
//! their manifests (package.json, catalogs, the package manager), and
//! [`manifest_edits`] changes them in place, keeping their formatting.

pub mod edit;
pub mod lockfiles;
pub mod manifest_edits;
pub mod model;
pub mod project;
pub mod registry;
pub mod semver;
pub mod walk;

/// Version of the parsers' output: part of every parse cache key, so a
/// cached parse is not reused across parser changes.
pub const PARSER_VERSION: u32 = 1;

pub use lockfiles::load;
pub use model::{Ecosystem, Graph, Kind};
pub use registry::{HttpRegistry, Registry, Release};
pub use walk::Walk;
