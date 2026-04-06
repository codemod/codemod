use butterflow_models::{Task, TaskStatus, WorkflowRun};
use chrono::Utc;
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, Padding, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::tui::app::LogView;

use super::{
    centered_rect, format_duration, key_hint, key_hint_colored, render_status_line,
    shorten_home_path, status_bar_height, task_status_color, task_status_icon, task_status_label,
    truncate_middle, workflow_status_color, workflow_status_icon, workflow_status_label,
    StatusLine, ACCENT, BODY_BG, CYAN, DIM, HEADER_BG, RED, SURFACE, TEXT, TEXT_MUTED,
};

/// Render the task list screen.
pub fn render(
    f: &mut Frame,
    area: Rect,
    workflow_run: Option<&WorkflowRun>,
    tasks: &[Task],
    table_state: &mut TableState,
    status: Option<&StatusLine>,
    log_view: Option<&LogView>,
    log_scroll: u16,
    log_follow: bool,
) {
    let status_height = status_bar_height(status);
    let help_height = help_bar_height(tasks, log_view.is_some(), area.width);
    let chunks = Layout::vertical([
        Constraint::Length(2),             // title bar
        Constraint::Min(0),                // table
        Constraint::Length(help_height),   // help bar
        Constraint::Length(status_height), // status bar
    ])
    .split(area);

    render_header(f, chunks[0], workflow_run);

    let content = chunks[1];
    f.render_widget(
        Block::default().style(Style::default().bg(BODY_BG)),
        chunks[1],
    );
    let visible_tasks: Vec<&Task> = tasks.iter().filter(|task| !task.is_master).collect();

    if visible_tasks.is_empty() {
        let y = content.y + content.height / 2;
        let line = Line::from(Span::styled(
            "Waiting for tasks…",
            Style::default().fg(TEXT_MUTED),
        ));
        f.render_widget(line, Rect::new(content.x, y, content.width, 1));
        render_help_bar(f, chunks[2], tasks, log_view.is_some());
        render_status_line(f, chunks[3], status);
        if let Some(log_view) = log_view {
            render_log_modal(f, area, log_view, log_scroll, log_follow);
        }
        return;
    }

    let matrix_columns = collect_matrix_columns(&visible_tasks);
    let header = build_header(&matrix_columns);
    let rows = build_rows(&visible_tasks, &matrix_columns);
    let widths = build_widths(&visible_tasks, &matrix_columns);

    let table = Table::new(rows, widths)
        .header(header)
        .style(Style::default().bg(BODY_BG))
        .row_highlight_style(Style::default().bg(SURFACE));
    f.render_stateful_widget(table, content, table_state);

    render_help_bar(f, chunks[2], tasks, log_view.is_some());
    render_status_line(f, chunks[3], status);

    if let Some(log_view) = log_view {
        render_log_modal(f, area, log_view, log_scroll, log_follow);
    }
}

