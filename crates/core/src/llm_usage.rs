pub use codemod_ai::llm::TokenUsage;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmRequestOutcome {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmUsageRecord {
    pub provider: String,
    pub model: String,
    pub outcome: LlmRequestOutcome,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Default)]
pub struct LlmUsageContext {
    records: Arc<Mutex<Vec<LlmUsageRecord>>>,
}

impl std::fmt::Debug for LlmUsageContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LlmUsageContext")
            .field("records", &self.get_all())
            .finish()
    }
}

impl LlmUsageContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_success(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        usage: Option<TokenUsage>,
    ) {
        self.record(LlmUsageRecord {
            provider: provider.into(),
            model: model.into(),
            outcome: LlmRequestOutcome::Success,
            usage,
        });
    }

    pub fn record_error(&self, provider: impl Into<String>, model: impl Into<String>) {
        self.record(LlmUsageRecord {
            provider: provider.into(),
            model: model.into(),
            outcome: LlmRequestOutcome::Error,
            usage: None,
        });
    }

    pub fn get_all(&self) -> Vec<LlmUsageRecord> {
        self.records
            .lock()
            .map(|records| records.clone())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.records
            .lock()
            .map(|records| records.is_empty())
            .unwrap_or(true)
    }

    fn record(&self, record: LlmUsageRecord) {
        if let Ok(mut records) = self.records.lock() {
            records.push(record);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_shares_usage_records() {
        let context = LlmUsageContext::new();
        let clone = context.clone();

        clone.record_success(
            "anthropic",
            "claude-test",
            Some(TokenUsage {
                uncached_input_tokens: Some(10),
                cache_read_input_tokens: Some(4),
                cache_write_input_tokens: Some(2),
                output_tokens: Some(3),
                reasoning_tokens: None,
                total_tokens: Some(19),
            }),
        );
        context.record_error("openai", "gpt-test");

        let records = context.get_all();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].provider, "anthropic");
        assert_eq!(records[1].outcome, LlmRequestOutcome::Error);
    }
}
