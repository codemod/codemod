use std::{
    collections::{HashMap, HashSet},
    io::IsTerminal,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use similar::{ChangeTag, TextDiff};

use crate::{
    ai_handoff::LAUNCHABLE_AGENTS,
    structured_log::{strip_ansi_escape_sequences, StructuredLogger},
};

pub(crate) fn agent_display_name(canonical: &str) -> &str {
    LAUNCHABLE_AGENTS
        .iter()
        .find(|agent| agent.canonical == canonical)
        .map(|agent| agent.label)
        .unwrap_or(canonical)
}

fn should_preserve_agent_stream(canonical: &str) -> bool {
    matches!(canonical, "claude-code" | "codex" | "opencode")
}

fn agent_colors_enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn agent_style(text: impl AsRef<str>, code: &str) -> String {
    let text = text.as_ref();
    if agent_colors_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn agent_dim(text: impl AsRef<str>) -> String {
    agent_style(text, "2")
}

fn agent_accent(text: impl AsRef<str>) -> String {
    agent_style(text, "36")
}

fn agent_success(text: impl AsRef<str>) -> String {
    agent_style(text, "32")
}

fn agent_warning(text: impl AsRef<str>) -> String {
    agent_style(text, "33")
}

fn sanitize_agent_text(text: &str) -> String {
    strip_ansi_escape_sequences(text)
        .replace("[0m", "")
        .replace("[1m", "")
}

pub(crate) fn agent_header(agent: &str) -> String {
    format!("{} {}", agent_accent("AI"), agent_style(agent, "1;36"))
}

fn agent_tool_line(name: &str, summary: &str) -> String {
    if summary.is_empty() {
        format!("  {} {}", agent_success("•"), agent_style(name, "1"))
    } else {
        format!(
            "  {} {} {}",
            agent_success("•"),
            agent_style(name, "1"),
            agent_dim(summary)
        )
    }
}

pub(crate) fn agent_status_line(status: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("  {} {}", agent_accent("•"), status)
    } else {
        format!("  {} {} {}", agent_accent("•"), status, agent_dim(detail))
    }
}

fn agent_warning_line(label: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("  {} {}", agent_warning("!"), label)
    } else {
        format!("  {} {} {}", agent_warning("!"), label, agent_dim(detail))
    }
}

fn agent_message_prefix() -> String {
    format!("  {} ", agent_accent("›"))
}

fn agent_message_line(text: &str) -> String {
    let prefix = agent_message_prefix();
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AgentStreamRender {
    Suppress,
    Line(String),
    Fragment(String),
}

#[derive(Default)]
pub(crate) struct ClaudeStreamRenderer {
    active_tool_name: Option<String>,
    active_tool_input: String,
}

#[derive(Default)]
pub(crate) struct OpenCodeStreamRenderer {
    started: bool,
    rendered_tools: HashSet<String>,
    text_lengths: HashMap<String, usize>,
}

#[derive(Default)]
pub(crate) struct CodexStreamRenderer {
    rendered_items: HashSet<String>,
}

impl ClaudeStreamRenderer {
    pub(crate) fn render_line(&mut self, line: &str) -> Option<AgentStreamRender> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("stream_event") => self.render_stream_event(value.get("event")?),
            Some("system") => Some(render_claude_system_event(&value)),
            Some("rate_limit_event") => Some(render_claude_rate_limit_event(&value)),
            Some("result") | Some("assistant") | Some("user") => Some(AgentStreamRender::Suppress),
            Some(_) => Some(AgentStreamRender::Suppress),
            None if value.is_object() => Some(AgentStreamRender::Suppress),
            None => None,
        }
    }

    fn render_stream_event(&mut self, event: &serde_json::Value) -> Option<AgentStreamRender> {
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("content_block_start") => {
                let block = event.get("content_block")?;
                if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                    let name = block
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("tool");
                    self.active_tool_name = Some(name.to_string());
                    self.active_tool_input.clear();
                    if let Some(input) = block.get("input").filter(|input| !input.is_null()) {
                        let is_empty_object = input
                            .as_object()
                            .map(|object| object.is_empty())
                            .unwrap_or(false);
                        if !is_empty_object {
                            self.active_tool_input = input.to_string();
                        }
                    }
                    return Some(AgentStreamRender::Suppress);
                }
                Some(AgentStreamRender::Suppress)
            }
            Some("content_block_delta") => {
                let delta = event.get("delta")?;
                match delta.get("type").and_then(serde_json::Value::as_str) {
                    Some("text_delta") => {
                        let text = delta
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("");
                        Some(AgentStreamRender::Fragment(text.to_string()))
                    }
                    Some("input_json_delta") => {
                        if let Some(partial_json) = delta
                            .get("partial_json")
                            .and_then(serde_json::Value::as_str)
                        {
                            self.active_tool_input.push_str(partial_json);
                        }
                        Some(AgentStreamRender::Suppress)
                    }
                    _ => Some(AgentStreamRender::Suppress),
                }
            }
            Some("content_block_stop") => {
                if let Some(name) = self.active_tool_name.take() {
                    let input = std::mem::take(&mut self.active_tool_input);
                    return Some(AgentStreamRender::Line(format_claude_tool_call(
                        &name, &input,
                    )));
                }
                Some(AgentStreamRender::Suppress)
            }
            Some("message_start") | Some("message_delta") | Some("message_stop") => {
                Some(AgentStreamRender::Suppress)
            }
            _ => None,
        }
    }
}