fn collect_matrix_columns(tasks: &[&Task]) -> Vec<String> {
    let mut keys = Vec::new();
    for task in tasks {
        if let Some(matrix_values) = &task.matrix_values {
            for key in matrix_values.keys() {
                if !key.starts_with('_') && !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
    }
    keys.sort();
    keys.retain(|key| {
        tasks.iter().all(|task| {
            task.matrix_values
                .as_ref()
                .and_then(|matrix_values| matrix_values.get(key))
                .map(|value| match value {
                    serde_json::Value::String(value) => value.len(),
                    other => other.to_string().len(),
                } < 32)
                .unwrap_or(true)
        })
    });
    keys
}

fn build_header(matrix_columns: &[String]) -> Row<'static> {
    let mut header_cells = vec![
        Cell::from(Span::styled("STATUS", Style::default().fg(TEXT_MUTED))),
        Cell::from(Span::styled("NODE", Style::default().fg(TEXT_MUTED))),
    ];
    for key in matrix_columns {
        header_cells.push(Cell::from(Span::styled(
            key.to_uppercase(),
            Style::default().fg(TEXT_MUTED),
        )));
    }
    header_cells.push(Cell::from(Span::styled(
        "STARTED",
        Style::default().fg(TEXT_MUTED),
    )));
    header_cells.push(Cell::from(Span::styled(
        "DURATION",
        Style::default().fg(TEXT_MUTED),
    )));

    Row::new(header_cells).height(1).bottom_margin(1)
}

fn build_rows<'a>(tasks: &[&'a Task], matrix_columns: &[String]) -> Vec<Row<'a>> {
    tasks
        .iter()
        .map(|task| {
            let icon = task_status_icon(task.status, task.error.as_deref());
            let color = task_status_color(task.status, task.error.as_deref());
            let label = task_status_label(task.status, task.error.as_deref());

            let started = task
                .started_at
                .map(|time| time.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "—".to_string());

            let duration = match (task.started_at, task.ended_at) {
                (Some(start), Some(end)) => format_duration((end - start).num_seconds()),
                (Some(start), None) if task.status == TaskStatus::Running => {
                    format!("{}…", format_duration((Utc::now() - start).num_seconds()))
                }
                _ => "—".to_string(),
            };

            let mut cells = vec![
                Cell::from(Line::from(vec![
                    Span::styled(format!("{icon} "), Style::default().fg(color)),
                    Span::styled(label, Style::default().fg(color)),
                ])),
                Cell::from(Span::styled(
                    task.node_id.clone(),
                    Style::default().fg(TEXT),
                )),
            ];

            for key in matrix_columns {
                let value = task
                    .matrix_values
                    .as_ref()
                    .and_then(|matrix_values| matrix_values.get(key))
                    .map(|value| match value {
                        serde_json::Value::String(value) => value.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| "—".to_string());
                cells.push(Cell::from(Span::styled(
                    value,
                    Style::default().fg(TEXT_MUTED),
                )));
            }

            cells.push(Cell::from(Span::styled(
                started,
                Style::default().fg(TEXT_MUTED),
            )));
            cells.push(Cell::from(Span::styled(
                duration,
                Style::default().fg(TEXT_MUTED),
            )));

            Row::new(cells)
        })
        .collect()
}

fn build_widths(tasks: &[&Task], matrix_columns: &[String]) -> Vec<Constraint> {
    let mut widths = vec![Constraint::Length(14), Constraint::Fill(1)];
    for key in matrix_columns {
        let max_len = tasks
            .iter()
            .filter_map(|task| {
                task.matrix_values
                    .as_ref()
                    .and_then(|matrix_values| matrix_values.get(key))
                    .map(|value| match value {
                        serde_json::Value::String(value) => value.len(),
                        other => other.to_string().len(),
                    })
            })
            .max()
            .unwrap_or(0);
        widths.push(Constraint::Length(
            max_len.max(key.len()).min(32) as u16 + 2,
        ));
    }
    widths.push(Constraint::Length(10));
    widths.push(Constraint::Length(10));
    widths
}

