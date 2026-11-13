use codemod_agents::{AgentEvent, FileChange, FileChangeKind};
use console::style;
use similar::{ChangeTag, TextDiff};

pub(crate) enum RenderedAgentEvent {
    Line(String),
    Fragment(String),
}

/// How to show one thing a coding agent did. `line_open` says whether the
/// previous fragment left a line open.
pub(crate) fn render_agent_activity(
    activity: &AgentEvent,
    line_open: bool,
) -> Option<RenderedAgentEvent> {
    match activity {
        AgentEvent::Started { model, .. } => Some(RenderedAgentEvent::Line(match model {
            Some(model) => format!("  {} started {}", style("•").cyan(), style(model).dim()),
            None => format!("  {} started", style("•").cyan()),
        })),
        AgentEvent::Text { text, delta: true } => Some(RenderedAgentEvent::Fragment(
            format_message_fragment(text, line_open, true),
        )),
        AgentEvent::Text { text, delta: false } => {
            let text = text.trim_end();
            (!text.is_empty()).then(|| {
                RenderedAgentEvent::Line(
                    format_message_fragment(text, false, true)
                        .trim_end()
                        .to_string(),
                )
            })
        }
        AgentEvent::Thinking { .. } => None,
        AgentEvent::ToolCall {
            name,
            input,
            changes,
            ..
        } => {
            let params = Some(input)
                .filter(|params| !params.is_null())
                .filter(|params| !params.as_object().is_some_and(|object| object.is_empty()))
                .map(compact_tool_params)
                .unwrap_or_default();
            let mut line = if params.is_empty() {
                format!("  {} {}", style("•").green(), style(name).bold())
            } else {
                format!(
                    "  {} {} {}",
                    style("•").green(),
                    style(name).bold(),
                    style(params).dim()
                )
            };
            if let Some(diff) = format_file_changes(changes, true) {
                line.push('\n');
                line.push_str(&diff);
            }
            Some(RenderedAgentEvent::Line(line))
        }
        AgentEvent::ToolResult {
            output,
            is_error: true,
            ..
        } => {
            let first = output.lines().next().unwrap_or("the tool failed");
            Some(RenderedAgentEvent::Line(format!(
                "  {} {}",
                style("!").yellow(),
                style(truncate_tool_value(first, 160)).dim()
            )))
        }
        AgentEvent::ToolResult { .. } => None,
        AgentEvent::Warning { message } => Some(RenderedAgentEvent::Line(format!(
            "  {} {}",
            style("!").yellow(),
            message
        ))),
        AgentEvent::Error { message } => Some(RenderedAgentEvent::Line(format!(
            "  {} {}",
            style("✗").red(),
            message
        ))),
        AgentEvent::Output { line, .. } => {
            Some(RenderedAgentEvent::Line(format!("  {}", style(line).dim())))
        }
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

/// The preview under a tool call that changes files: the change itself for
/// one, a count for several.
fn format_file_changes(changes: &[FileChange], styled: bool) -> Option<String> {
    let summary = |text: String| format!("    {} {}", dim("diff", styled), dim(&text, styled));
    let first = changes.first()?;
    let path = compact_path_display(&first.path);
    if changes.len() > 1 {
        let files = changes
            .iter()
            .map(|change| change.path.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        return Some(summary(if files == 1 {
            format!("{} edits in {path}", changes.len())
        } else {
            format!("{files} files, first {path}")
        }));
    }

    if let Some(diff) = first.diff.as_deref().filter(|diff| !diff.trim().is_empty()) {
        return Some(format_unified_diff_preview(&first.path, diff, styled));
    }
    Some(
        match (first.kind, first.old.as_deref(), first.new.as_deref()) {
            (FileChangeKind::Delete, Some(old), _) => {
                format_compact_diff(&first.path, old, "", styled)
            }
            (FileChangeKind::Create, _, Some(new)) => {
                format_create_preview(&first.path, new, styled)
            }
            (FileChangeKind::Write, _, Some(new)) => format_write_preview(&first.path, new, styled),
            (_, Some(old), Some(new)) => format_compact_diff(&first.path, old, new, styled),
            (FileChangeKind::Create, ..) => summary(format!("creates {path}")),
            (FileChangeKind::Delete, ..) => summary(format!("deletes {path}")),
            (FileChangeKind::Write | FileChangeKind::Update, ..) => {
                summary(format!("changed {path}"))
            }
        },
    )
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

/// A unified diff, cut short. `file_path` is used when the diff names no file.
fn format_unified_diff_preview(file_path: &str, diff: &str, styled: bool) -> String {
    let mut file_path = file_path;
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

#[cfg(test)]
mod tests {
    use super::{render_agent_activity, RenderedAgentEvent};
    use codemod_agents::{AgentEvent, FileChange, FileChangeKind};
    use serde_json::json;

    fn line(activity: AgentEvent) -> String {
        match render_agent_activity(&activity, false).expect("rendered") {
            RenderedAgentEvent::Line(line) => console::strip_ansi_codes(&line).into_owned(),
            RenderedAgentEvent::Fragment(_) => panic!("expected a line"),
        }
    }

    #[test]
    fn delta_text_renders_as_fragments_without_repeating_prefix() {
        let first = render_agent_activity(
            &AgentEvent::Text {
                text: "Ren".into(),
                delta: true,
            },
            false,
        )
        .expect("first fragment");
        let second = render_agent_activity(
            &AgentEvent::Text {
                text: "amed to `calcom`.".into(),
                delta: true,
            },
            true,
        )
        .expect("second fragment");

        assert!(matches!(first, RenderedAgentEvent::Fragment(text) if text.contains("Ren")));
        assert!(
            matches!(second, RenderedAgentEvent::Fragment(text) if text == "amed to `calcom`.")
        );
    }

    fn change(path: &str, kind: FileChangeKind) -> FileChange {
        FileChange {
            path: path.into(),
            kind,
            old: None,
            new: None,
            diff: None,
        }
    }

    /// A tool call by an agent whose tool names and input mean nothing here.
    fn tool_call(changes: Vec<FileChange>) -> String {
        line(AgentEvent::ToolCall {
            id: None,
            name: "some_agents_file_tool".into(),
            input: json!({ "target": "package.json" }),
            changes,
        })
    }

    #[test]
    fn a_replacement_renders_as_a_compact_diff() {
        let line = tool_call(vec![FileChange {
            old: Some("\"name\": \"calcom-monorepo\"".into()),
            new: Some("\"name\": \"calcom\"".into()),
            ..change("package.json", FileChangeKind::Update)
        }]);
        assert!(line.contains("diff package.json"), "{line}");
        assert!(line.contains("-\"name\": \"calcom-monorepo\""));
        assert!(line.contains("+\"name\": \"calcom\""));
    }

    #[test]
    fn written_and_created_files_render_with_their_size() {
        let written = tool_call(vec![FileChange {
            new: Some("a\nb\n".into()),
            ..change("src/lib/new.ts", FileChangeKind::Write)
        }]);
        assert!(
            written.contains("writes src/lib/new.ts (2 lines)"),
            "{written}"
        );

        let created = tool_call(vec![FileChange {
            new: Some("first line\n".into()),
            ..change("new.ts", FileChangeKind::Create)
        }]);
        assert!(created.contains("creates new.ts (+1 lines)"), "{created}");
        assert!(created.contains("+first line"), "{created}");
    }

    #[test]
    fn a_unified_diff_renders_as_given() {
        let line = tool_call(vec![FileChange {
            diff: Some("--- a/x.ts\n+++ b/x.ts\n@@ -1 +1 @@\n-old\n+new\n".into()),
            ..change("/work/x.ts", FileChangeKind::Update)
        }]);
        assert!(line.contains("diff x.ts"), "{line}");
        assert!(line.contains("-old") && line.contains("+new"), "{line}");
    }

    #[test]
    fn changes_without_content_and_lists_of_changes_render_as_a_summary() {
        let deleted = tool_call(vec![change("old.ts", FileChangeKind::Delete)]);
        assert!(deleted.contains("deletes old.ts"), "{deleted}");
        let updated = tool_call(vec![change("a.ts", FileChangeKind::Update)]);
        assert!(updated.contains("changed a.ts"), "{updated}");

        let files = tool_call(vec![
            change("a.ts", FileChangeKind::Update),
            change("b.ts", FileChangeKind::Create),
        ]);
        assert!(files.contains("2 files, first a.ts"), "{files}");
        let edits = tool_call(vec![
            change("a.ts", FileChangeKind::Update),
            change("a.ts", FileChangeKind::Update),
        ]);
        assert!(edits.contains("2 edits in a.ts"), "{edits}");
    }

    #[test]
    fn a_tool_call_that_changes_no_file_renders_on_one_line() {
        let line = line(AgentEvent::ToolCall {
            id: None,
            name: "Bash".into(),
            input: json!({ "command": "ls" }),
            changes: Vec::new(),
        });
        assert_eq!(line.trim(), "• Bash command=ls");
    }

    #[test]
    fn failures_and_errors_render_and_successful_results_do_not() {
        assert!(line(AgentEvent::ToolResult {
            id: None,
            output: "permission denied\nmore".into(),
            is_error: true,
        })
        .contains("permission denied"));
        assert!(render_agent_activity(
            &AgentEvent::ToolResult {
                id: None,
                output: "ok".into(),
                is_error: false,
            },
            false
        )
        .is_none());
        assert!(line(AgentEvent::Error {
            message: "rate limited".into()
        })
        .contains("rate limited"));
    }
}
