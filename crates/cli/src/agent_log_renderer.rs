use console::style;
use similar::{ChangeTag, TextDiff};

pub(crate) enum RenderedAgentEvent {
    Line(String),
    Fragment(String),
}

pub(crate) fn render_agent_event_payload_styled(
    payload: &str,
    line_open: bool,
) -> Option<RenderedAgentEvent> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    match value.get("event").and_then(serde_json::Value::as_str)? {
        "message" => {
            let text = value.get("text").and_then(serde_json::Value::as_str)?;
            let delta = value
                .get("delta")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if delta {
                Some(RenderedAgentEvent::Fragment(format_message_fragment(
                    text, line_open, true,
                )))
            } else {
                Some(RenderedAgentEvent::Line(format!(
                    "  {} {}",
                    style("›").cyan(),
                    text
                )))
            }
        }
        "tool_call" => {
            let tool_name = value
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool");
            let params = value
                .get("tool_params")
                .filter(|params| !params.as_object().is_some_and(|object| object.is_empty()))
                .map(compact_tool_params)
                .unwrap_or_default();
            let mut line = if params.is_empty() {
                format!("  {} {}", style("•").green(), style(tool_name).bold())
            } else {
                format!(
                    "  {} {} {}",
                    style("•").green(),
                    style(tool_name).bold(),
                    style(params).dim()
                )
            };
            if let Some(diff) = value
                .get("tool_params")
                .and_then(|params| format_tool_diff(tool_name, params, true))
            {
                line.push('\n');
                line.push_str(&diff);
            }
            Some(RenderedAgentEvent::Line(line))
        }
        "warning" => {
            let message = value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("warning");
            Some(RenderedAgentEvent::Line(format!(
                "  {} {}",
                style("!").yellow(),
                message
            )))
        }
        "status" => {
            let status = value
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("status");
            let detail = value
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if detail.is_empty() {
                Some(RenderedAgentEvent::Line(format!(
                    "  {} {}",
                    style("•").cyan(),
                    status
                )))
            } else {
                Some(RenderedAgentEvent::Line(format!(
                    "  {} {} {}",
                    style("•").cyan(),
                    status,
                    style(detail).dim()
                )))
            }
        }
        "raw" => value
            .get("line")
            .and_then(serde_json::Value::as_str)
            .map(|line| RenderedAgentEvent::Line(line.to_string())),
        _ => None,
    }
}

pub(crate) fn render_task_log_line(line: &str) -> String {
    render_agent_event_payload_plain(line).unwrap_or_else(|| line.to_string())
}

pub(crate) fn render_task_log_lines(lines: &[String]) -> Vec<String> {
    let mut rendered = Vec::new();
    let mut message_buffer = String::new();

    for line in lines {
        if let Some(delta) = agent_message_delta(line) {
            message_buffer.push_str(&delta);
            continue;
        }

        flush_message_buffer(&mut rendered, &mut message_buffer);
        rendered.push(render_task_log_line(line));
    }

    flush_message_buffer(&mut rendered, &mut message_buffer);
    rendered
}

pub(crate) fn render_agent_message_fragment(fragment: &str, already_open: bool) -> String {
    format_message_fragment(fragment, already_open, true)
}