fn render_header(f: &mut Frame, area: Rect, workflow_run: Option<&WorkflowRun>) {
    let block = Block::default()
        .style(Style::default().bg(HEADER_BG))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(run) = workflow_run else {
        let line = Line::from(Span::styled("Loading…", Style::default().fg(TEXT_MUTED)));
        let y = inner.y + inner.height.saturating_sub(1) / 2;
        f.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
        return;
    };

    let icon = workflow_status_icon(run.status);
    let color = workflow_status_color(run.status);
    let status_text = workflow_status_label(run.status);
    let workflow_name = run.name.as_deref().unwrap_or("Workflow");
    let target = run
        .target_path
        .as_ref()
        .map(|path| shorten_home_path(path.as_path()))
        .unwrap_or_default();
    let available_width = inner.width.saturating_sub(2) as usize;
    let workflow_label = truncate_middle(workflow_name, available_width.max(1).min(72));
    let target_label = truncate_middle(&format!("target: {target}"), available_width.max(1));

    let title_line = Line::from(vec![
        Span::styled(
            "codemod",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(DIM)),
        Span::styled(
            workflow_label,
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(status_text, Style::default().fg(color)),
    ]);
    let target_line = Line::from(Span::styled(target_label, Style::default().fg(DIM)));

    f.render_widget(title_line, Rect::new(inner.x, inner.y, inner.width, 1));
    f.render_widget(target_line, Rect::new(inner.x, inner.y + 1, inner.width, 1));
}

/// Build the list of hint groups for the current task list state.
fn build_hint_groups(tasks: &[Task], log_view_open: bool) -> Vec<Vec<Span<'static>>> {
    if log_view_open {
        return vec![
            key_hint("↑↓/pg", "scroll"),
            key_hint("g", "top"),
            key_hint("G", "bottom"),
            key_hint("esc", "close"),
            key_hint("q", "quit"),
        ];
    }

    let has_awaiting = tasks
        .iter()
        .any(|task| task.status == TaskStatus::AwaitingTrigger && !task.is_master);
    let has_failed = tasks
        .iter()
        .any(|task| task.status == TaskStatus::Failed && !task.is_master);

    let mut groups: Vec<Vec<Span<'static>>> = Vec::new();
    groups.push(key_hint("↑↓", "navigate"));
    groups.push(key_hint("⏎", "logs"));
    if has_awaiting {
        groups.push(key_hint_colored("t", "trigger", CYAN));
        groups.push(key_hint_colored("T", "trigger all", CYAN));
    }
    if has_failed {
        groups.push(key_hint_colored("R", "retry", RED));
    }
    groups.push(key_hint("s", "settings"));
    groups.push(key_hint("c", "cancel"));
    groups.push(key_hint("esc", "back"));
    groups.push(key_hint("q", "quit"));
    groups
}

