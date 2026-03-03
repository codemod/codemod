//! Internal memory management for Rig-based AI execution.

pub mod compact;
pub mod controller;
pub mod history;
pub mod policy;
pub mod semantic;
pub mod summarize;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Memory summarization failed: {0}")]
    Summarization(String),
    #[error("Memory compaction failed: {0}")]
    Compaction(String),
}

pub type Result<T> = std::result::Result<T, MemoryError>;