fn render_agent_event_payload_plain(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    match value.get("event").and_then(serde_json::Value::as_str)? {
        "message" => {
            let text = value.get("text").and_then(serde_json::Value::as_str)?;
            Some(format!("  › {text}"))
        }
        "tool_call" => {
            let tool_name = value
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool");
            let params = value
                .get("tool_params")
                .filter(|params| !params.as_object().is_some_and(|object| object.is_empty()))
                .map(compact_tool_params)
                .unwrap_or_default();
            let mut line = if params.is_empty() {
                format!("  • {tool_name}")
            } else {
                format!("  • {tool_name} {params}")
            };
            if let Some(diff) = value
                .get("tool_params")
                .and_then(|params| format_tool_diff(tool_name, params, false))
            {
                line.push('\n');
                line.push_str(&diff);
            }
            Some(line)
        }
        "warning" => {
            let message = value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("warning");
            Some(format!("  ! {message}"))
        }
        "status" => {
            let status = value
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("status");
            let detail = value
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if detail.is_empty() {
                Some(format!("  • {status}"))
            } else {
                Some(format!("  • {status} {detail}"))
            }
        }
        "raw" => value
            .get("line")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn compact_tool_params(value: &serde_json::Value) -> String {
    if let Some(summary) = summarize_known_tool_params(value) {
        return summary;
    }
    match value {
        serde_json::Value::Object(object) => object
            .iter()
            .filter_map(|(key, value)| compact_param(key, value))
            .take(3)
            .collect::<Vec<_>>()
            .join(" "),
        other => truncate_tool_value(&other.to_string(), 120),
    }
}

fn format_message_fragment(fragment: &str, already_open: bool, styled: bool) -> String {
    let mut output = String::new();
    let mut line_open = already_open;
    let prefix = if styled {
        format!("  {} ", style("›").cyan())
    } else {
        "  › ".to_string()
    };

    for segment in fragment.split_inclusive('\n') {
        if !line_open && segment != "\n" {
            output.push_str(&prefix);
        }
        output.push_str(segment);
        line_open = !segment.ends_with('\n');
    }

    output
}

fn format_tool_diff(tool_name: &str, params: &serde_json::Value, styled: bool) -> Option<String> {
    if let Some(diff) = params
        .pointer("/metadata/diff")
        .and_then(serde_json::Value::as_str)
        .filter(|diff| !diff.trim().is_empty())
    {
        return Some(format_unified_diff_preview(diff, styled));
    }

    match tool_name {
        "Write" | "Edit" | "Delete" if params.get("changes").is_some() => {
            format_codex_changes_diff(params, styled)
        }
        "Write" => {
            let file_path = params
                .get("file_path")
                .and_then(serde_json::Value::as_str)?;
            let content = params.get("content").and_then(serde_json::Value::as_str)?;
            Some(format_write_preview(file_path, content, styled))
        }
        "Edit" => {
            let file_path = params
                .get("file_path")
                .and_then(serde_json::Value::as_str)?;
            let old = params
                .get("old_string")
                .and_then(serde_json::Value::as_str)?;
            let new = params
                .get("new_string")
                .and_then(serde_json::Value::as_str)?;
            Some(format_compact_diff(file_path, old, new, styled))
        }
        "MultiEdit" => {
            let file_path = params
                .get("file_path")
                .and_then(serde_json::Value::as_str)?;
            let edits = params
                .get("edits")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            Some(format!(
                "    {} {}",
                dim("diff", styled),
                dim(
                    &format!("{edits} edits in {}", compact_path_display(file_path)),
                    styled
                )
            ))
        }
        _ => None,
    }
}

fn format_codex_changes_diff(params: &serde_json::Value, styled: bool) -> Option<String> {
    let changes = params
        .get("changes")
        .and_then(serde_json::Value::as_array)?;
    if changes.len() != 1 {
        let files = params
            .get("files")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(changes.len() as u64);
        let path = params
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("files");
        return Some(format!(
            "    {} {}",
            dim("diff", styled),
            dim(
                &format!("{files} files, first {}", compact_path_display(path)),
                styled
            )
        ));
    }

    let change = changes.first()?;
    let path = change.get("path").and_then(serde_json::Value::as_str)?;
    if let Some(diff) = change
        .get("diff")
        .or_else(|| change.get("unified_diff"))
        .or_else(|| change.get("patch"))
        .and_then(serde_json::Value::as_str)
        .filter(|diff| !diff.trim().is_empty())
    {
        return Some(format_unified_diff_preview(diff, styled));
    }

    let kind = change
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("update");
    let original = change
        .get("old")
        .or_else(|| change.get("before"))
        .or_else(|| change.get("previous"))
        .and_then(serde_json::Value::as_str);
    let modified = change
        .get("new")
        .or_else(|| change.get("after"))
        .or_else(|| change.get("content"))
        .and_then(serde_json::Value::as_str);

    match (kind, original, modified) {
        ("delete" | "remove", Some(original), _) => {
            Some(format_compact_diff(path, original, "", styled))
        }
        ("add" | "create", _, Some(modified)) => {
            Some(format_create_preview(path, modified, styled))
        }
        (_, Some(original), Some(modified)) => {
            Some(format_compact_diff(path, original, modified, styled))
        }
        _ => Some(format!(
            "    {} {}",
            dim("diff", styled),
            dim(&format!("changed {}", compact_path_display(path)), styled)
        )),
    }
}

fn format_compact_diff(file_path: &str, original: &str, modified: &str, styled: bool) -> String {
    if original == modified {
        return format!(
            "    {} {}",
            dim("diff", styled),
            dim(
                &format!("no changes in {}", compact_path_display(file_path)),
                styled
            )
        );
    }

    let diff = TextDiff::from_lines(original, modified);
    let mut output = format!(
        "    {} {}\n",
        dim("diff", styled),
        dim(&compact_path_display(file_path), styled)
    );
    let mut line_count = 0usize;
    let max_lines = 8usize;

    'hunks: for hunk in diff.unified_diff().context_radius(1).iter_hunks() {
        output.push_str("    ");
        output.push_str(&accent(hunk.header().to_string().trim_end(), styled));
        output.push('\n');

        for change in hunk.iter_changes() {
            if line_count >= max_lines {
                output.push_str("    ");
                output.push_str(&dim("... diff truncated", styled));
                output.push('\n');
                break 'hunks;
            }

            let (sign, rendered) = match change.tag() {
                ChangeTag::Delete => ("-", removed(&change.to_string_lossy(), styled)),
                ChangeTag::Insert => ("+", added(&change.to_string_lossy(), styled)),
                ChangeTag::Equal => (" ", dim(&change.to_string_lossy(), styled)),
            };
            output.push_str("    ");
            output.push_str(sign);
            output.push_str(&rendered);
            if !output.ends_with('\n') {
                output.push('\n');
            }
            line_count += 1;
        }
    }

    output.trim_end().to_string()
}

fn format_write_preview(file_path: &str, content: &str, styled: bool) -> String {
    let line_count = count_display_lines(content);
    format!(
        "    {} {}",
        dim("writes", styled),
        dim(
            &format!("{} ({} lines)", compact_path_display(file_path), line_count),
            styled
        )
    )
}

fn format_create_preview(file_path: &str, content: &str, styled: bool) -> String {
    let line_count = count_display_lines(content);
    let mut output = format!(
        "    {} {}\n",
        dim("creates", styled),
        dim(
            &format!(
                "{} (+{} lines)",
                compact_path_display(file_path),
                line_count
            ),
            styled
        )
    );

    let mut shown = 0usize;
    for line in content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(4)
    {
        output.push_str("    ");
        output.push_str(&added(
            &format!("+{}", truncate_tool_value(line, 120)),
            styled,
        ));
        output.push('\n');
        shown += 1;
    }

    if shown == 0 {
        output.push_str("    ");
        output.push_str(&dim("(empty file)", styled));
        output.push('\n');
    } else if content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > shown
    {
        output.push_str("    ");
        output.push_str(&dim("... preview truncated", styled));
        output.push('\n');
    }

    output.trim_end().to_string()
}

fn format_unified_diff_preview(diff: &str, styled: bool) -> String {
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
        dim("diff", styled),
        dim(&compact_path_display(file_path), styled)
    );
    let mut line_count = 0usize;
    let max_lines = 8usize;

    for line in body {
        if line.starts_with("@@") {
            output.push_str("    ");
            output.push_str(&accent(line, styled));
            output.push('\n');
            continue;
        }
        if line_count >= max_lines {
            output.push_str("    ");
            output.push_str(&dim("... diff truncated", styled));
            output.push('\n');
            break;
        }
        output.push_str("    ");
        if line.starts_with('-') {
            output.push_str(&removed(line, styled));
        } else if line.starts_with('+') {
            output.push_str(&added(line, styled));
        } else {
            output.push_str(&dim(line, styled));
        }
        output.push('\n');
        line_count += 1;
    }

    output.trim_end().to_string()
}

