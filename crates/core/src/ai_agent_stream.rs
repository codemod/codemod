use std::collections::{HashMap, HashSet};

use serde_json::json;

use crate::{ai_handoff::LAUNCHABLE_AGENTS, structured_log::strip_ansi_escape_sequences};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AgentLogEvent {
    pub agent: String,
    pub stream: String,
    #[serde(flatten)]
    pub kind: AgentLogEventKind,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentLogEventKind {
    Status {
        status: String,
        detail: Option<String>,
    },
    Message {
        text: String,
        delta: bool,
    },
    ToolCall {
        tool_name: String,
        tool_params: serde_json::Value,
    },
    Warning {
        message: String,
        metadata: Option<serde_json::Value>,
    },
    Raw {
        line: String,
    },
}

impl AgentLogEvent {
    pub(crate) fn new(
        agent: impl Into<String>,
        stream: impl Into<String>,
        kind: AgentLogEventKind,
    ) -> Self {
        Self {
            agent: agent.into(),
            stream: stream.into(),
            kind,
        }
    }
}

pub(crate) fn agent_display_name(canonical: &str) -> &str {
    LAUNCHABLE_AGENTS
        .iter()
        .find(|agent| agent.canonical == canonical)
        .map(|agent| agent.label)
        .unwrap_or(canonical)
}

pub(crate) trait AgentStreamNormalizer {
    fn normalize_line(&mut self, stream: &str, line: &str) -> Option<NormalizedAgentLine>;
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NormalizedAgentLine {
    Event(AgentLogEvent),
    Suppress,
}

#[derive(Default)]
pub(crate) struct ClaudeStreamNormalizer {
    active_tool_name: Option<String>,
    active_tool_input: String,
}

#[derive(Default)]
pub(crate) struct OpenCodeStreamNormalizer {
    started: bool,
    rendered_tools: HashSet<String>,
    text_lengths: HashMap<String, usize>,
}

#[derive(Default)]
pub(crate) struct CodexStreamNormalizer {
    rendered_items: HashSet<String>,
}

impl AgentStreamNormalizer for ClaudeStreamNormalizer {
    fn normalize_line(&mut self, stream: &str, line: &str) -> Option<NormalizedAgentLine> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let kind = match value.get("type").and_then(serde_json::Value::as_str) {
            Some("stream_event") => self.normalize_stream_event(value.get("event")?),
            Some("system") => normalize_claude_system_event(&value),
            Some("rate_limit_event") => Some(normalize_claude_rate_limit_event(&value)),
            Some("result") | Some("assistant") | Some("user") => {
                return Some(NormalizedAgentLine::Suppress)
            }
            _ => return Some(NormalizedAgentLine::Suppress),
        };
        Some(match kind {
            Some(kind) => {
                NormalizedAgentLine::Event(AgentLogEvent::new("claude-code", stream, kind))
            }
            None => NormalizedAgentLine::Suppress,
        })
    }
}

impl ClaudeStreamNormalizer {
    fn normalize_stream_event(&mut self, event: &serde_json::Value) -> Option<AgentLogEventKind> {
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("content_block_start") => {
                let block = event.get("content_block")?;
                if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                    self.active_tool_name = Some(
                        block
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("tool")
                            .to_string(),
                    );
                    self.active_tool_input.clear();
                    if let Some(input) = block.get("input").filter(|input| !input.is_null()) {
                        if !input.as_object().is_some_and(serde_json::Map::is_empty) {
                            self.active_tool_input = input.to_string();
                        }
                    }
                }
                None
            }
            Some("content_block_delta") => {
                let delta = event.get("delta")?;
                match delta.get("type").and_then(serde_json::Value::as_str) {
                    Some("text_delta") => {
                        let text = delta
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("");
                        Some(AgentLogEventKind::Message {
                            text: sanitize_agent_text(text),
                            delta: true,
                        })
                    }
                    Some("input_json_delta") => {
                        if let Some(partial_json) = delta
                            .get("partial_json")
                            .and_then(serde_json::Value::as_str)
                        {
                            self.active_tool_input.push_str(partial_json);
                        }
                        None
                    }
                    _ => None,
                }
            }
            Some("content_block_stop") => self.active_tool_name.take().map(|name| {
                let input = std::mem::take(&mut self.active_tool_input);
                AgentLogEventKind::ToolCall {
                    tool_name: format_tool_name(&name),
                    tool_params: parse_json_or_raw(&input),
                }
            }),
            Some("message_start") | Some("message_delta") | Some("message_stop") => None,
            _ => None,
        }
    }
}

