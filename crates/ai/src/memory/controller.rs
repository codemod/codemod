//! Memory compaction orchestration and retry helpers.

use rig::completion::Message;

use crate::memory::compact::{
    build_memory_packet, deterministic_prune, rebuild_history_with_memory, PruneResult,
    RebuildStats,
};
use crate::memory::history::{estimate_context_chars, HistoryDocument};
use crate::memory::semantic::SemanticDocument;
use crate::memory::summarize::hierarchical_summarize;
use crate::memory::{MemoryError, Result};

pub const MEMORY_PROACTIVE_REASON_PREFIX: &str = "__memory_compaction_proactive__:";
pub(crate) const MAX_COMPACTION_ATTEMPTS: usize = 5;
pub(crate) const SOFT_CONTEXT_TOKEN_BUDGET: u64 = 96_000;
pub(crate) const TARGET_CONTEXT_TOKEN_BUDGET: u64 = 72_000;
pub(crate) const SOFT_CONTEXT_CHAR_BUDGET: usize = 120_000;
const TARGET_CONTEXT_CHAR_BUDGET: usize = 80_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryTrigger {
    Proactive,
    ReactiveProviderError,
}

#[derive(Debug, Clone)]
pub struct CompactionStats {
    pub attempt: usize,
    pub trigger: MemoryTrigger,
    pub before_chars: usize,
    pub after_chars: usize,
    pub archived_docs: usize,
    pub retrieved_docs: usize,
}

#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub history: Vec<Message>,
    pub stats: CompactionStats,
    pub retrieval_docs: Vec<SemanticDocument>,
}

#[derive(Debug, Clone)]
pub struct TokenUsageSnapshot {
    pub total_tokens: u64,
    pub context_chars: usize,
}

pub fn proactive_cancel_reason(chars: usize, estimated_tokens: Option<u64>) -> String {
    match estimated_tokens {
        Some(tokens) => format!("{MEMORY_PROACTIVE_REASON_PREFIX}chars={chars};tokens={tokens}"),
        None => format!("{MEMORY_PROACTIVE_REASON_PREFIX}chars={chars}"),
    }
}

pub fn is_proactive_cancel_reason(reason: &str) -> bool {
    reason.starts_with(MEMORY_PROACTIVE_REASON_PREFIX)
}

pub fn maybe_proactive_budget(
    prompt: &Message,
    history: &[Message],
    usage_snapshot: Option<&TokenUsageSnapshot>,
) -> Option<(usize, Option<u64>)> {
    let chars = estimate_context_chars(prompt, history);

    if let Some(snapshot) = usage_snapshot {
        if snapshot.context_chars > 0 && snapshot.total_tokens > 0 {
            let estimated_tokens = ((chars as u128 * snapshot.total_tokens as u128)
                / snapshot.context_chars as u128) as u64;

            if estimated_tokens > SOFT_CONTEXT_TOKEN_BUDGET {
                return Some((chars, Some(estimated_tokens)));
            }
        }
    }

    if chars > SOFT_CONTEXT_CHAR_BUDGET {
        return Some((chars, None));
    }

    None
}

