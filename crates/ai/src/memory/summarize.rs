//! Hierarchical summarization over archived context chunks.

use rig::completion::Prompt;

use crate::memory::history::{clip_chars, HistoryDocument};
use crate::memory::policy::{FINAL_SUMMARY_CHAR_LIMIT, SUMMARY_CHUNK_CHARS};
use crate::memory::{MemoryError, Result};

const SUMMARIZER_PREAMBLE: &str = "You summarize prior AI tool-execution context. Preserve facts, file paths, commands, failures, and decisions. Keep output concise and structured.";

fn chunk_documents(docs: &[String], max_chunk_chars: usize) -> Vec<String> {
    if docs.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for doc in docs {
        let candidate_len = current.chars().count() + doc.chars().count() + 2;
        if !current.is_empty() && candidate_len > max_chunk_chars {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(doc);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

async fn summarize_chunk<C>(
    client: &C,
    model: &str,
    query_focus: &str,
    chunk: &str,
) -> Result<String>
where
    C: rig::client::CompletionClient,
{
    let prompt = format!(
        "Summarize this archived execution context.\n\
         Focus: {}\n\
         Output requirements:\n\
         - Keep concrete facts only.\n\
         - Include file paths, commands, and errors if present.\n\
         - Keep under 350 words.\n\n\
         Context:\n{}",
        query_focus, chunk
    );

    let response = client
        .agent(model.to_string())
        .temperature(0.1)
        .preamble(SUMMARIZER_PREAMBLE)
        .build()
        .prompt(prompt)
        .extended_details()
        .await
        .map_err(|e| MemoryError::Summarization(e.to_string()))?;

    tracing::debug!(
        "Memory summarize chunk usage: input_tokens={}, output_tokens={}, total_tokens={}",
        response.total_usage.input_tokens,
        response.total_usage.output_tokens,
        response.total_usage.total_tokens
    );

    Ok(clip_chars(&response.output, FINAL_SUMMARY_CHAR_LIMIT / 2))
}

pub async fn hierarchical_summarize<C>(
    client: &C,
    model: &str,
    archived_docs: &[HistoryDocument],
    query_focus: &str,
) -> Result<String>
where
    C: rig::client::CompletionClient,
{
    if archived_docs.is_empty() {
        return Ok(String::new());
    }

    let mut level = archived_docs
        .iter()
        .map(|doc| {
            format!(
                "Doc {} (idx={} role={:?} tool_result={}):\n{}",
                doc.id, doc.index, doc.role, doc.is_tool_result, doc.text
            )
        })
        .collect::<Vec<_>>();

    let mut passes = 0usize;
    while level.len() > 1
        || level.first().map(|t| t.chars().count()).unwrap_or(0) > FINAL_SUMMARY_CHAR_LIMIT
    {
        passes += 1;
        if passes > 4 {
            break;
        }

        let chunks = chunk_documents(&level, SUMMARY_CHUNK_CHARS);
        let mut next = Vec::new();
        for chunk in chunks {
            next.push(summarize_chunk(client, model, query_focus, &chunk).await?);
        }
        level = next;
    }

    let summary = level.join("\n\n");
    Ok(clip_chars(&summary, FINAL_SUMMARY_CHAR_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_documents_respects_boundaries() {
        let docs = vec![
            "a".repeat(20),
            "b".repeat(20),
            "c".repeat(20),
            "d".repeat(20),
        ];
        let chunks = chunk_documents(&docs, 45);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 50));
    }
}