impl OpenCodeStreamRenderer {
    pub(crate) fn render_line(&mut self, line: &str) -> Option<AgentStreamRender> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => {
                let part = value.get("part").unwrap_or(&value);
                Some(self.render_complete_text_part(part))
            }
            Some("tool") | Some("tool_use") => {
                let part = value.get("part").unwrap_or(&value);
                Some(self.render_tool_part(part))
            }
            Some("agent") => {
                let part = value.get("part").unwrap_or(&value);
                Some(render_opencode_agent_part(part))
            }
            Some("retry") => {
                let part = value.get("part").unwrap_or(&value);
                Some(render_opencode_retry_part(part))
            }
            Some("error") | Some("session_error") => Some(render_opencode_session_error(&value)),
            Some("step_start") | Some("step_finish") => Some(AgentStreamRender::Suppress),
            Some("message.updated") => Some(self.render_message_updated(&value)),
            Some("message.part.delta") => {
                Some(self.render_message_part_delta(value.get("properties")?))
            }
            Some("message.part.updated") => {
                Some(self.render_message_part_updated(value.get("properties")?))
            }
            Some("session.error") => Some(render_opencode_session_error(&value)),
            Some(_) => Some(AgentStreamRender::Suppress),
            None if value.is_object() => Some(AgentStreamRender::Suppress),
            None => None,
        }
    }

    fn render_message_updated(&mut self, value: &serde_json::Value) -> AgentStreamRender {
        if self.started {
            return AgentStreamRender::Suppress;
        }

        let Some(info) = value.pointer("/properties/info") else {
            return AgentStreamRender::Suppress;
        };
        if info.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            return AgentStreamRender::Suppress;
        }

        self.started = true;
        let model = info
            .get("modelID")
            .or_else(|| info.pointer("/model/modelID"))
            .and_then(serde_json::Value::as_str)
            .map(sanitize_agent_text)
            .unwrap_or_else(|| "model".to_string());
        AgentStreamRender::Line(agent_status_line("started", &model))
    }

    fn render_message_part_delta(&mut self, properties: &serde_json::Value) -> AgentStreamRender {
        let field = properties
            .get("field")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if field != "text" {
            return AgentStreamRender::Suppress;
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
        AgentStreamRender::Fragment(delta.to_string())
    }

    fn render_message_part_updated(&mut self, properties: &serde_json::Value) -> AgentStreamRender {
        let Some(part) = properties.get("part") else {
            return AgentStreamRender::Suppress;
        };

        match part.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => self.render_text_part(part),
            Some("tool") => self.render_tool_part(part),
            Some("agent") => render_opencode_agent_part(part),
            Some("retry") => render_opencode_retry_part(part),
            _ => AgentStreamRender::Suppress,
        }
    }

    fn render_text_part(&mut self, part: &serde_json::Value) -> AgentStreamRender {
        if part
            .get("ignored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return AgentStreamRender::Suppress;
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
            return AgentStreamRender::Suppress;
        }

        AgentStreamRender::Fragment(text[previous_len..].to_string())
    }

    fn render_complete_text_part(&mut self, part: &serde_json::Value) -> AgentStreamRender {
        if part
            .get("ignored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return AgentStreamRender::Suppress;
        }

        let text = part
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            return AgentStreamRender::Suppress;
        }

        AgentStreamRender::Line(agent_message_line(text))
    }

    fn render_tool_part(&mut self, part: &serde_json::Value) -> AgentStreamRender {
        let state = part.get("state").unwrap_or(part);
        let status = state
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if status == "pending" {
            return AgentStreamRender::Suppress;
        }

        let key = part
            .get("callID")
            .or_else(|| part.get("id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tool")
            .to_string();
        if !self.rendered_tools.insert(key) {
            return AgentStreamRender::Suppress;
        }

        let name = part
            .get("tool")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tool");
        AgentStreamRender::Line(format_opencode_tool_call(&format_tool_name(name), state))
    }
}

