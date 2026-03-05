//! Internal defaults for AI memory management.

/// Max compaction attempts before failing with explicit memory exhaustion.
pub const MAX_COMPACTION_ATTEMPTS: usize = 5;

/// Soft input token budget used for token-aware proactive compaction checks.
pub const SOFT_CONTEXT_TOKEN_BUDGET: u64 = 96_000;

/// Target input token budget after compaction/rebuild.
pub const TARGET_CONTEXT_TOKEN_BUDGET: u64 = 72_000;

/// Soft input budget used for proactive memory compaction checks.
pub const SOFT_CONTEXT_CHAR_BUDGET: usize = 120_000;

/// Target budget after compaction/rebuild.
pub const TARGET_CONTEXT_CHAR_BUDGET: usize = 80_000;

/// Recent raw messages to preserve without summarizing (attempt 0).
pub const RECENT_MESSAGE_WINDOW: usize = 16;

/// Maximum chars to keep per archived snippet during compaction archiving.
pub const MAX_SNIPPET_CHARS_PER_DOC: usize = 1_200;

/// Number of dynamic context documents injected per prompt when semantic retrieval is available.
pub const DYNAMIC_CONTEXT_SAMPLE_DOCS: usize = 6;

/// Upper bound for indexed semantic memory documents to limit embedding cost/latency.
pub const MAX_VECTOR_INDEX_DOCS: usize = 256;

/// Maximum chars per summarization chunk.
pub const SUMMARY_CHUNK_CHARS: usize = 6_000;

/// Hard cap for the final hierarchical summary text.
pub const FINAL_SUMMARY_CHAR_LIMIT: usize = 14_000;
