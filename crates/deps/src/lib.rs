//! One dependency graph across lockfile formats.
//!
//! Every supported lockfile (Yarn Berry, pnpm, package-lock.json,
//! Cargo.lock) maps into one [`Graph`]: nodes are `name@version` per
//! ecosystem, edges keep the declared range, the dependency kind and the
//! workspace. [`Walk`] reads it; [`Registry`] supplies release metadata.

pub mod lockfiles;
pub mod model;
pub mod registry;
pub mod semver;
pub mod walk;

pub use lockfiles::load;
pub use model::{Ecosystem, Graph, Kind};
pub use registry::{HttpRegistry, Registry, Release};
pub use walk::Walk;
