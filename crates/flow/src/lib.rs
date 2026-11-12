//! Spike: `step()` and `forEachFile` for TypeScript workflows.
//!
//! See `README.md` for the design and what this spike leaves out.

pub mod build;
mod files;
pub mod run;

pub use build::{build, BuildError, Built, FLOW_MODULE};
pub use run::{run, RunError, RunOptions, RunOutcome};
