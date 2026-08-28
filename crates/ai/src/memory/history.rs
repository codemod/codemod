//! Message extraction and lightweight context-size estimation.

use rig::completion::message::{ToolResultContent, UserContent};
use rig::completion::{AssistantContent, Message};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryDocument {
    pub id: String,
    pub index: usize,
    pub role: MessageRole,
    pub is_tool_result: bool,
    pub text: String,
}

pub fn clip_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect()
}

fn join_non_empty<I>(parts: I) -> String
where
    I: IntoIterator<Item = String>,
{
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_result_content(content: &ToolResultContent) -> String {
    if let ToolResultContent::Text(text) = content {
        text.text.clone()
    } else {
        "[non-text tool result]".to_string()
    }
}

fn render_user_content(item: &UserContent) -> String {
    if let UserContent::Text(text) = item {
        return text.text.clone();
    }

    if let UserContent::ToolResult(tool_result) = item {
        let body = join_non_empty(tool_result.content.iter().map(render_tool_result_content));
        return format!("[tool_result:{}]\n{}", tool_result.id, body);
    }

    "[non-text user content]".to_string()
}

fn render_assistant_content(item: &AssistantContent) -> String {
    if let AssistantContent::Text(text) = item {
        return text.text.clone();
    }

    if let AssistantContent::ToolCall(tool_call) = item {
        let args = serde_json::to_string(&tool_call.function.arguments)
            .unwrap_or_else(|_| "{}".to_string());
        return format!(
            "[tool_call:{}]\n{}({})",
            tool_call.id,
            tool_call.function.name,
            clip_chars(&args, 500)
        );
    }

    if let AssistantContent::Reasoning(reasoning) = item {
        return reasoning.display_text();
    }

    "[non-text assistant content]".to_string()
}

pub fn extract_message_text(message: &Message) -> String {
    match message {
        Message::User { content } => join_non_empty(content.iter().map(render_user_content)),
        Message::Assistant { content, .. } => {
            join_non_empty(content.iter().map(render_assistant_content))
        }
    }
}

pub fn is_tool_result_message(message: &Message) -> bool {
    matches!(
        message,
        Message::User { content }
            if content
                .iter()
                .any(|item| matches!(item, UserContent::ToolResult(_)))
    )
}

pub fn estimate_context_chars(prompt: &Message, history: &[Message]) -> usize {
    let history_chars = history
        .iter()
        .map(extract_message_text)
        .map(|s| s.chars().count())
        .sum::<usize>();
    history_chars + extract_message_text(prompt).chars().count()
}

pub fn extract_history_documents(
    history: &[Message],
    max_doc_chars: usize,
) -> Vec<HistoryDocument> {
    history
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let text = extract_message_text(message);
            if text.trim().is_empty() {
                return None;
            }

            let role = match message {
                Message::User { .. } => MessageRole::User,
                Message::Assistant { .. } => MessageRole::Assistant,
            };

            Some(HistoryDocument {
                id: format!("history-{}", index),
                index,
                role,
                is_tool_result: is_tool_result_message(message),
                text: clip_chars(&text, max_doc_chars),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_message_text_for_user_and_assistant() {
        let user = Message::user("hello");
        let assistant = Message::assistant("world");

        assert_eq!(extract_message_text(&user), "hello");
        assert_eq!(extract_message_text(&assistant), "world");
    }

    #[test]
    fn test_extract_message_text_for_tool_result() {
        let tool_result = Message::tool_result("call-1", "done");
        let text = extract_message_text(&tool_result);

        assert!(text.contains("[tool_result:call-1]"));
        assert!(text.contains("done"));
    }

    #[test]
    fn test_estimate_context_chars_counts_prompt_and_history() {
        let prompt = Message::user("task");
        let history = vec![Message::user("a"), Message::assistant("b")];

        assert!(estimate_context_chars(&prompt, &history) >= 4);
    }

    #[test]
    fn test_extract_history_documents_clips_and_labels() {
        let history = vec![
            Message::user("short"),
            Message::tool_result("call-2", "x".repeat(100)),
            Message::assistant("done"),
        ];
        let docs = extract_history_documents(&history, 20);

        assert_eq!(docs.len(), 3);
        assert_eq!(docs[0].role, MessageRole::User);
        assert!(docs[1].is_tool_result);
        assert!(docs[1].text.chars().count() <= 20);
    }
}