impl CodexStreamRenderer {
    pub(crate) fn render_line(&mut self, line: &str) -> Option<AgentStreamRender> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("thread.started") | Some("turn.started") | Some("turn.completed") => {
                Some(AgentStreamRender::Suppress)
            }
            Some("item.started") | Some("item.completed") => {
                Some(self.render_item(value.get("item")?))
            }
            Some(_) => Some(AgentStreamRender::Suppress),
            None if value.is_object() => Some(AgentStreamRender::Suppress),
            None => None,
        }
    }

    fn render_item(&mut self, item: &serde_json::Value) -> AgentStreamRender {
        match item.get("type").and_then(serde_json::Value::as_str) {
            Some("agent_message") => render_codex_agent_message(item),
            Some("command_execution") => self.render_command_execution(item),
            Some("file_change") => self.render_file_change(item),
            Some("error") => render_codex_error(item),
            _ => AgentStreamRender::Suppress,
        }
    }

    fn render_command_execution(&mut self, item: &serde_json::Value) -> AgentStreamRender {
        if item.get("status").and_then(serde_json::Value::as_str) != Some("in_progress") {
            return AgentStreamRender::Suppress;
        }
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("command")
            .to_string();
        if !self.rendered_items.insert(id) {
            return AgentStreamRender::Suppress;
        }
        let command = item
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        AgentStreamRender::Line(agent_tool_line(
            "Bash",
            &format!("command={}", truncate_tool_value(command, 120)),
        ))
    }

    fn render_file_change(&mut self, item: &serde_json::Value) -> AgentStreamRender {
        if item.get("status").and_then(serde_json::Value::as_str) != Some("completed") {
            return AgentStreamRender::Suppress;
        }
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("file-change")
            .to_string();
        if !self.rendered_items.insert(id) {
            return AgentStreamRender::Suppress;
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
        let name = codex_file_change_tool_name(&changes);
        let summary = if changes.len() > 1 {
            format!(
                "{} ({} files)",
                compact_path_display(first_path),
                changes.len()
            )
        } else {
            compact_path_display(first_path)
        };
        let mut line = agent_tool_line(name, &summary);
        if changes.len() == 1 {
            if let Some(diff) = format_git_worktree_diff(first_path) {
                line.push('\n');
                line.push_str(&diff);
            }
        }
        AgentStreamRender::Line(line)
    }
}

pub(crate) fn render_agent_stream_line(
    canonical: &str,
    stream: &str,
    line: String,
) -> AgentStreamRender {
    if canonical == "claude-code" {
        let mut renderer = ClaudeStreamRenderer::default();
        return renderer
            .render_line(&line)
            .unwrap_or(AgentStreamRender::Line(line));
    }

    let line = if should_preserve_agent_stream(canonical) {
        line
    } else {
        format!("[{stream}] {line}")
    };
    AgentStreamRender::Line(line)
}

#[allow(dead_code)]
pub(crate) fn format_agent_stream_line(canonical: &str, stream: &str, line: String) -> String {
    match render_agent_stream_line(canonical, stream, line) {
        AgentStreamRender::Line(line) | AgentStreamRender::Fragment(line) => line,
        AgentStreamRender::Suppress => String::new(),
    }
}

fn render_claude_system_event(value: &serde_json::Value) -> AgentStreamRender {
    let subtype = value
        .get("subtype")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match subtype {
        "init" => {
            let model = value
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(sanitize_agent_text)
                .unwrap_or_else(|| "model".to_string());
            AgentStreamRender::Line(agent_status_line("started", &model))
        }
        "api_retry" => AgentStreamRender::Line(agent_warning_line("API retrying", "")),
        _ => AgentStreamRender::Suppress,
    }
}

fn render_claude_rate_limit_event(value: &serde_json::Value) -> AgentStreamRender {
    let info = value.get("rate_limit_info").unwrap_or(value);
    let status = info
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("warning")
        .replace('_', " ");
    let limit_type = info
        .get("rateLimitType")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("rate limit")
        .replace('_', " ");
    let utilization = info
        .get("utilization")
        .and_then(serde_json::Value::as_f64)
        .map(|value| format!("{:.0}%", value * 100.0));

    let mut detail = format!("({limit_type}");
    if let Some(utilization) = utilization {
        detail.push_str(", ");
        detail.push_str(&utilization);
        detail.push_str(" used");
    }
    detail.push(')');
    AgentStreamRender::Line(agent_warning_line(&format!("rate limit {status}"), &detail))
}

