//! Deterministic pruning and memory packet assembly.

use std::collections::HashSet;

use rig::completion::message::UserContent;
use rig::completion::{AssistantContent, Message};

use crate::memory::history::{
    clip_chars, estimate_context_chars, extract_history_documents, extract_message_text,
    HistoryDocument,
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

fn assistant_tool_call_ids(message: &Message) -> HashSet<String> {
    match message {
        Message::Assistant { content, .. } => content
            .iter()
            .filter_map(|item| {
                if let AssistantContent::ToolCall(tool_call) = item {
                    Some(tool_call.id.clone())
                } else {
                    None
                }
            })
            .collect(),
        _ => HashSet::new(),
    }
}

fn user_tool_result_ids(message: &Message) -> HashSet<String> {
    match message {
        Message::User { content } => content
            .iter()
            .filter_map(|item| {
                if let UserContent::ToolResult(tool_result) = item {
                    Some(tool_result.id.clone())
                } else {
                    None
                }
            })
            .collect(),
        _ => HashSet::new(),
    }
}

fn retain_required_tool_call_parents(history: &[Message], retain: &mut HashSet<usize>) {
    let initially_retained = retain.iter().copied().collect::<Vec<_>>();
    for idx in initially_retained {
        let result_ids = user_tool_result_ids(&history[idx]);
        if result_ids.is_empty() {
            continue;
        }

        for parent_idx in (0..idx).rev() {
            let call_ids = assistant_tool_call_ids(&history[parent_idx]);
            if !call_ids.is_empty() && result_ids.iter().all(|id| call_ids.contains(id)) {
                retain.insert(parent_idx);
                break;
            }
        }
    }
}

fn has_immediate_matching_tool_call(previous: &Message, result_ids: &HashSet<String>) -> bool {
    if result_ids.is_empty() {
        return true;
    }

    let call_ids = assistant_tool_call_ids(previous);
    !call_ids.is_empty() && result_ids.iter().all(|id| call_ids.contains(id))
}

fn sanitize_tool_order(messages: Vec<Message>) -> Vec<Message> {
    let mut rebuilt = Vec::with_capacity(messages.len());

    for message in messages {
        let result_ids = user_tool_result_ids(&message);
        if result_ids.is_empty() {
            rebuilt.push(message);
            continue;
        }

        let Some(previous) = rebuilt.last() else {
            continue;
        };

        if has_immediate_matching_tool_call(previous, &result_ids) {
            rebuilt.push(message);
        }
    }

    rebuilt
}

fn normalize_compacted_history_to_text(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .filter_map(|message| {
            let text = extract_message_text(&message);
            if text.trim().is_empty() {
                return None;
            }

            Some(match message {
                Message::User { .. } => Message::user(text),
                Message::Assistant { .. } => Message::assistant(text),
            })
        })
        .collect()
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
    let mut retain = retain;
    retain_required_tool_call_parents(history, &mut retain);
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
        let rebuilt = normalize_compacted_history_to_text(sanitize_tool_order(
            prune.retained_history.clone(),
        ));
        return (
            rebuilt.clone(),
            RebuildStats {
                retrieved_docs_count: 0,
                rebuilt_context_chars: estimate_context_chars(prompt, &rebuilt),
            },
        );
    }

    let packet = memory_packet.expect("checked above");
    let mut rebuilt = Vec::with_capacity(prune.retained_history.len() + 1);
    rebuilt.push(packet);
    rebuilt.extend_from_slice(&prune.retained_history);
    rebuilt = normalize_compacted_history_to_text(sanitize_tool_order(rebuilt));

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
    use rig::OneOrMany;

    fn assistant_tool_call_message(call_id: &str, name: &str) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::tool_call(
                call_id.to_string(),
                name.to_string(),
                serde_json::json!({}),
            )),
        }
    }

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
        assert!(extract_message_text(&rebuilt[0]).contains("[Memory Packet]"));
        assert!(extract_message_text(&rebuilt[1]).contains("initial"));
    }

    #[test]
    fn test_retain_required_tool_call_parents_for_tool_results() {
        let history = vec![
            assistant_tool_call_message("call-1", "read"),
            Message::tool_result("call-1", "ok"),
            Message::assistant("later"),
            Message::assistant("later-2"),
            Message::assistant("later-3"),
            Message::assistant("later-4"),
            Message::assistant("later-5"),
            Message::assistant("later-6"),
            Message::assistant("later-7"),
            Message::assistant("later-8"),
            Message::assistant("later-9"),
            Message::assistant("later-10"),
        ];
        let prompt = Message::user("task");

        let result = deterministic_prune(&history, &prompt, 0);
        let retained_text = result
            .retained_history
            .iter()
            .map(extract_message_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(retained_text.contains("[tool_call:call-1]"));
    }

    #[test]
    fn test_rebuild_sanitizes_orphaned_tool_result() {
        let prune = PruneResult {
            retained_history: vec![Message::tool_result("call-2", "orphaned")],
            archived_documents: Vec::new(),
            context_chars_before: 42,
        };
        let prompt = Message::user("task");
        let packet = build_memory_packet("summary", &[]);
        let (rebuilt, _) = rebuild_history_with_memory(&prune, Some(packet), &prompt);

        let rebuilt_text = rebuilt
            .iter()
            .map(extract_message_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rebuilt_text.contains("[tool_result:call-2]"));
    }

    #[test]
    fn test_rebuild_normalizes_paired_tool_call_result_to_text_messages() {
        let prune = PruneResult {
            retained_history: vec![
                assistant_tool_call_message("call-1", "read_file"),
                Message::tool_result("call-1", "file content"),
            ],
            archived_documents: Vec::new(),
            context_chars_before: 100,
        };
        let prompt = Message::user("task");
        let packet = build_memory_packet("summary", &[]);
        let (rebuilt, _) = rebuild_history_with_memory(&prune, Some(packet), &prompt);

        assert_eq!(rebuilt.len(), 2);
        assert!(matches!(rebuilt[0], Message::Assistant { .. }));
        assert!(matches!(rebuilt[1], Message::User { .. }));

        let rebuilt_text = rebuilt
            .iter()
            .map(extract_message_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rebuilt_text.contains("[tool_call:call-1]"));
        assert!(rebuilt_text.contains("[tool_result:call-1]"));
    }
}
