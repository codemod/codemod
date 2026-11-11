//! TypeScript workflows for `codemod workflow run -w`.
//!
//! - [`build`] turns `workflow.ts` into one ES module bundle: every inline
//!   `jssg()` transform becomes a content-hashed artifact, and the workflow
//!   is bundled with the orchestration runtime (`packages/orchestration`).
//! - [`host`] runs that bundle in one QuickJS context inside the codemod
//!   process, with the file system, process, HTTP and JSSG natives it needs.
//! - [`walker`] is the native file selection the host exposes.
//! - [`jssg_batch`] holds the JSSG batch rules (planning, path containment,
//!   selector skip, result conversion) the host applies per file.
//!
//! Agent steps run in their own process (`orchestration-agent`).

pub mod build;
pub mod host;
pub mod jssg_batch;
pub mod walker;