fn render_opencode_session_error(value: &serde_json::Value) -> AgentStreamRender {
    let message = value
        .pointer("/properties/error/data/message")
        .or_else(|| value.pointer("/properties/error/message"))
        .or_else(|| value.pointer("/properties/message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("session error");
    AgentStreamRender::Line(agent_warning_line("error", &sanitize_agent_text(message)))
}

fn render_opencode_agent_part(part: &serde_json::Value) -> AgentStreamRender {
    let name = part
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("agent");
    AgentStreamRender::Line(agent_status_line("agent", &sanitize_agent_text(name)))
}

fn render_opencode_retry_part(part: &serde_json::Value) -> AgentStreamRender {
    let attempt = part
        .get("attempt")
        .and_then(serde_json::Value::as_u64)
        .map(|attempt| format!("attempt {attempt}"))
        .unwrap_or_default();
    AgentStreamRender::Line(agent_warning_line("retry", &attempt))
}

fn render_codex_agent_message(item: &serde_json::Value) -> AgentStreamRender {
    let text = item
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return AgentStreamRender::Suppress;
    }
    AgentStreamRender::Line(agent_message_line(text))
}

fn render_codex_error(item: &serde_json::Value) -> AgentStreamRender {
    let message = item
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("error");
    AgentStreamRender::Line(agent_warning_line("error", &sanitize_agent_text(message)))
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
                    Some(first) => {
                        format!(
                            "{}{}",
                            first.to_uppercase(),
                            chars.as_str().to_ascii_lowercase()
                        )
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn format_git_worktree_diff(path: &str) -> Option<String> {
    let path = Path::new(path);
    let worktree = if path.is_absolute() {
        path.parent().unwrap_or_else(|| Path::new("."))
    } else {
        Path::new(".")
    };
    let path_arg = if path.is_absolute() {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path.to_str()?)
            .to_string()
    } else {
        path.to_string_lossy().into_owned()
    };

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("diff")
        .arg("--")
        .arg(&path_arg)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let diff = String::from_utf8(output.stdout).ok()?;
    if diff.trim().is_empty() {
        return None;
    }
    Some(format_unified_agent_diff(&diff))
}

fn format_opencode_tool_call(name: &str, state: &serde_json::Value) -> String {
    let input = state
        .get("input")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    let summary = summarize_opencode_tool_state(name, state)
        .unwrap_or_else(|| summarize_opencode_tool_input(name, &input));
    let mut line = agent_tool_line(name, &summary);
    if let Some(diff) = format_opencode_tool_diff(name, state, &input) {
        line.push('\n');
        line.push_str(&diff);
    }
    line
}

fn summarize_opencode_tool_state(name: &str, state: &serde_json::Value) -> Option<String> {
    if matches!(name, "Edit" | "Write" | "MultiEdit") {
        if let Some(path) = state
            .pointer("/metadata/files/0/relativePath")
            .or_else(|| state.pointer("/metadata/files/0/filePath"))
            .and_then(serde_json::Value::as_str)
        {
            return Some(compact_path_display(path));
        }
    }

    let title = state
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(sanitize_agent_text)?;
    let first_line = title.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() || first_line.starts_with("Success.") {
        return None;
    }
    if let Some(rest) = first_line
        .strip_prefix(name)
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
    {
        return Some(compact_path_display(rest));
    }
    Some(compact_path_display(first_line))
}

fn summarize_opencode_tool_input(name: &str, input: &serde_json::Value) -> String {
    if input.is_null() {
        return String::new();
    }
    summarize_claude_tool_input(name, &input.to_string())
}

fn format_opencode_tool_diff(
    name: &str,
    state: &serde_json::Value,
    input: &serde_json::Value,
) -> Option<String> {
    if let Some(diff) = state
        .pointer("/metadata/diff")
        .and_then(serde_json::Value::as_str)
        .filter(|diff| !diff.trim().is_empty())
    {
        return Some(format_unified_agent_diff(diff));
    }

    format_claude_tool_diff(name, &input.to_string())
}

fn format_unified_agent_diff(diff: &str) -> String {
    let mut file_path = "";
    let mut body = Vec::new();

    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            file_path = path.trim_start_matches("b/").trim();
            continue;
        }
        if line.starts_with("Index:")
            || line.starts_with("===")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
        {
            continue;
        }
        body.push(line);
    }

    let mut output = format!(
        "    {} {}\n",
        agent_dim("diff"),
        agent_dim(compact_path_display(file_path))
    );
    let mut line_count = 0usize;
    let max_lines = 8usize;

    for line in body {
        if line.starts_with("@@") {
            output.push_str("    ");
            output.push_str(&agent_accent(line));
            output.push('\n');
            continue;
        }

        if line_count >= max_lines {
            output.push_str("    ");
            output.push_str(&agent_dim("... diff truncated"));
            output.push('\n');
            break;
        }

        output.push_str("    ");
        if line.starts_with('-') {
            output.push_str(&agent_style(line, "31"));
        } else if line.starts_with('+') {
            output.push_str(&agent_style(line, "32"));
        } else {
            output.push_str(&agent_dim(line));
        }
        output.push('\n');
        line_count += 1;
    }

    output.trim_end().to_string()
}