impl AgentStreamNormalizer for OpenCodeStreamNormalizer {
    fn normalize_line(&mut self, stream: &str, line: &str) -> Option<NormalizedAgentLine> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let kind = match value.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => self.normalize_complete_text_part(value.get("part").unwrap_or(&value)),
            Some("tool") | Some("tool_use") => {
                self.normalize_tool_part(value.get("part").unwrap_or(&value))
            }
            Some("agent") => normalize_opencode_agent_part(value.get("part").unwrap_or(&value)),
            Some("retry") => normalize_opencode_retry_part(value.get("part").unwrap_or(&value)),
            Some("error") | Some("session_error") | Some("session.error") => {
                normalize_opencode_session_error(&value)
            }
            Some("step_start") | Some("step_finish") => None,
            Some("message.updated") => self.normalize_message_updated(&value),
            Some("message.part.delta") => {
                self.normalize_message_part_delta(value.get("properties")?)
            }
            Some("message.part.updated") => {
                self.normalize_message_part_updated(value.get("properties")?)
            }
            _ => return Some(NormalizedAgentLine::Suppress),
        };
        Some(match kind {
            Some(kind) => NormalizedAgentLine::Event(AgentLogEvent::new("opencode", stream, kind)),
            None => NormalizedAgentLine::Suppress,
        })
    }
}

impl OpenCodeStreamNormalizer {
    fn normalize_message_updated(
        &mut self,
        value: &serde_json::Value,
    ) -> Option<AgentLogEventKind> {
        if self.started {
            return None;
        }
        let info = value.pointer("/properties/info")?;
        if info.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            return None;
        }
        self.started = true;
        let model = info
            .get("modelID")
            .or_else(|| info.pointer("/model/modelID"))
            .and_then(serde_json::Value::as_str)
            .map(sanitize_agent_text)
            .unwrap_or_else(|| "model".to_string());
        Some(AgentLogEventKind::Status {
            status: "started".to_string(),
            detail: Some(model),
        })
    }

    fn normalize_message_part_delta(
        &mut self,
        properties: &serde_json::Value,
    ) -> Option<AgentLogEventKind> {
        if properties
            .get("field")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            != "text"
        {
            return None;
        }
        let delta = properties
            .get("delta")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if let Some(part_id) = properties
            .get("partID")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        {
            let current = self.text_lengths.entry(part_id).or_insert(0);
            *current = current.saturating_add(delta.len());
        }
        Some(AgentLogEventKind::Message {
            text: sanitize_agent_text(delta),
            delta: true,
        })
    }

    fn normalize_message_part_updated(
        &mut self,
        properties: &serde_json::Value,
    ) -> Option<AgentLogEventKind> {
        let part = properties.get("part")?;
        match part.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => self.normalize_text_part(part),
            Some("tool") => self.normalize_tool_part(part),
            Some("agent") => normalize_opencode_agent_part(part),
            Some("retry") => normalize_opencode_retry_part(part),
            _ => None,
        }
    }

    fn normalize_text_part(&mut self, part: &serde_json::Value) -> Option<AgentLogEventKind> {
        if part
            .get("ignored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return None;
        }
        let id = part
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text")
            .to_string();
        let text = part
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let previous_len = *self.text_lengths.get(&id).unwrap_or(&0);
        let next_len = text.len();
        self.text_lengths.insert(id, next_len);
        if next_len <= previous_len {
            return None;
        }
        Some(AgentLogEventKind::Message {
            text: sanitize_agent_text(&text[previous_len..]),
            delta: true,
        })
    }

    fn normalize_complete_text_part(
        &mut self,
        part: &serde_json::Value,
    ) -> Option<AgentLogEventKind> {
        if part
            .get("ignored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return None;
        }
        let text = part
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            return None;
        }
        Some(AgentLogEventKind::Message {
            text: sanitize_agent_text(text),
            delta: false,
        })
    }

    fn normalize_tool_part(&mut self, part: &serde_json::Value) -> Option<AgentLogEventKind> {
        let state = part.get("state").unwrap_or(part);
        if state
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            == "pending"
        {
            return None;
        }
        let key = part
            .get("callID")
            .or_else(|| part.get("id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tool")
            .to_string();
        if !self.rendered_tools.insert(key) {
            return None;
        }
        let name = part
            .get("tool")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tool");
        Some(AgentLogEventKind::ToolCall {
            tool_name: format_tool_name(name),
            tool_params: state.clone(),
        })
    }
}

