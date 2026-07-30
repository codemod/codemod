use anyhow::{Context, Result};
use butterflow_core::llm_usage::{LlmRequestOutcome, LlmUsageRecord, TokenUsage};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Serialize)]
struct LlmUsageOutput {
    schema_version: u8,
    requests: Vec<CanonicalUsageRecord>,
}

#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct CanonicalUsageRecord {
    provider: String,
    model: String,
    outcome: LlmRequestOutcome,
    usage_available: bool,
    usage: Option<CanonicalTokenUsage>,
    request_count: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct CanonicalTokenUsage {
    uncached_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl From<&TokenUsage> for CanonicalTokenUsage {
    fn from(usage: &TokenUsage) -> Self {
        Self {
            uncached_input_tokens: usage.uncached_input_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_write_input_tokens: usage.cache_write_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

fn canonical_records(records: &[LlmUsageRecord]) -> Vec<CanonicalUsageRecord> {
    let mut grouped = BTreeMap::<
        (
            String,
            String,
            LlmRequestOutcome,
            Option<CanonicalTokenUsage>,
        ),
        u64,
    >::new();
    for record in records {
        let usage = record.usage.as_ref().map(CanonicalTokenUsage::from);
        *grouped
            .entry((
                record.provider.clone(),
                record.model.clone(),
                record.outcome,
                usage,
            ))
            .or_default() += 1;
    }

    grouped
        .into_iter()
        .map(
            |((provider, model, outcome, usage), request_count)| CanonicalUsageRecord {
                provider,
                model,
                outcome,
                usage_available: usage.is_some(),
                usage,
                request_count,
            },
        )
        .collect()
}

pub(crate) fn write_llm_usage_output(path: &Path, records: &[LlmUsageRecord]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create LLM usage directory {}", parent.display()))?;

    let output = LlmUsageOutput {
        schema_version: 1,
        requests: canonical_records(records),
    };
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create LLM usage file in {}", parent.display()))?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), &output)
        .with_context(|| format!("Failed to serialize LLM usage for {}", path.display()))?;
    writeln!(temporary.as_file_mut())
        .with_context(|| format!("Failed to finalize LLM usage for {}", path.display()))?;
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("Failed to flush LLM usage for {}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to persist LLM usage to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(
        uncached_input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_write_input_tokens: Option<u64>,
        output_tokens: u64,
        reasoning_tokens: Option<u64>,
        total_tokens: u64,
    ) -> TokenUsage {
        TokenUsage {
            uncached_input_tokens: Some(uncached_input_tokens),
            cache_read_input_tokens,
            cache_write_input_tokens,
            output_tokens: Some(output_tokens),
            reasoning_tokens,
            total_tokens: Some(total_tokens),
        }
    }

    #[test]
    fn writes_grouped_provider_reported_usage_without_sensitive_request_data() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let path = directory.path().join("nested/llm-usage.json");
        let openai = LlmUsageRecord {
            provider: "openai".to_string(),
            model: "gpt-test".to_string(),
            outcome: LlmRequestOutcome::Success,
            usage: Some(usage(60, Some(40), None, 20, Some(5), 120)),
        };
        let records = vec![
            openai.clone(),
            openai,
            LlmUsageRecord {
                provider: "anthropic".to_string(),
                model: "claude-test".to_string(),
                outcome: LlmRequestOutcome::Success,
                usage: Some(usage(10, Some(4), Some(2), 3, None, 19)),
            },
            LlmUsageRecord {
                provider: "openai".to_string(),
                model: "gpt-test".to_string(),
                outcome: LlmRequestOutcome::Error,
                usage: None,
            },
        ];

        write_llm_usage_output(&path, &records).expect("usage output should write");
        let actual: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("usage output should be readable"))
                .expect("usage output should be JSON");

        assert_eq!(
            actual,
            json!({
                "schema_version": 1,
                "requests": [
                    {
                        "provider": "anthropic",
                        "model": "claude-test",
                        "outcome": "success",
                        "usage_available": true,
                        "usage": {
                            "uncached_input_tokens": 10,
                            "cache_read_input_tokens": 4,
                            "cache_write_input_tokens": 2,
                            "output_tokens": 3,
                            "reasoning_tokens": null,
                            "total_tokens": 19
                        },
                        "request_count": 1
                    },
                    {
                        "provider": "openai",
                        "model": "gpt-test",
                        "outcome": "success",
                        "usage_available": true,
                        "usage": {
                            "uncached_input_tokens": 60,
                            "cache_read_input_tokens": 40,
                            "cache_write_input_tokens": null,
                            "output_tokens": 20,
                            "reasoning_tokens": 5,
                            "total_tokens": 120
                        },
                        "request_count": 2
                    },
                    {
                        "provider": "openai",
                        "model": "gpt-test",
                        "outcome": "error",
                        "usage_available": false,
                        "usage": null,
                        "request_count": 1
                    }
                ]
            })
        );
        let serialized = std::fs::read_to_string(&path).expect("usage output should be text");
        assert!(!serialized.contains("test-key"));
        assert!(!serialized.contains("Generate an identifier"));
    }

    #[test]
    fn replaces_an_existing_output_with_the_current_run() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let path = directory.path().join("llm-usage.json");
        std::fs::write(&path, "stale data").expect("existing output should be created");

        write_llm_usage_output(&path, &[]).expect("usage output should replace old content");

        let actual: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("usage output should be readable"))
                .expect("usage output should be JSON");
        assert_eq!(actual, json!({ "schema_version": 1, "requests": [] }));
    }
}
