//! Internal memory policy for AI context management.

/// Max compaction attempts before failing with explicit memory exhaustion.
pub const MAX_COMPACTION_ATTEMPTS: usize = 5;

/// Default soft input token budget used for proactive checks when model limits are unknown.
pub const SOFT_CONTEXT_TOKEN_BUDGET: u64 = 96_000;

/// Default target input token budget after compaction/rebuild when model limits are unknown.
pub const TARGET_CONTEXT_TOKEN_BUDGET: u64 = 72_000;

/// Default soft input char budget used for proactive checks when model limits are unknown.
pub const SOFT_CONTEXT_CHAR_BUDGET: usize = 120_000;

/// Default target char budget after compaction/rebuild when model limits are unknown.
pub const TARGET_CONTEXT_CHAR_BUDGET: usize = 80_000;

const CONTEXT_SOFT_PERCENT: u64 = 80;
const CONTEXT_TARGET_PERCENT: u64 = 60;
const TOKEN_TO_CHAR_NUMERATOR: u64 = 5;
const TOKEN_TO_CHAR_DENOMINATOR: u64 = 4;
const MIN_SOFT_CONTEXT_TOKEN_BUDGET: u64 = 8_000;
const MIN_TARGET_CONTEXT_TOKEN_BUDGET: u64 = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPolicy {
    pub max_compaction_attempts: usize,
    pub soft_context_token_budget: u64,
    pub target_context_token_budget: u64,
    pub soft_context_char_budget: usize,
    pub target_context_char_budget: usize,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            max_compaction_attempts: MAX_COMPACTION_ATTEMPTS,
            soft_context_token_budget: SOFT_CONTEXT_TOKEN_BUDGET,
            target_context_token_budget: TARGET_CONTEXT_TOKEN_BUDGET,
            soft_context_char_budget: SOFT_CONTEXT_CHAR_BUDGET,
            target_context_char_budget: TARGET_CONTEXT_CHAR_BUDGET,
        }
    }
}

fn tokens_to_chars(tokens: u64) -> usize {
    ((tokens as u128 * TOKEN_TO_CHAR_NUMERATOR as u128) / TOKEN_TO_CHAR_DENOMINATOR as u128)
        as usize
}

pub fn resolve_memory_policy(context_tokens: Option<u64>) -> MemoryPolicy {
    let Some(context_tokens) = context_tokens else {
        return MemoryPolicy::default();
    };
    if context_tokens <= 1 {
        return MemoryPolicy::default();
    }

    let mut soft = (context_tokens.saturating_mul(CONTEXT_SOFT_PERCENT) / 100)
        .max(MIN_SOFT_CONTEXT_TOKEN_BUDGET);
    if soft >= context_tokens {
        soft = context_tokens.saturating_sub(1).max(1);
    }

    let mut target = (context_tokens.saturating_mul(CONTEXT_TARGET_PERCENT) / 100)
        .max(MIN_TARGET_CONTEXT_TOKEN_BUDGET);
    if target >= soft {
        target = soft.saturating_sub(1).max(1);
    }

    MemoryPolicy {
        max_compaction_attempts: MAX_COMPACTION_ATTEMPTS,
        soft_context_token_budget: soft,
        target_context_token_budget: target,
        soft_context_char_budget: tokens_to_chars(soft),
        target_context_char_budget: tokens_to_chars(target),
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_memory_policy_from_context_limit_uses_dynamic_ratios() {
        let policy = resolve_memory_policy(Some(200_000));
        assert_eq!(policy.soft_context_token_budget, 160_000);
        assert_eq!(policy.target_context_token_budget, 120_000);
        assert!(policy.target_context_char_budget < policy.soft_context_char_budget);
    }

    #[test]
    fn test_resolve_memory_policy_unknown_model_uses_defaults() {
        let policy = resolve_memory_policy(None);
        assert_eq!(policy, MemoryPolicy::default());
    }

    #[test]
    fn test_resolve_memory_policy_keeps_target_lower_than_soft() {
        let policy = resolve_memory_policy(Some(8_100));
        assert!(policy.soft_context_token_budget > policy.target_context_token_budget);
        assert!(policy.soft_context_char_budget > policy.target_context_char_budget);
    }
}