impl AgentStreamNormalizer for CodexStreamNormalizer {
    fn normalize_line(&mut self, stream: &str, line: &str) -> Option<NormalizedAgentLine> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let kind = match value.get("type").and_then(serde_json::Value::as_str) {
            Some("thread.started") | Some("turn.started") | Some("turn.completed") => None,
            Some("item.started") | Some("item.completed") => {
                self.normalize_item(value.get("item")?)
            }
            _ => return Some(NormalizedAgentLine::Suppress),
        };
        Some(match kind {
            Some(kind) => NormalizedAgentLine::Event(AgentLogEvent::new("codex", stream, kind)),
            None => NormalizedAgentLine::Suppress,
        })
    }
}

impl CodexStreamNormalizer {
    fn normalize_item(&mut self, item: &serde_json::Value) -> Option<AgentLogEventKind> {
        match item.get("type").and_then(serde_json::Value::as_str) {
            Some("agent_message") => normalize_codex_agent_message(item),
            Some("command_execution") => self.normalize_command_execution(item),
            Some("file_change") => self.normalize_file_change(item),
            Some("error") => normalize_codex_error(item),
            _ => None,
        }
    }

    fn normalize_command_execution(
        &mut self,
        item: &serde_json::Value,
    ) -> Option<AgentLogEventKind> {
        if item.get("status").and_then(serde_json::Value::as_str) != Some("in_progress") {
            return None;
        }
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("command")
            .to_string();
        if !self.rendered_items.insert(id) {
            return None;
        }
        Some(AgentLogEventKind::ToolCall {
            tool_name: "Bash".to_string(),
            tool_params: json!({
                "command": item.get("command").and_then(serde_json::Value::as_str).unwrap_or("")
            }),
        })
    }