pub fn is_context_limit_error_text(message: &str) -> bool {
    fn has_context_keywords(text: &str) -> bool {
        let normalized = text.to_ascii_lowercase();
        let patterns = [
            "context length",
            "maximum context",
            "context window",
            "too many tokens",
            "token limit",
            "request too large",
            "input is too long",
            "prompt is too long",
            "context_length_exceeded",
        ];
        patterns.iter().any(|pattern| normalized.contains(pattern))
    }

    fn parse_error_json_candidates(message: &str) -> Vec<serde_json::Value> {
        let mut parsed = Vec::new();

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(message) {
            parsed.push(value);
        }

        if let Some(start) = message.find('{') {
            if let Some(end) = message.rfind('}') {
                if end > start {
                    let raw = &message[start..=end];
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
                        parsed.push(value);
                    }
                }
            }
        }

        parsed
    }

    fn error_field(value: &serde_json::Value) -> &serde_json::Value {
        value.get("error").unwrap_or(value)
    }

    fn field_str<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
        value.get(key).and_then(serde_json::Value::as_str)
    }

    for candidate in parse_error_json_candidates(message) {
        let error = error_field(&candidate);
        let code = field_str(error, "code")
            .or_else(|| field_str(&candidate, "code"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if code == "context_length_exceeded" {
            return true;
        }

        let error_type = field_str(error, "type")
            .or_else(|| field_str(&candidate, "type"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        let status = field_str(error, "status")
            .or_else(|| field_str(&candidate, "status"))
            .unwrap_or_default()
            .to_ascii_uppercase();
        let detail_message = field_str(error, "message")
            .or_else(|| field_str(&candidate, "message"))
            .unwrap_or_default();

        // Anthropic and similar providers use `invalid_request_error` for context/token overflow.
        if error_type == "invalid_request_error" && has_context_keywords(detail_message) {
            return true;
        }

        // Gemini often emits INVALID_ARGUMENT when token/context constraints are exceeded.
        if status == "INVALID_ARGUMENT" && has_context_keywords(detail_message) {
            return true;
        }

        if has_context_keywords(detail_message) {
            return true;
        }
    }

    has_context_keywords(message)
}

fn build_retrieval_docs(archived_docs: &[HistoryDocument], summary: &str) -> Vec<SemanticDocument> {
    let mut docs = archived_docs
        .iter()
        .map(|doc| SemanticDocument {
            id: doc.id.clone(),
            text: doc.text.clone(),
        })
        .collect::<Vec<_>>();

    if !summary.trim().is_empty() {
        docs.push(SemanticDocument {
            id: "memory-summary".to_string(),
            text: summary.to_string(),
        });
    }

    docs
}

fn aggressive_trim_to_target(
    mut history: Vec<Message>,
    prompt: &Message,
    attempt: usize,
) -> Vec<Message> {
    if estimate_context_chars(prompt, &history) <= TARGET_CONTEXT_CHAR_BUDGET {
        return history;
    }

    let keep_recent = 8usize.saturating_sub(attempt).max(2);
    if history.len() > keep_recent + 1 {
        let mut trimmed = Vec::new();
        trimmed.push(history[0].clone());
        trimmed.extend_from_slice(&history[history.len().saturating_sub(keep_recent)..]);
        history = trimmed;
    }

    while estimate_context_chars(prompt, &history) > TARGET_CONTEXT_CHAR_BUDGET && history.len() > 2
    {
        let remove_index = if history.len() > 3 { 2 } else { 1 };
        history.remove(remove_index);
    }

    history
}

fn merge_stats(
    attempt: usize,
    trigger: MemoryTrigger,
    prune: &PruneResult,
    rebuild: &RebuildStats,
) -> CompactionStats {
    CompactionStats {
        attempt,
        trigger,
        before_chars: prune.context_chars_before,
        after_chars: rebuild.rebuilt_context_chars,
        archived_docs: prune.archived_documents.len(),
        retrieved_docs: rebuild.retrieved_docs_count,
    }
}

pub async fn compact_history_for_retry<C>(
    client: &C,
    model: &str,
    task_prompt: &str,
    current_prompt: &Message,
    history: &[Message],
    attempt: usize,
    trigger: MemoryTrigger,
) -> Result<CompactionResult>
where
    C: rig::client::CompletionClient,
{
    if attempt >= MAX_COMPACTION_ATTEMPTS {
        return Err(MemoryError::Compaction(format!(
            "Reached max compaction attempts ({MAX_COMPACTION_ATTEMPTS})"
        )));
    }

    let prune = deterministic_prune(history, current_prompt, attempt);
    if prune.archived_documents.is_empty() {
        let trimmed =
            aggressive_trim_to_target(prune.retained_history.clone(), current_prompt, attempt);
        let stats = CompactionStats {
            attempt,
            trigger,
            before_chars: prune.context_chars_before,
            after_chars: estimate_context_chars(current_prompt, &trimmed),
            archived_docs: 0,
            retrieved_docs: 0,
        };
        return Ok(CompactionResult {
            history: trimmed,
            stats,
            retrieval_docs: Vec::new(),
        });
    }

    let summary =
        hierarchical_summarize(client, model, &prune.archived_documents, task_prompt).await?;
    let retrieval_docs = build_retrieval_docs(&prune.archived_documents, &summary);
    let packet = Some(build_memory_packet(&summary, &[]));

    let (mut rebuilt, mut rebuild_stats) =
        rebuild_history_with_memory(&prune, packet, current_prompt);
    rebuilt = aggressive_trim_to_target(rebuilt, current_prompt, attempt);
    rebuild_stats.retrieved_docs_count = retrieval_docs.len();
    rebuild_stats.rebuilt_context_chars = estimate_context_chars(current_prompt, &rebuilt);

    let estimated_tokens_after = if prune.context_chars_before > 0 {
        (rebuild_stats.rebuilt_context_chars as u128 * TARGET_CONTEXT_TOKEN_BUDGET as u128
            / prune.context_chars_before as u128) as u64
    } else {
        0
    };
    if estimated_tokens_after > TARGET_CONTEXT_TOKEN_BUDGET {
        rebuilt = aggressive_trim_to_target(rebuilt, current_prompt, attempt.saturating_add(1));
        rebuild_stats.rebuilt_context_chars = estimate_context_chars(current_prompt, &rebuilt);
    }

    Ok(CompactionResult {
        history: rebuilt,
        stats: merge_stats(attempt, trigger, &prune, &rebuild_stats),
        retrieval_docs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_limit_error_detection() {
        assert!(is_context_limit_error_text(
            "maximum context length is 128000 tokens"
        ));
        assert!(is_context_limit_error_text(
            "Request too large for this model"
        ));
        assert!(!is_context_limit_error_text("network timeout"));
    }

    #[test]
    fn test_context_limit_error_detection_openai_json() {
        let payload = r#"{
          "error": {
            "message": "This model's maximum context length is 128000 tokens.",
            "type": "invalid_request_error",
            "code": "context_length_exceeded"
          }
        }"#;
        assert!(is_context_limit_error_text(payload));
    }

    #[test]
    fn test_context_limit_error_detection_anthropic_json() {
        let payload = r#"{
          "type": "error",
          "error": {
            "type": "invalid_request_error",
            "message": "prompt is too long: 210000 tokens > max 200000"
          }
        }"#;
        assert!(is_context_limit_error_text(payload));
    }

    #[test]
    fn test_context_limit_error_detection_gemini_json() {
        let payload = r#"{
          "error": {
            "code": 400,
            "message": "Input is too long for model context window",
            "status": "INVALID_ARGUMENT"
          }
        }"#;
        assert!(is_context_limit_error_text(payload));
    }

    #[test]
    fn test_context_limit_error_detection_embedded_json() {
        let payload = r#"ProviderError: {"error":{"code":"context_length_exceeded","message":"max context"}} "#;
        assert!(is_context_limit_error_text(payload));
    }

    #[test]
    fn test_context_limit_error_detection_non_context_json() {
        let payload = r#"{"error":{"code":"rate_limit_exceeded","message":"Too many requests"}}"#;
        assert!(!is_context_limit_error_text(payload));
    }

    #[test]
    fn test_proactive_reason_roundtrip() {
        let reason = proactive_cancel_reason(12_345, Some(9_876));
        assert!(is_proactive_cancel_reason(&reason));
        assert!(!is_proactive_cancel_reason("other"));
    }

    #[test]
    fn test_maybe_proactive_budget_threshold() {
        let prompt = Message::user("x".repeat(SOFT_CONTEXT_CHAR_BUDGET + 10));
        assert!(maybe_proactive_budget(&prompt, &[], None).is_some());
    }

    #[test]
    fn test_maybe_proactive_budget_uses_usage_snapshot() {
        let prompt = Message::user("x".repeat(20_000));
        let snapshot = TokenUsageSnapshot {
            total_tokens: 100_000,
            context_chars: 20_000,
        };
        let signal = maybe_proactive_budget(&prompt, &[], Some(&snapshot));
        assert!(signal.is_some());
    }
}
