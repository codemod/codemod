//! What the adapters share for building prompts and reading output.

use serde_json::{json, Value};

use crate::adapter::Parser;
use crate::event::{AgentEvent, FileChange, FileChangeKind, Usage};
use crate::transcript::Transcript;

/// The prompt with extra instructions in front, for agents without a flag
/// for them.
pub(crate) fn with_instructions(prompt: &str, system_prompt: Option<&str>) -> String {
    match system_prompt.map(str::trim).filter(|s| !s.is_empty()) {
        Some(instructions) => {
            format!("<instructions>\n{instructions}\n</instructions>\n\n{prompt}")
        }
        None => prompt.to_string(),
    }
}

/// A prompt that goes on the command line as an argument of its own, for
/// agents not known to take `flag=value` or `--`: one that starts with `-`
/// gets a space in front, so it is not read as a flag.
pub(crate) fn not_a_flag(prompt: String) -> String {
    if prompt.starts_with('-') {
        format!(" {prompt}")
    } else {
        prompt
    }
}

/// The string at a JSON pointer.
pub(crate) fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// A line as JSON, when it is a JSON object.
pub(crate) fn json_line(line: &str) -> Option<Value> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    serde_json::from_str(line).ok().filter(Value::is_object)
}

/// The agent says it started: note its session and model.
pub(crate) fn started(
    transcript: &mut Transcript,
    session: Option<&str>,
    model: Option<&str>,
) -> AgentEvent {
    if let Some(session) = session {
        transcript.session(session);
    }
    if let Some(model) = model {
        transcript.model(model);
    }
    AgentEvent::Started {
        session_id: session.map(str::to_string),
        model: model.map(str::to_string),
    }
}

/// The `input_tokens` and `output_tokens` of a usage object.
pub(crate) fn token_usage(usage: Option<&Value>) -> Usage {
    let count = |key: &str| usage?.get(key)?.as_u64();
    Usage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cost_usd: None,
    }
}

/// Tool arguments, which some agents send as a JSON string.
pub(crate) fn arguments(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// A tool's output as text: a string, the text of content blocks, an
/// object's `content` or `message`, or JSON.
pub(crate) fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| str_at(block, "/text"))
            .collect::<Vec<_>>()
            .join("\n"),
        other => str_at(other, "/content")
            .or_else(|| str_at(other, "/message"))
            .map(str::to_string)
            .unwrap_or_else(|| other.to_string()),
    }
}

/// A tool call, with the file changes its input describes.
pub(crate) fn tool_call(id: Option<&str>, name: &str, input: Value) -> AgentEvent {
    AgentEvent::ToolCall {
        id: id.map(str::to_string),
        name: name.to_string(),
        changes: file_changes(&input),
        input,
    }
}

const PATH_KEYS: &[&str] = &["file_path", "filePath", "path"];
const CONTENT_KEYS: &[&str] = &["content", "file_text", "fileText"];
const OLD_KEYS: &[&str] = &["old_string", "oldString", "old_str"];
const NEW_KEYS: &[&str] = &["new_string", "newString", "new_str"];

/// The file changes in a tool call's input. Agents name their file tools
/// and the fields differently, but the inputs come in the same shapes: a
/// path with the file's whole content, with a text and its replacement, or
/// with a list of such `edits`.
pub(crate) fn file_changes(input: &Value) -> Vec<FileChange> {
    let first = |value: &Value, keys: &[&str]| {
        keys.iter()
            .find_map(|key| value.get(key)?.as_str())
            .map(str::to_string)
    };
    let Some(path) = first(input, PATH_KEYS) else {
        return Vec::new();
    };
    let replacement = |edit: &Value| {
        Some(FileChange {
            path: path.clone(),
            kind: FileChangeKind::Update,
            old: Some(first(edit, OLD_KEYS)?),
            new: Some(first(edit, NEW_KEYS)?),
            diff: None,
        })
    };
    if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        return edits.iter().filter_map(replacement).collect();
    }
    if let Some(change) = replacement(input) {
        return vec![change];
    }
    match first(input, CONTENT_KEYS) {
        Some(content) => vec![FileChange {
            path,
            kind: FileChangeKind::Write,
            old: None,
            new: Some(content),
            diff: None,
        }],
        None => Vec::new(),
    }
}

/// Reads every line as text the agent wrote: for agents, or versions of
/// them, that print their answer and nothing else. The run's answer is all
/// of it.
pub(crate) struct TextParser;

impl Parser for TextParser {
    fn parse(&mut self, line: &str, _transcript: &mut Transcript) -> Vec<AgentEvent> {
        vec![AgentEvent::Text {
            text: format!("{line}\n"),
            delta: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_changes_are_read_from_the_shapes_agents_use() {
        let write = file_changes(&json!({ "file_path": "a.txt", "content": "hi" }));
        assert_eq!(write.len(), 1);
        assert_eq!(write[0].kind, FileChangeKind::Write);
        assert_eq!(write[0].new.as_deref(), Some("hi"));

        let edit = file_changes(&json!({ "path": "a.txt", "old_str": "a", "new_str": "b" }));
        assert_eq!(edit[0].kind, FileChangeKind::Update);
        assert_eq!(
            (edit[0].old.as_deref(), edit[0].new.as_deref()),
            (Some("a"), Some("b"))
        );

        let edits = file_changes(&json!({
            "filePath": "a.txt",
            "edits": [
                { "oldString": "a", "newString": "b" },
                { "oldString": "c", "newString": "d" },
            ],
        }));
        assert_eq!(edits.len(), 2);
        assert!(edits.iter().all(|change| change.path == "a.txt"));

        assert!(file_changes(&json!({ "path": "a.txt" })).is_empty());
        assert!(file_changes(&json!({ "command": "ls" })).is_empty());
    }

    #[test]
    fn tool_output_is_flattened_to_text() {
        assert_eq!(text_of(&json!("plain")), "plain");
        assert_eq!(
            text_of(&json!([{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }])),
            "a\nb"
        );
        assert_eq!(text_of(&json!({ "content": "inner" })), "inner");
        assert_eq!(text_of(&json!({ "lines": 1 })), r#"{"lines":1}"#);
        assert_eq!(text_of(&Value::Null), "");
    }

    #[test]
    fn a_prompt_argument_never_starts_with_a_dash() {
        assert_eq!(not_a_flag("--help me".into()), " --help me");
        assert_eq!(not_a_flag("help me".into()), "help me");
    }
}