fn format_claude_tool_call(name: &str, input: &str) -> String {
    let mut line = agent_tool_line(name, &summarize_claude_tool_input(name, input));
    if let Some(diff) = format_claude_tool_diff(name, input) {
        line.push('\n');
        line.push_str(&diff);
    }
    line
}

fn summarize_claude_tool_input(name: &str, input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return String::new();
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return truncate_tool_value(&sanitize_agent_text(trimmed), 120);
    };
    let Some(object) = value.as_object() else {
        return truncate_tool_value(&sanitize_agent_text(&value.to_string()), 120);
    };

    if let Some(summary) = summarize_known_tool(name, object) {
        return summary;
    }

    let preferred_keys = [
        "file_path",
        "filePath",
        "path",
        "pattern",
        "command",
        "cmd",
        "url",
        "query",
        "description",
    ];
    let mut parts = Vec::new();
    for key in preferred_keys {
        if let Some(value) = object.get(key) {
            parts.push(format!("{key}={}", format_tool_value(key, value)));
        }
        if parts.len() == 2 {
            break;
        }
    }
    if parts.is_empty() {
        for (key, value) in object.iter().take(2) {
            parts.push(format!("{key}={}", format_tool_value(key, value)));
        }
    }

    parts.join(" ")
}

fn summarize_known_tool(
    name: &str,
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    match name {
        "Read" => object
            .get("file_path")
            .and_then(serde_json::Value::as_str)
            .map(compact_path_display),
        "Write" => {
            let file_path = object
                .get("file_path")
                .and_then(serde_json::Value::as_str)
                .map(compact_path_display)?;
            let content_lines = object
                .get("content")
                .and_then(serde_json::Value::as_str)
                .map(count_display_lines)
                .unwrap_or(0);
            Some(if content_lines > 0 {
                format!("{file_path} (+{content_lines} lines)")
            } else {
                file_path
            })
        }
        "Edit" => object
            .get("file_path")
            .and_then(serde_json::Value::as_str)
            .map(compact_path_display),
        "MultiEdit" => {
            let file_path = object
                .get("file_path")
                .and_then(serde_json::Value::as_str)
                .map(compact_path_display)?;
            let edits = object
                .get("edits")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            Some(format!("{file_path} ({edits} edits)"))
        }
        _ => None,
    }
}

fn count_display_lines(content: &str) -> usize {
    let count = content.lines().count();
    if content.ends_with('\n') {
        count
    } else {
        count.max(1)
    }
}

fn format_tool_value(key: &str, value: &serde_json::Value) -> String {
    let raw = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    let sanitized = sanitize_agent_text(&raw);
    if key.ends_with("path") || key == "path" {
        return compact_path_display(&sanitized);
    }
    truncate_tool_value(&sanitized, 80)
}

fn compact_path_display(path: &str) -> String {
    let path = Path::new(path);
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();

    if components.len() <= 3 {
        return components.join("/");
    }

    components[components.len().saturating_sub(3)..].join("/")
}

fn format_claude_tool_diff(name: &str, input: &str) -> Option<String> {
    if !matches!(name, "Write" | "Edit" | "MultiEdit") {
        return None;
    }

    let value = serde_json::from_str::<serde_json::Value>(input.trim()).ok()?;
    match name {
        "Write" => {
            let file_path = value.get("file_path").and_then(serde_json::Value::as_str)?;
            let content = value.get("content").and_then(serde_json::Value::as_str)?;
            let original = std::fs::read_to_string(file_path).unwrap_or_default();
            Some(if original.is_empty() {
                format_create_preview(file_path, content)
            } else {
                format_compact_diff(file_path, &original, content)
            })
        }
        "Edit" => {
            let file_path = value.get("file_path").and_then(serde_json::Value::as_str)?;
            let old = value
                .get("old_string")
                .and_then(serde_json::Value::as_str)?;
            let new = value
                .get("new_string")
                .and_then(serde_json::Value::as_str)?;
            Some(format_compact_diff(file_path, old, new))
        }
        "MultiEdit" => {
            let file_path = value.get("file_path").and_then(serde_json::Value::as_str)?;
            let edits = value
                .get("edits")
                .and_then(serde_json::Value::as_array)
                .map(|edits| edits.len())
                .unwrap_or(0);
            Some(format!(
                "    {} {}",
                agent_dim("diff"),
                agent_dim(format!(
                    "{edits} edits in {}",
                    compact_path_display(file_path)
                ))
            ))
        }
        _ => None,
    }
}

