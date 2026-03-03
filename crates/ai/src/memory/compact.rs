//! Deterministic pruning and memory packet assembly.

use std::collections::HashSet;

use rig::completion::Message;

use crate::memory::history::{
    clip_chars, estimate_context_chars, extract_history_documents, HistoryDocument,
};
use crate::memory::policy::{
    MAX_SNIPPET_CHARS_PER_DOC, RECENT_MESSAGE_WINDOW, SOFT_CONTEXT_CHAR_BUDGET,
};

#[derive(Debug, Clone)]
pub struct PruneResult {
    pub retained_history: Vec<Message>,
    pub archived_documents: Vec<HistoryDocument>,
    pub context_chars_before: usize,
}

#[derive(Debug, Clone)]
pub struct RebuildStats {
    pub retrieved_docs_count: usize,
    pub rebuilt_context_chars: usize,
}

fn retention_indices(len: usize, attempt: usize) -> HashSet<usize> {
    if len == 0 {
        return HashSet::new();
    }

    let dynamic_window = RECENT_MESSAGE_WINDOW
        .saturating_sub(attempt.saturating_mul(2))
        .max(4);
    let recent_start = len.saturating_sub(dynamic_window);

    let mut indices = HashSet::new();
    indices.insert(0); // initial anchor
    for idx in recent_start..len {
        indices.insert(idx);
    }
    indices
}

pub fn deterministic_prune(history: &[Message], prompt: &Message, attempt: usize) -> PruneResult {
    if history.is_empty() {
        return PruneResult {
            retained_history: Vec::new(),
            archived_documents: Vec::new(),
            context_chars_before: estimate_context_chars(prompt, history),
        };
    }

    let context_before = estimate_context_chars(prompt, history);

    let retain = retention_indices(history.len(), attempt);
    let mut retained_history = Vec::new();
    let mut archived_messages = Vec::new();

    for (idx, message) in history.iter().enumerate() {
        if retain.contains(&idx) {
            retained_history.push(message.clone());
        } else {
            archived_messages.push(message.clone());
        }
    }

    let archived_documents =
        extract_history_documents(&archived_messages, MAX_SNIPPET_CHARS_PER_DOC);
    PruneResult {
        retained_history,
        archived_documents,
        context_chars_before: context_before,
    }
}

pub fn build_memory_packet(summary: &str, retrieved_snippets: &[String]) -> Message {
    let mut packet = String::from(
        "[Memory Packet]\nThe following context was compacted from earlier turns to preserve tool execution continuity.\n",
    );

    if !summary.trim().is_empty() {
        packet.push_str("\n[Summary]\n");
        packet.push_str(summary.trim());
        packet.push('\n');
    }

    if !retrieved_snippets.is_empty() {
        packet.push_str("\n[Retrieved Snippets]\n");
        for snippet in retrieved_snippets {
            packet.push_str("- ");
            packet.push_str(snippet.trim());
            packet.push('\n');
        }
    }

    // Final clip guard to avoid packet itself exploding context.
    Message::user(clip_chars(&packet, SOFT_CONTEXT_CHAR_BUDGET / 3))
}

pub fn rebuild_history_with_memory(
    prune: &PruneResult,
    memory_packet: Option<Message>,
    prompt: &Message,
) -> (Vec<Message>, RebuildStats) {
    if prune.archived_documents.is_empty() || memory_packet.is_none() {
        let rebuilt = prune.retained_history.clone();
        return (
            rebuilt.clone(),
            RebuildStats {
                retrieved_docs_count: 0,
                rebuilt_context_chars: estimate_context_chars(prompt, &rebuilt),
            },
        );
    }

    let packet = memory_packet.expect("checked above");
    let mut rebuilt = Vec::new();

    if let Some((first, rest)) = prune.retained_history.split_first() {
        rebuilt.push(first.clone());
        rebuilt.push(packet);
        rebuilt.extend_from_slice(rest);
    } else {
        rebuilt.push(packet);
    }

    let rebuilt_chars = estimate_context_chars(prompt, &rebuilt);
    (
        rebuilt,
        RebuildStats {
            retrieved_docs_count: 0,
            rebuilt_context_chars: rebuilt_chars,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::history::extract_message_text;

    #[test]
    fn test_deterministic_prune_keeps_anchor_and_recent() {
        let history = (0..25)
            .map(|idx| Message::user(format!("message-{idx}")))
            .collect::<Vec<_>>();
        let prompt = Message::user("do task");

        let result = deterministic_prune(&history, &prompt, 0);

        assert!(!result.retained_history.is_empty());
        assert!(extract_message_text(&result.retained_history[0]).contains("message-0"));
        assert!(result.retained_history.len() <= RECENT_MESSAGE_WINDOW + 1);
        assert!(!result.archived_documents.is_empty());
    }

    #[test]
    fn test_rebuild_history_inserts_memory_packet_after_anchor() {
        let history = vec![
            Message::user("initial"),
            Message::assistant("a"),
            Message::assistant("b"),
            Message::assistant("c"),
            Message::assistant("d"),
            Message::assistant("e"),
            Message::assistant("f"),
            Message::assistant("g"),
            Message::assistant("h"),
            Message::assistant("i"),
            Message::assistant("j"),
            Message::assistant("k"),
            Message::assistant("l"),
            Message::assistant("m"),
            Message::assistant("n"),
            Message::assistant("o"),
            Message::assistant("p"),
            Message::assistant("q"),
            Message::assistant("r"),
            Message::assistant("s"),
        ];
        let prompt = Message::user("task");
        let prune = deterministic_prune(&history, &prompt, 0);
        let packet = build_memory_packet("summary", &["snippet".to_string()]);

        let (rebuilt, _) = rebuild_history_with_memory(&prune, Some(packet), &prompt);
        assert!(rebuilt.len() >= 2);
        assert!(extract_message_text(&rebuilt[0]).contains("initial"));
        assert!(extract_message_text(&rebuilt[1]).contains("[Memory Packet]"));
    }
}
