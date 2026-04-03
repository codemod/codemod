pub mod run_list;
pub mod settings;
pub mod task_list;

use butterflow_core::config::ShellCommandExecutionRequest;
use butterflow_models::{TaskStatus, WorkflowStatus};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use serde::Serialize;

use crate::tui::app::AgentSelectionItem;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum StatusTone {
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusLine {
    pub tone: StatusTone,
    pub message: String,
}

// -- Palette --
// Muted, modern palette using RGB for consistency across terminals.
pub const ACCENT: Color = Color::Rgb(110, 140, 255); // soft blue
pub const GREEN: Color = Color::Rgb(80, 200, 120);
pub const RED: Color = Color::Rgb(240, 80, 80);
pub const YELLOW: Color = Color::Rgb(240, 200, 60);
pub const CYAN: Color = Color::Rgb(80, 210, 220);
pub const DIM: Color = Color::Rgb(100, 100, 110);
pub const SURFACE: Color = Color::Rgb(40, 40, 50); // row highlight bg
pub const TEXT: Color = Color::Rgb(210, 210, 220);
pub const TEXT_MUTED: Color = Color::Rgb(130, 130, 145);
pub const HEADER_BG: Color = Color::Rgb(30, 30, 40);
pub const BODY_BG: Color = Color::Rgb(12, 12, 18);
pub const KEY_BG: Color = Color::Rgb(55, 55, 70);
pub const ERROR_BG: Color = Color::Rgb(82, 36, 36);

/// Get a status icon for a task status
pub fn task_status_icon(status: TaskStatus, error: Option<&str>) -> &'static str {
    if task_is_canceled(status, error) {
        return "\u{2013}";
    }
    match status {
        TaskStatus::Completed => "\u{25cf}",       // ●
        TaskStatus::Failed => "\u{25cf}",          // ●
        TaskStatus::Running => "\u{25cf}",         // ●
        TaskStatus::AwaitingTrigger => "\u{25cb}", // ○
        TaskStatus::Pending => "\u{2219}",         // ∙
        TaskStatus::Blocked => "\u{2219}",         // ∙
        TaskStatus::WontDo => "\u{2013}",          // –
    }
}

/// Get a color for a task status
pub fn task_status_color(status: TaskStatus, error: Option<&str>) -> Color {
    if task_is_canceled(status, error) {
        return DIM;
    }
    match status {
        TaskStatus::Completed => GREEN,
        TaskStatus::Failed => RED,
        TaskStatus::Running => YELLOW,
        TaskStatus::AwaitingTrigger => CYAN,
        TaskStatus::Pending => DIM,
        TaskStatus::Blocked => DIM,
        TaskStatus::WontDo => DIM,
    }
}

/// Human-readable label for task status
pub fn task_status_label(status: TaskStatus, error: Option<&str>) -> &'static str {
    if task_is_canceled(status, error) {
        return "canceled";
    }
    match status {
        TaskStatus::Completed => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Running => "running",
        TaskStatus::AwaitingTrigger => "ready",
        TaskStatus::Pending => "pending",
        TaskStatus::Blocked => "blocked",
        TaskStatus::WontDo => "skipped",
    }
}

fn task_is_canceled(status: TaskStatus, error: Option<&str>) -> bool {
    status == TaskStatus::Failed && error == Some("Canceled by user")
}

/// Get a status icon for a workflow status
pub fn workflow_status_icon(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Completed => "\u{25cf}",
        WorkflowStatus::Failed => "\u{25cf}",
        WorkflowStatus::Running => "\u{25cf}",
        WorkflowStatus::AwaitingTrigger => "\u{25cb}",
        WorkflowStatus::Pending => "\u{2219}",
        WorkflowStatus::Canceled => "\u{2013}",
    }
}

/// Get a color for a workflow status
pub fn workflow_status_color(status: WorkflowStatus) -> Color {
    match status {
        WorkflowStatus::Completed => GREEN,
        WorkflowStatus::Failed => RED,
        WorkflowStatus::Running => YELLOW,
        WorkflowStatus::AwaitingTrigger => CYAN,
        WorkflowStatus::Pending => DIM,
        WorkflowStatus::Canceled => DIM,
    }
}

