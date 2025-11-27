//! Integration tests for semantic analysis providers.
//!
//! Tests are organized by language:
//! - `javascript_tests`: JavaScript semantic analysis
//! - `typescript_tests`: TypeScript semantic analysis  
//! - `python_tests`: Python semantic analysis
//! - `common_tests`: Tests that apply to all languages (e.g., no provider configured)

mod common_tests;
mod javascript_tests;
mod python_tests;
mod typescript_tests;