fn count_display_lines(content: &str) -> usize {
    let count = content.lines().count();
    if content.ends_with('\n') {
        count
    } else {
        count.max(1)
    }
}

fn accent(text: &str, styled: bool) -> String {
    if styled {
        style(text).cyan().to_string()
    } else {
        text.to_string()
    }
}

fn dim(text: &str, styled: bool) -> String {
    if styled {
        style(text).dim().to_string()
    } else {
        text.to_string()
    }
}

fn added(text: &str, styled: bool) -> String {
    if styled {
        style(text).green().to_string()
    } else {
        text.to_string()
    }
}

fn removed(text: &str, styled: bool) -> String {
    if styled {
        style(text).red().to_string()
    } else {
        text.to_string()
    }
}

fn summarize_known_tool_params(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    for key in [
        "file_path",
        "filePath",
        "path",
        "command",
        "cmd",
        "description",
    ] {
        if let Some(value) = object.get(key) {
            return compact_param(key, value);
        }
    }
    let title = object
        .get("title")
        .and_then(serde_json::Value::as_str)
        .filter(|title| !title.trim().is_empty())?;
    Some(truncate_tool_value(
        title.lines().next().unwrap_or(title),
        120,
    ))
}

fn compact_param(key: &str, value: &serde_json::Value) -> Option<String> {
    if key == "content" || key == "changes" || key == "metadata" || key == "input" {
        return None;
    }
    let raw = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    if raw.trim().is_empty() {
        return None;
    }
    let display = if key.ends_with("path") || key == "path" {
        compact_path_display(&raw)
    } else {
        truncate_tool_value(&raw, 120)
    };
    Some(format!("{key}={display}"))
}