fn format_compact_diff(file_path: &str, original: &str, modified: &str) -> String {
    if original == modified {
        return format!(
            "    {} {}",
            agent_dim("diff"),
            agent_dim(format!("no changes in {}", compact_path_display(file_path)))
        );
    }

    let diff = TextDiff::from_lines(original, modified);
    let mut output = format!(
        "    {} {}\n",
        agent_dim("diff"),
        agent_dim(compact_path_display(file_path))
    );
    let mut line_count = 0usize;
    let max_lines = 8usize;

    'hunks: for hunk in diff.unified_diff().context_radius(1).iter_hunks() {
        let header = hunk.header().to_string();
        output.push_str("    ");
        output.push_str(&agent_accent(header.trim_end()));
        output.push('\n');

        for change in hunk.iter_changes() {
            if line_count >= max_lines {
                output.push_str("    ");
                output.push_str(&agent_dim("... diff truncated"));
                output.push('\n');
                break 'hunks;
            }

            let (sign, styled) = match change.tag() {
                ChangeTag::Delete => ("-", agent_style(change.to_string_lossy(), "31")),
                ChangeTag::Insert => ("+", agent_style(change.to_string_lossy(), "32")),
                ChangeTag::Equal => (" ", agent_dim(change.to_string_lossy())),
            };
            output.push_str("    ");
            output.push_str(sign);
            output.push_str(&styled);
            if !output.ends_with('\n') {
                output.push('\n');
            }
            line_count += 1;
        }
    }

    output.trim_end().to_string()
}

fn format_create_preview(file_path: &str, content: &str) -> String {
    let line_count = count_display_lines(content);
    let mut output = format!(
        "    {} {}\n",
        agent_dim("creates"),
        agent_dim(format!(
            "{} (+{} lines)",
            compact_path_display(file_path),
            line_count
        ))
    );

    let mut shown = 0usize;
    for line in content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(4)
    {
        output.push_str("    ");
        output.push_str(&agent_style(
            format!("+{}", truncate_tool_value(line, 120)),
            "32",
        ));
        output.push('\n');
        shown += 1;
    }

    if shown == 0 {
        output.push_str("    ");
        output.push_str(&agent_dim("(empty file)"));
        output.push('\n');
    } else if content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > shown
    {
        output.push_str("    ");
        output.push_str(&agent_dim("... preview truncated"));
        output.push('\n');
    }

    output.trim_end().to_string()
}

fn truncate_tool_value(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_count = compact.chars().count();
    if char_count <= max_chars {
        return compact;
    }

    let mut truncated = compact
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

pub(crate) fn write_agent_spinner_live(agent_name: &str, tick: usize) {
    use std::io::Write;

    if !std::io::stderr().is_terminal() {
        return;
    }

    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let frame = FRAMES[tick % FRAMES.len()];
    let line = format!(
        "\r\x1b[2K  {} {}",
        agent_accent(frame),
        agent_dim(format!("{agent_name} is thinking..."))
    );
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "{line}");
    let _ = stderr.flush();
}

pub(crate) fn clear_agent_spinner_live() {
    use std::io::Write;

    if !std::io::stderr().is_terminal() {
        return;
    }

    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "\r\x1b[2K");
    let _ = stderr.flush();
}

pub(crate) fn write_agent_wait_notice_live(line: &str) {
    use std::io::Write;

    if !std::io::stderr().is_terminal() {
        eprintln!("{line}");
        return;
    }

    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "\r\x1b[2K{line}");
    let _ = stderr.flush();
}

pub(crate) fn write_agent_stream_render_live(
    stream: &str,
    render: &AgentStreamRender,
    open_fragment: &AtomicBool,
    spinner_active: &AtomicBool,
) {
    match render {
        AgentStreamRender::Suppress => {}
        AgentStreamRender::Line(line) => {
            if spinner_active.swap(false, Ordering::Relaxed) {
                clear_agent_spinner_live();
            }
            if open_fragment.swap(false, Ordering::Relaxed) {
                write_agent_stream_fragment_live(stream, "\n");
            }
            write_agent_stream_line_live(stream, line);
        }
        AgentStreamRender::Fragment(fragment) => {
            if !fragment.is_empty() {
                if spinner_active.swap(false, Ordering::Relaxed) {
                    clear_agent_spinner_live();
                }
                let was_open = open_fragment.load(Ordering::Relaxed);
                let (formatted, is_open) = format_agent_message_fragment(fragment, was_open);
                open_fragment.store(is_open, Ordering::Relaxed);
                write_agent_stream_fragment_live(stream, &formatted);
            }
        }
    }
}