/// Human-readable label for workflow status
pub fn workflow_status_label(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Completed => "completed",
        WorkflowStatus::Failed => "failed",
        WorkflowStatus::Running => "running",
        WorkflowStatus::AwaitingTrigger => "ready",
        WorkflowStatus::Pending => "pending",
        WorkflowStatus::Canceled => "canceled",
    }
}

/// Format a duration in human-readable form
pub fn format_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Render a keybinding hint: " key " label
pub fn key_hint<'a>(key: &'a str, label: &'a str) -> Vec<Span<'a>> {
    vec![
        Span::styled(
            format!(" {key} "),
            Style::default()
                .bg(KEY_BG)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}  "), Style::default().fg(TEXT_MUTED)),
    ]
}

/// Render a colored keybinding hint
pub fn key_hint_colored<'a>(key: &'a str, label: &'a str, color: Color) -> Vec<Span<'a>> {
    vec![
        Span::styled(
            format!(" {key} "),
            Style::default()
                .bg(KEY_BG)
                .fg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}  "), Style::default().fg(TEXT_MUTED)),
    ]
}

pub fn render_status_line(f: &mut Frame, area: Rect, status: Option<&StatusLine>) {
    if area.height == 0 || !should_render_status(status) {
        f.render_widget(
            Block::default().style(Style::default().bg(BODY_BG)),
            area,
        );
        return;
    }

    let Some(status) = status else {
        return;
    };

    let content = vec![Line::from(vec![
        Span::styled(
            "error: ",
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(status.message.as_str(), Style::default().fg(TEXT)),
    ])];

    let paragraph = Paragraph::new(content)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::TOP)
                .style(Style::default().fg(TEXT).bg(ERROR_BG)),
        );

    f.render_widget(paragraph, area);
}

pub fn status_bar_height(status: Option<&StatusLine>) -> u16 {
    if should_render_status(status) { 3 } else { 0 }
}

fn should_render_status(status: Option<&StatusLine>) -> bool {
    matches!(status, Some(StatusLine { tone: StatusTone::Error, .. }))
}

pub fn render_screen_background(f: &mut Frame, area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(BODY_BG)),
        area,
    );
}

pub fn shorten_home_path(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    if let Some(home) = dirs::home_dir() {
        if let Some(rest) = display.strip_prefix(&home.display().to_string()) {
            return format!("~{rest}");
        }
    }
    display
}

pub fn truncate_middle(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }

    if max_len <= 3 {
        return "…".repeat(max_len.max(1));
    }

    let head_len = (max_len - 1) / 2;
    let tail_len = max_len - head_len - 1;
    let head: String = value.chars().take(head_len).collect();
    let tail: String = value
        .chars()
        .rev()
        .take(tail_len)
        .collect::<String>()
        .chars()
        .rev()
        .collect();

    format!("{head}…{tail}")
}

