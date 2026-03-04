pub mod run_list;
pub mod settings;
pub mod task_list;

use butterflow_models::{TaskStatus, WorkflowStatus};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

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
pub const KEY_BG: Color = Color::Rgb(55, 55, 70);

/// Get a status icon for a task status
pub fn task_status_icon(status: TaskStatus) -> &'static str {
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
pub fn task_status_color(status: TaskStatus) -> Color {
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
pub fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Completed => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Running => "running",
        TaskStatus::AwaitingTrigger => "awaiting",
        TaskStatus::Pending => "pending",
        TaskStatus::Blocked => "blocked",
        TaskStatus::WontDo => "skipped",
    }
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
        WorkflowStatus::AwaitingTrigger => "awaiting trigger",
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
