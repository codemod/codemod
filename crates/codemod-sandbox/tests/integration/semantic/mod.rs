//! Integration tests for semantic analysis providers.
//!
//! One test per fixture (a small source tree under `fixtures/`). Its codemod
//! registers named cases, one rule each, with `fixtures/codemods/spec.js`;
//! the test runs them all and fails with every failed case by name. See
//! `fixtures/mod.rs` for the `jssg_test!` forms.
//!
//! - `javascript_tests`, `typescript_tests`, `python_tests`: by language
//! - `common_tests`: no provider configured

#[macro_use]
pub mod fixtures;

mod common_tests;
mod javascript_tests;
mod python_tests;
mod typescript_tests;