/// Pack hint groups greedily into rows of `row_width` chars each.
fn pack_into_rows(groups: Vec<Vec<Span<'static>>>, row_width: usize) -> Vec<Vec<Span<'static>>> {
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_row: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for group in groups {
        let w: usize = group.iter().map(|s| s.content.chars().count()).sum();
        if current_width + w > row_width && !current_row.is_empty() {
            rows.push(std::mem::take(&mut current_row));
            current_width = 0;
        }
        current_width += w;
        current_row.extend(group);
    }
    if !current_row.is_empty() {
        rows.push(current_row);
    }
    rows
}

pub fn help_bar_height(tasks: &[Task], log_view_open: bool, available_width: u16) -> u16 {
    let row_width = available_width.saturating_sub(2) as usize;
    if row_width == 0 {
        return 1;
    }
    let groups = build_hint_groups(tasks, log_view_open);
    let num_rows = pack_into_rows(groups, row_width).len().max(1);
    // each row takes 1 line, with a blank line between rows
    (num_rows * 2).saturating_sub(1) as u16
}

fn render_help_bar(f: &mut Frame, area: Rect, tasks: &[Task], log_view_open: bool) {
    f.render_widget(Block::default().style(Style::default().bg(BODY_BG)), area);
    let padded = area.inner(Margin::new(1, 0));
    let row_width = padded.width as usize;

    let groups = build_hint_groups(tasks, log_view_open);
    let rows = pack_into_rows(groups, row_width);

    for (i, row_spans) in rows.into_iter().enumerate() {
        // stride of 2: one content row, one blank row between
        let row_area = Rect::new(padded.x, padded.y + (i * 2) as u16, padded.width, 1);
        if row_area.y >= area.y + area.height {
            break;
        }
        f.render_widget(Line::from(row_spans), row_area);
    }
}

fn render_log_modal(
    f: &mut Frame,
    area: Rect,
    log_view: &LogView,
    log_scroll: u16,
    log_follow: bool,
) {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(HEADER_BG)), area);

    let modal_area = centered_rect(area, 80, 70);
    let title_status = task_status_label(log_view.status, log_view.error.as_deref());
    let title_status_color = task_status_color(log_view.status, log_view.error.as_deref());

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" task "),
            Span::styled(
                log_view.node_id.clone(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" / "),
            Span::styled(
                title_status,
                Style::default()
                    .fg(title_status_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .style(Style::default().bg(HEADER_BG));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);
    let inner = Rect::new(
        inner.x + 1,
        inner.y + 1,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(1),
    );

    let content_chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(inner);
    let content_area = content_chunks[0];
    let footer_area = content_chunks[2];

    let mut lines = Vec::new();
    if log_view.lines.is_empty() {
        lines.push(Line::from(Span::styled(
            if log_view.status == TaskStatus::Running {
                "Task is running. Waiting for log output… some commands only flush when the step exits."
            } else {
                "(no logs)"
            },
            Style::default().fg(TEXT_MUTED),
        )));
    } else {
        for entry in &log_view.lines {
            append_log_entry_lines(&mut lines, entry, Style::default().fg(TEXT));
        }
    }

    if let Some(error) = &log_view.error {
        append_tagged_log_entry_lines(
            &mut lines,
            "error",
            error,
            Style::default().fg(RED).add_modifier(Modifier::BOLD),
            Style::default().fg(TEXT),
        );
    }

    let wrapped_lines = wrap_lines_to_width(lines, content_area.width.saturating_sub(1) as usize);

    let viewport_height = content_area.height as usize;
    let max_scroll = wrapped_lines.len().saturating_sub(viewport_height) as u16;
    let scroll = if log_follow {
        max_scroll
    } else {
        log_scroll.min(max_scroll)
    };

    let paragraph = Paragraph::new(Text::from(wrapped_lines))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(paragraph, content_area);

    let mut spans = Vec::new();
    spans.extend(super::key_hint("↑↓/pg", "scroll"));
    spans.extend(super::key_hint("g", "top"));
    spans.extend(super::key_hint("G", "bottom"));
    spans.extend(super::key_hint("esc", "close"));
    let footer_line_area = Rect::new(
        footer_area.x,
        footer_area.y + footer_area.height.saturating_sub(1),
        footer_area.width,
        1,
    );
    f.render_widget(Line::from(spans), footer_line_area);
}

fn append_log_entry_lines(lines: &mut Vec<Line<'static>>, entry: &str, style: Style) {
    let normalized = entry.replace('\r', "");
    let parts: Vec<&str> = normalized.split('\n').collect();

    for part in parts {
        lines.push(Line::from(Span::styled(part.to_string(), style)));
    }
}

fn append_tagged_log_entry_lines(
    lines: &mut Vec<Line<'static>>,
    tag: &str,
    entry: &str,
    tag_style: Style,
    text_style: Style,
) {
    let normalized = entry.replace('\r', "");
    let mut parts = normalized.split('\n');

    if let Some(first) = parts.next() {
        lines.push(Line::from(vec![
            Span::styled(format!("[{tag}] "), tag_style),
            Span::styled(first.to_string(), text_style),
        ]));
    }

    for part in parts {
        lines.push(Line::from(Span::styled(part.to_string(), text_style)));
    }
}

fn wrap_lines_to_width(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return lines;
    } // Note: This function wraps lines but does not preserve styling on wrapped chunks.
      // For styled text that wraps across lines, the style is lost on subsequent lines.
      // A more sophisticated implementation would track character-level styles and apply
      // them to wrapped chunks, but this is a reasonable trade-off for log output.
    let mut wrapped = Vec::new();

    for line in lines {
        let plain = line.to_string();
        if plain.is_empty() {
            wrapped.push(Line::from(String::new()));
            continue;
        }

        let chars: Vec<char> = plain.chars().collect();
        let mut start = 0;
        while start < chars.len() {
            let end = (start + width).min(chars.len());
            let chunk: String = chars[start..end].iter().collect();
            wrapped.push(Line::from(chunk));
            start = end;
        }
    }

    wrapped
}