fn format_agent_message_fragment(fragment: &str, already_open: bool) -> (String, bool) {
    let mut output = String::new();
    let mut line_open = already_open;

    for segment in fragment.split_inclusive('\n') {
        if !line_open && segment != "\n" {
            output.push_str(&agent_message_prefix());
        }
        output.push_str(segment);
        line_open = !segment.ends_with('\n');
    }

    if !fragment.ends_with('\n') && output.is_empty() {
        line_open = already_open;
    }

    (output, line_open)
}

pub(crate) fn should_stream_agent_output_live(quiet: bool, logger: &StructuredLogger) -> bool {
    !quiet && !logger.is_jsonl()
}

pub(crate) fn should_emit_agent_wait_notice(quiet: bool, logger: &StructuredLogger) -> bool {
    !quiet && !logger.is_jsonl()
}

fn write_agent_stream_line_live(stream: &str, line: &str) {
    use std::io::Write;

    match stream {
        "stderr" => {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{line}");
            let _ = stderr.flush();
        }
        _ => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{line}");
            let _ = stdout.flush();
        }
    }
}

pub(crate) fn write_agent_stream_fragment_live(stream: &str, fragment: &str) {
    use std::io::Write;

    match stream {
        "stderr" => {
            let mut stderr = std::io::stderr().lock();
            let _ = write!(stderr, "{fragment}");
            let _ = stderr.flush();
        }
        _ => {
            let mut stdout = std::io::stdout().lock();
            let _ = write!(stdout, "{fragment}");
            let _ = stdout.flush();
        }
    }
}

mod tests {
    #[allow(unused_imports)]
    use crate::ai_agent_stream::{
        format_agent_message_fragment, format_agent_stream_line, format_claude_tool_call,
        AgentStreamRender, ClaudeStreamRenderer, CodexStreamRenderer, OpenCodeStreamRenderer,
    };

    #[test]
    fn codex_agent_output_is_not_tagged_as_stderr() {
        assert_eq!(
            format_agent_stream_line("codex", "stderr", "hello".to_string()),
            "hello"
        );
        assert_eq!(
            format_agent_stream_line("claude-code", "stdout", "hello".to_string()),
            "hello"
        );
        assert_eq!(
            format_agent_stream_line("opencode", "stderr", "hello".to_string()),
            "hello"
        );
        assert_eq!(
            format_agent_stream_line("aider", "stderr", "hello".to_string()),
            "[stderr] hello"
        );
    }