pub fn render_shell_approval_modal(
    f: &mut Frame,
    area: Rect,
    request: &ShellCommandExecutionRequest,
) {
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );

    let modal_area = centered_rect(area, 88, 58);

    let block = Block::default()
        .title(" shell command approval ")
        .borders(Borders::ALL)
        .style(Style::default().bg(HEADER_BG).fg(TEXT));
    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);
    let inner = Rect::new(
        inner.x + 1,
        inner.y + 1,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(1),
    );
    let chunks = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Min(0),
        ratatui::layout::Constraint::Length(2),
        ratatui::layout::Constraint::Length(1),
    ])
    .split(inner);

    let lines = vec![
        Line::from(Span::styled(
            "This step wants to execute a shell command.",
            Style::default().fg(TEXT),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Node: ", Style::default().fg(TEXT_MUTED)),
            Span::styled(request.node_name.as_str(), Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Step: ", Style::default().fg(TEXT_MUTED)),
            Span::styled(request.step_name.as_str(), Style::default().fg(TEXT)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Command",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(request.command.as_str()),
    ];

    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    f.render_widget(paragraph, chunks[0]);
    render_modal_footer(
        f,
        chunks[2],
        vec![
            ("y", "approve"),
            ("n", "decline"),
            ("esc", "decline"),
        ],
    );
}

pub fn render_capability_approval_modal(
    f: &mut Frame,
    area: Rect,
    modules: &[LlrtSupportedModules],
) {
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );

    let modal_area = centered_rect(area, 88, 50);

    let block = Block::default()
        .title(" capabilities approval ")
        .borders(Borders::ALL)
        .style(Style::default().bg(HEADER_BG).fg(TEXT));
    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);
    let inner = Rect::new(
        inner.x + 1,
        inner.y + 1,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(1),
    );
    let chunks = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Min(0),
        ratatui::layout::Constraint::Length(2),
        ratatui::layout::Constraint::Length(1),
    ])
    .split(inner);

    let requested = modules
        .iter()
        .map(|module| format!("{module:?}"))
        .collect::<Vec<_>>()
        .join(", ");

    let lines = vec![
        Line::from(Span::styled(
            "This step wants to use sensitive runtime capabilities.",
            Style::default().fg(TEXT),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Requested: ", Style::default().fg(TEXT_MUTED)),
            Span::styled(requested, Style::default().fg(TEXT)),
        ]),
    ];

    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    f.render_widget(paragraph, chunks[0]);
    render_modal_footer(
        f,
        chunks[2],
        vec![
            ("y", "approve"),
            ("n", "decline"),
            ("esc", "decline"),
        ],
    );
}

pub fn render_agent_selection_modal(
    f: &mut Frame,
    area: Rect,
    options: &[AgentSelectionItem],
    cursor: usize,
) {
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );

    let modal_area = centered_rect(area, 88, 62);

    let block = Block::default()
        .title(" ai agent selection ")
        .borders(Borders::ALL)
        .style(Style::default().bg(HEADER_BG).fg(TEXT));
    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);
    let inner = Rect::new(
        inner.x + 1,
        inner.y + 1,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(1),
    );
    let chunks = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Min(0),
        ratatui::layout::Constraint::Length(2),
        ratatui::layout::Constraint::Length(1),
    ])
    .split(inner);

    let mut lines = vec![
        Line::from(Span::styled(
            "Select a coding agent to execute the AI step.",
            Style::default().fg(TEXT),
        )),
        Line::from(""),
    ];

    if options.is_empty() {
        lines.push(Line::from(Span::styled(
            "No launchable agents detected. Press esc to use built-in AI.",
            Style::default().fg(TEXT_MUTED),
        )));
    } else {
        for (index, option) in options.iter().enumerate() {
            let prefix = if index == cursor { "› " } else { "  " };
            let style = if index == cursor {
                Style::default().fg(TEXT).bg(SURFACE).add_modifier(Modifier::BOLD)
            } else if option.is_available {
                Style::default().fg(TEXT)
            } else {
                Style::default().fg(TEXT_MUTED)
            };
            lines.push(Line::from(Span::styled(
                format!("{prefix}{}", option.label),
                style,
            )));
        }
    }

    lines.push(Line::from(""));

    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    f.render_widget(paragraph, chunks[0]);
    render_modal_footer(
        f,
        chunks[2],
        vec![
            ("enter", "select"),
            ("esc", "built-in AI"),
        ],
    );
}

fn render_modal_footer(f: &mut Frame, area: Rect, hints: Vec<(&str, &str)>) {
    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.extend(key_hint(key, label));
    }
    let footer_area = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(1),
        area.width,
        1,
    );
    f.render_widget(Line::from(spans), footer_area);
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Percentage((100 - height_percent) / 2),
        ratatui::layout::Constraint::Percentage(height_percent),
        ratatui::layout::Constraint::Percentage((100 - height_percent) / 2),
    ])
    .split(area);

    ratatui::layout::Layout::horizontal([
        ratatui::layout::Constraint::Percentage((100 - width_percent) / 2),
        ratatui::layout::Constraint::Percentage(width_percent),
        ratatui::layout::Constraint::Percentage((100 - width_percent) / 2),
    ])
    .split(vertical[1])[1]
}