    fn normalize_file_change(&mut self, item: &serde_json::Value) -> Option<AgentLogEventKind> {
        if item.get("status").and_then(serde_json::Value::as_str) != Some("completed") {
            return None;
        }
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("file-change")
            .to_string();
        if !self.rendered_items.insert(id) {
            return None;
        }
        let changes = item
            .get("changes")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let first_path = changes
            .first()
            .and_then(|change| change.get("path"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("file");
        Some(AgentLogEventKind::ToolCall {
            tool_name: codex_file_change_tool_name(&changes).to_string(),
            tool_params: json!({
                "path": first_path,
                "files": changes.len(),
                "changes": changes,
            }),
        })
    }
}

pub(crate) fn normalize_raw_agent_line(
    canonical: &str,
    stream: &str,
    line: String,
) -> AgentLogEvent {
    let line = if should_preserve_agent_stream(canonical) {
        sanitize_agent_text(&line)
    } else {
        format!("[{stream}] {}", sanitize_agent_text(&line))
    };
    AgentLogEvent::new(canonical, stream, AgentLogEventKind::Raw { line })
}

fn should_preserve_agent_stream(canonical: &str) -> bool {
    matches!(canonical, "claude-code" | "codex" | "opencode")
}

fn normalize_claude_system_event(value: &serde_json::Value) -> Option<AgentLogEventKind> {
    match value
        .get("subtype")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
    {
        "init" => Some(AgentLogEventKind::Status {
            status: "started".to_string(),
            detail: value
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(sanitize_agent_text),
        }),
        "api_retry" => Some(AgentLogEventKind::Warning {
            message: "API retrying".to_string(),
            metadata: None,
        }),
        _ => None,
    }
}

fn normalize_claude_rate_limit_event(value: &serde_json::Value) -> AgentLogEventKind {
    let info = value.get("rate_limit_info").unwrap_or(value);
    let status = info
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("warning")
        .replace('_', " ");
    AgentLogEventKind::Warning {
        message: format!("rate limit {status}"),
        metadata: value.get("rate_limit_info").cloned(),
    }
}

fn normalize_opencode_session_error(value: &serde_json::Value) -> Option<AgentLogEventKind> {
    let message = value
        .pointer("/properties/error/data/message")
        .or_else(|| value.pointer("/properties/error/message"))
        .or_else(|| value.pointer("/properties/message"))
        .and_then(serde_json::Value::as_str)?;
    Some(AgentLogEventKind::Warning {
        message: sanitize_agent_text(message),
        metadata: value.get("properties").cloned(),
    })
}

fn normalize_opencode_agent_part(part: &serde_json::Value) -> Option<AgentLogEventKind> {
    let name = part.get("name").and_then(serde_json::Value::as_str)?;
    Some(AgentLogEventKind::Status {
        status: "agent".to_string(),
        detail: Some(sanitize_agent_text(name)),
    })
}

fn normalize_opencode_retry_part(part: &serde_json::Value) -> Option<AgentLogEventKind> {
    Some(AgentLogEventKind::Warning {
        message: "retry".to_string(),
        metadata: Some(part.clone()),
    })
}

fn normalize_codex_agent_message(item: &serde_json::Value) -> Option<AgentLogEventKind> {
    let text = item
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return None;
    }
    Some(AgentLogEventKind::Message {
        text: sanitize_agent_text(text),
        delta: false,
    })
}

fn normalize_codex_error(item: &serde_json::Value) -> Option<AgentLogEventKind> {
    let message = item.get("message").and_then(serde_json::Value::as_str)?;
    Some(AgentLogEventKind::Warning {
        message: sanitize_agent_text(message),
        metadata: Some(item.clone()),
    })
}

fn codex_file_change_tool_name(changes: &[serde_json::Value]) -> &'static str {
    let kind = changes
        .first()
        .and_then(|change| change.get("kind"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("update");
    match kind {
        "add" | "create" => "Write",
        "delete" | "remove" => "Delete",
        _ => "Edit",
    }
}

fn format_tool_name(name: &str) -> String {
    match name {
        "read" => "Read".to_string(),
        "write" => "Write".to_string(),
        "edit" => "Edit".to_string(),
        "bash" => "Bash".to_string(),
        "apply_patch" => "Edit".to_string(),
        _ => name
            .split(['_', '-'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => format!(
                        "{}{}",
                        first.to_uppercase(),
                        chars.as_str().to_ascii_lowercase()
                    ),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn parse_json_or_raw(input: &str) -> serde_json::Value {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Object(Default::default());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({ "raw": sanitize_agent_text(trimmed) }))
}

fn sanitize_agent_text(text: &str) -> String {
    strip_ansi_escape_sequences(text)
        .replace("[0m", "")
        .replace("[1m", "")
}

#[cfg(test)]
mod tests {
    use super::{
        AgentLogEventKind, AgentStreamNormalizer, ClaudeStreamNormalizer, CodexStreamNormalizer,
        NormalizedAgentLine, OpenCodeStreamNormalizer,
    };

    fn event(line: Option<NormalizedAgentLine>) -> super::AgentLogEvent {
        match line.expect("normalized line") {
            NormalizedAgentLine::Event(event) => event,
            NormalizedAgentLine::Suppress => panic!("line was suppressed"),
        }
    }

    fn suppressed(line: Option<NormalizedAgentLine>) -> bool {
        matches!(line, Some(NormalizedAgentLine::Suppress))
    }

    #[test]
    fn claude_system_init_becomes_status_event() {
        let mut normalizer = ClaudeStreamNormalizer::default();
        let event = event(normalizer.normalize_line(
            "stderr",
            r#"{"type":"system","subtype":"init","model":"sonnet"}"#,
        ));

        assert_eq!(
            event.kind,
            AgentLogEventKind::Status {
                status: "started".to_string(),
                detail: Some("sonnet".to_string())
            }
        );
    }

    #[test]
    fn claude_tool_use_becomes_structured_tool_call() {
        let mut normalizer = ClaudeStreamNormalizer::default();
        assert!(suppressed(normalizer
            .normalize_line(
                "stderr",
                r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"Read","input":{}}}}"#
            )));
        assert!(suppressed(normalizer
            .normalize_line(
                "stderr",
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"README.md\"}"}}}"#
            )));
        let event = event(normalizer.normalize_line(
            "stderr",
            r#"{"type":"stream_event","event":{"type":"content_block_stop"}}"#,
        ));

        match event.kind {
            AgentLogEventKind::ToolCall {
                tool_name,
                tool_params,
            } => {
                assert_eq!(tool_name, "Read");
                assert_eq!(tool_params["file_path"], "README.md");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn opencode_delta_becomes_message_event() {
        let mut normalizer = OpenCodeStreamNormalizer::default();
        let event = event(normalizer.normalize_line(
                "stdout",
                r#"{"type":"message.part.delta","properties":{"field":"text","delta":"hello","partID":"p1"}}"#,
            ));

        assert_eq!(
            event.kind,
            AgentLogEventKind::Message {
                text: "hello".to_string(),
                delta: true
            }
        );
    }

    #[test]
    fn codex_command_execution_becomes_tool_call() {
        let mut normalizer = CodexStreamNormalizer::default();
        let event = event(normalizer.normalize_line(
                "stdout",
                r#"{"type":"item.started","item":{"id":"1","type":"command_execution","status":"in_progress","command":"git status"}}"#,
            ));

        match event.kind {
            AgentLogEventKind::ToolCall {
                tool_name,
                tool_params,
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_params["command"], "git status");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