    #[test]
    fn claude_stream_json_is_rendered_for_humans() {
        let mut renderer = ClaudeStreamRenderer::default();
        assert_eq!(
            renderer.render_line(r#"{"type":"system","subtype":"init","model":"sonnet"}"#),
            Some(AgentStreamRender::Line("  • started sonnet".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"Read"}}}"#
            ),
            Some(AgentStreamRender::Suppress)
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"README.md\"}"}}}"#
            ),
            Some(AgentStreamRender::Suppress)
        );
        assert_eq!(
            renderer
                .render_line(r#"{"type":"stream_event","event":{"type":"content_block_stop"}}"#),
            Some(AgentStreamRender::Line("  • Read README.md".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}}"#
            ),
            Some(AgentStreamRender::Fragment("hello".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"seven_day","utilization":0.66}}"#
            ),
            Some(AgentStreamRender::Line(
                "  ! rate limit allowed warning (seven day, 66% used)".to_string()
            ))
        );
        assert_eq!(
            renderer.render_line(r#"{"type":"assistant","message":{"content":[]}}"#),
            Some(AgentStreamRender::Suppress)
        );
    }

    #[test]
    fn opencode_stream_json_is_rendered_for_humans() {
        let mut renderer = OpenCodeStreamRenderer::default();
        assert_eq!(
            renderer.render_line(
                r#"{"type":"step_start","sessionID":"ses","part":{"id":"p0","type":"step-start"}}"#
            ),
            Some(AgentStreamRender::Suppress)
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"text","sessionID":"ses","part":{"id":"cli-text","type":"text","text":"hi"}}"#
            ),
            Some(AgentStreamRender::Line("  › hi".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"tool_use","sessionID":"ses","part":{"id":"cli-tool","type":"tool","callID":"call0","tool":"read","state":{"status":"completed","input":{"filePath":"README.md"},"title":"Read README.md","output":"","metadata":{}}}}"#
            ),
            Some(AgentStreamRender::Line("  • Read README.md".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"tool_use","sessionID":"ses","part":{"id":"cli-edit","type":"tool","callID":"call-edit","tool":"apply_patch","state":{"status":"completed","input":{"patchText":"*** Begin Patch"},"title":"Success. Updated the following files:\nM package.json","output":"","metadata":{"files":[{"relativePath":"package.json"}],"diff":"Index: package.json\n--- package.json\n+++ package.json\n@@ -1 +1 @@\n-before\n+after\n"}}}}"#
            ),
            Some(AgentStreamRender::Line(
                "  • Edit package.json\n    diff package.json\n    @@ -1 +1 @@\n    -before\n    +after".to_string()
            ))
        );

        let mut renderer = OpenCodeStreamRenderer::default();
        assert_eq!(
            renderer.render_line(
                r#"{"type":"message.updated","properties":{"info":{"role":"assistant","modelID":"gpt-5.4"}}}"#
            ),
            Some(AgentStreamRender::Line("  • started gpt-5.4".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"message.part.delta","properties":{"partID":"p1","field":"text","delta":"hello"}}"#
            ),
            Some(AgentStreamRender::Fragment("hello".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"message.part.updated","properties":{"part":{"id":"p1","type":"text","text":"hello world"}}}"#
            ),
            Some(AgentStreamRender::Fragment(" world".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"message.part.updated","properties":{"part":{"id":"t1","type":"tool","callID":"call1","tool":"Read","state":{"status":"completed","input":{"filePath":"README.md"},"title":"Read README.md","output":"","metadata":{}}}}}"#
            ),
            Some(AgentStreamRender::Line("  • Read README.md".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"message.part.updated","properties":{"part":{"id":"t1","type":"tool","callID":"call1","tool":"Read","state":{"status":"completed","input":{"filePath":"README.md"},"title":"Read README.md","output":"","metadata":{}}}}}"#
            ),
            Some(AgentStreamRender::Suppress)
        );
    }

    #[test]
    fn codex_stream_json_is_rendered_for_humans() {
        let mut renderer = CodexStreamRenderer::default();
        assert_eq!(
            renderer.render_line(r#"{"type":"thread.started","thread_id":"t"}"#),
            Some(AgentStreamRender::Suppress)
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"hello\nworld"}}"#
            ),
            Some(AgentStreamRender::Line("  › hello\n  › world".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"item.started","item":{"id":"item_2","type":"command_execution","command":"/bin/zsh -lc \"sed -n '1,40p' package.json\"","status":"in_progress"}}"#
            ),
            Some(AgentStreamRender::Line(
                "  • Bash command=/bin/zsh -lc \"sed -n '1,40p' package.json\"".to_string()
            ))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"item.completed","item":{"id":"item_2","type":"command_execution","command":"/bin/zsh -lc \"sed -n '1,40p' package.json\"","status":"completed"}}"#
            ),
            Some(AgentStreamRender::Suppress)
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"item.completed","item":{"id":"item_3","type":"file_change","changes":[{"path":"/tmp/package.json","kind":"update"}],"status":"completed"}}"#
            ),
            Some(AgentStreamRender::Line("  • Edit tmp/package.json".to_string()))
        );
        assert_eq!(
            renderer.render_line(
                r#"{"type":"item.completed","item":{"id":"item_4","type":"error","message":"bad config"}}"#
            ),
            Some(AgentStreamRender::Line("  ! error bad config".to_string()))
        );
    }

    #[test]
    fn agent_message_fragments_are_labeled() {
        assert_eq!(
            format_agent_message_fragment("hello", false),
            ("  › hello".to_string(), true)
        );
        assert_eq!(
            format_agent_message_fragment(" world\nnext", true),
            (" world\n  › next".to_string(), true)
        );
    }

    #[test]
    fn claude_write_tool_renders_compact_diff() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("summary.md");
        std::fs::write(&file_path, "old\nsame\n").unwrap();
        let input = serde_json::json!({
            "file_path": file_path,
            "content": "new\nsame\n"
        })
        .to_string();

        let line = format_claude_tool_call("Write", &input);

        assert!(line.contains("Write"));
        assert!(line.contains("summary.md"));
        assert!(line.contains("diff"));
        assert!(line.contains("-old"));
        assert!(line.contains("+new"));
    }
}