fn compact_path_display(path: &str) -> String {
    let path = std::path::Path::new(path);
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

fn agent_message_delta(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("event").and_then(serde_json::Value::as_str) != Some("message") {
        return None;
    }
    if !value
        .get("delta")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    value
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn flush_message_buffer(rendered: &mut Vec<String>, message_buffer: &mut String) {
    if message_buffer.trim().is_empty() {
        message_buffer.clear();
        return;
    }
    rendered.push(format!("  › {}", message_buffer.trim_end()));
    message_buffer.clear();
}

#[cfg(test)]
mod tests {
    use super::{render_agent_event_payload_styled, RenderedAgentEvent};

    #[test]
    fn delta_messages_render_as_fragments_without_repeating_prefix() {
        let first = render_agent_event_payload_styled(
            r#"{"agent":"claude-code","stream":"stdout","event":"message","text":"Ren","delta":true}"#,
            false,
        )
        .expect("first fragment");
        let second = render_agent_event_payload_styled(
            r#"{"agent":"claude-code","stream":"stdout","event":"message","text":"amed to `calcom`.","delta":true}"#,
            true,
        )
        .expect("second fragment");

        assert!(matches!(first, RenderedAgentEvent::Fragment(text) if text.contains("Ren")));
        assert!(
            matches!(second, RenderedAgentEvent::Fragment(text) if text == "amed to `calcom`.")
        );
    }

    #[test]
    fn edit_tool_renders_compact_diff() {
        let payload = serde_json::json!({
            "agent": "claude-code",
            "stream": "stderr",
            "event": "tool_call",
            "tool_name": "Edit",
            "tool_params": {
                "file_path": "package.json",
                "old_string": "\"name\": \"calcom-monorepo\"",
                "new_string": "\"name\": \"calcom\""
            }
        })
        .to_string();

        let rendered = render_agent_event_payload_styled(&payload, false).expect("rendered");
        let RenderedAgentEvent::Line(line) = rendered else {
            panic!("expected line");
        };
        assert!(line.contains("diff"));
        assert!(line.contains("-\"name\": \"calcom-monorepo\""));
        assert!(line.contains("+\"name\": \"calcom\""));
    }

    #[test]
    fn task_log_lines_coalesce_message_deltas() {
        let lines = vec![
            r#"{"agent":"claude-code","stream":"stdout","event":"message","text":"Ren","delta":true}"#.to_string(),
            r#"{"agent":"claude-code","stream":"stdout","event":"message","text":"amed to `calcom`.","delta":true}"#.to_string(),
            r#"{"agent":"claude-code","stream":"stderr","event":"tool_call","tool_name":"Read","tool_params":{"file_path":"package.json"}}"#.to_string(),
        ];

        let rendered = super::render_task_log_lines(&lines);

        assert_eq!(rendered[0], "  › Renamed to `calcom`.");
        assert!(rendered[1].contains("Read"));
    }

    #[test]
    fn codex_change_payload_renders_diff() {
        let payload = serde_json::json!({
            "agent": "codex",
            "stream": "stdout",
            "event": "tool_call",
            "tool_name": "Edit",
            "tool_params": {
                "path": "package.json",
                "files": 1,
                "changes": [{
                    "path": "package.json",
                    "kind": "update",
                    "old": "\"name\": \"calcom-monorepo\"",
                    "new": "\"name\": \"calcom\""
                }]
            }
        })
        .to_string();

        let rendered = render_agent_event_payload_styled(&payload, false).expect("rendered");
        let RenderedAgentEvent::Line(line) = rendered else {
            panic!("expected line");
        };
        assert!(line.contains("diff"));
        assert!(line.contains("-\"name\": \"calcom-monorepo\""));
        assert!(line.contains("+\"name\": \"calcom\""));
    }
}
