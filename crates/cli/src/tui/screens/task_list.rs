use butterflow_models::{Task, TaskStatus, WorkflowRun};
use chrono::Utc;
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Padding, Row, Table, TableState},
    Frame,
};

use super::{
    format_duration, key_hint, key_hint_colored, task_status_color, task_status_icon,
    task_status_label, workflow_status_color, workflow_status_icon, workflow_status_label, ACCENT,
    CYAN, DIM, HEADER_BG, RED, SURFACE, TEXT, TEXT_MUTED,
};

/// Render the task list screen
pub fn render(
    f: &mut Frame,
    area: Rect,
    workflow_run: Option<&WorkflowRun>,
    tasks: &[Task],
    table_state: &mut TableState,
) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // title bar
        Constraint::Length(1), // spacing
        Constraint::Min(0),    // table
        Constraint::Length(1), // help bar
    ])
    .split(area);

    // -- Title / header bar --
    render_header(f, chunks[0], workflow_run);

    // -- Content --
    let content = chunks[2].inner(Margin::new(1, 0));

    // Filter out master tasks
    let visible_tasks: Vec<&Task> = tasks.iter().filter(|t| !t.is_master).collect();

    if visible_tasks.is_empty() {
        let y = content.y + content.height / 2;
        let line = Line::from(Span::styled(
            "Waiting for tasks\u{2026}",
            Style::default().fg(TEXT_MUTED),
        ));
        f.render_widget(line, Rect::new(content.x, y, content.width, 1));
        render_help_bar(f, chunks[3], tasks);
        return;
    }

    // -- Discover matrix columns --
    // Collect unique matrix keys (excluding _-prefixed) where all values are < 32 chars.
    let matrix_columns = {
        let mut keys: Vec<String> = Vec::new();
        for task in &visible_tasks {
            if let Some(mv) = &task.matrix_values {
                for k in mv.keys() {
                    if !k.starts_with('_') && !keys.contains(k) {
                        keys.push(k.clone());
                    }
                }
            }
        }
        keys.sort();
        // Keep only keys whose values are all < 32 characters
        keys.retain(|k| {
            visible_tasks.iter().all(|task| {
                task.matrix_values
                    .as_ref()
                    .and_then(|mv| mv.get(k))
                    .map(|v| {
                        let s = match v {
                            serde_json::Value::String(s) => s.len(),
                            other => other.to_string().len(),
                        };
                        s < 32
                    })
                    .unwrap_or(true)
            })
        });
        keys
    };

    // -- Column headers --
    let mut header_cells = vec![
        Cell::from(Span::styled("STATUS", Style::default().fg(TEXT_MUTED))),
        Cell::from(Span::styled("NODE", Style::default().fg(TEXT_MUTED))),
    ];
    for k in &matrix_columns {
        header_cells.push(Cell::from(Span::styled(
            k.to_uppercase(),
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

    let header = Row::new(header_cells).height(1).bottom_margin(1);

    // -- Rows --
    let rows: Vec<Row> = visible_tasks
        .iter()
        .map(|task| {
            let icon = task_status_icon(task.status);
            let color = task_status_color(task.status);
            let label = task_status_label(task.status);

            let started = task
                .started_at
                .map(|t| t.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "\u{2014}".to_string());

            let duration = match (task.started_at, task.ended_at) {
                (Some(start), Some(end)) => {
                    let secs = (end - start).num_seconds();
                    format_duration(secs)
                }
                (Some(start), None) if task.status == TaskStatus::Running => {
                    let secs = (Utc::now() - start).num_seconds();
                    format!("{}\u{2026}", format_duration(secs))
                }
                _ => "\u{2014}".to_string(),
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

            for k in &matrix_columns {
                let val = task
                    .matrix_values
                    .as_ref()
                    .and_then(|mv| mv.get(k))
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| "\u{2014}".to_string());
                cells.push(Cell::from(Span::styled(
                    val,
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
        .collect();

    // -- Column widths --
    let mut widths: Vec<Constraint> = vec![
        Constraint::Length(14), // STATUS
        Constraint::Length(20), // NODE
    ];
    for k in &matrix_columns {
        // Size each matrix column to fit header or max value width, capped at 32
        let max_val_len = visible_tasks
            .iter()
            .filter_map(|t| {
                t.matrix_values
                    .as_ref()
                    .and_then(|mv| mv.get(k))
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.len(),
                        other => other.to_string().len(),
                    })
            })
            .max()
            .unwrap_or(0);
        let col_width = max_val_len.max(k.len()).min(32) as u16 + 2;
        widths.push(Constraint::Length(col_width));
    }
    widths.push(Constraint::Length(10)); // STARTED
    widths.push(Constraint::Length(10)); // DURATION

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(SURFACE));

    f.render_stateful_widget(table, content, table_state);

    // -- Help bar --
    render_help_bar(f, chunks[3], tasks);
}

fn render_header(f: &mut Frame, area: Rect, workflow_run: Option<&WorkflowRun>) {
    let block = Block::default()
        .style(Style::default().bg(HEADER_BG))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(run) = workflow_run else {
        let loading = Line::from(Span::styled(
            "Loading\u{2026}",
            Style::default().fg(TEXT_MUTED),
        ));
        let y = inner.y + inner.height.saturating_sub(1) / 2;
        f.render_widget(loading, Rect::new(inner.x, y, inner.width, 1));
        return;
    };

    let icon = workflow_status_icon(run.status);
    let color = workflow_status_color(run.status);
    let status_text = workflow_status_label(run.status);

    let name = run.name.as_deref().unwrap_or("Workflow");
    let target = run
        .target_path
        .as_ref()
        .map(|p| {
            let s = p.display().to_string();
            if let Some(home) = dirs::home_dir() {
                if let Some(rest) = s.strip_prefix(&home.display().to_string()) {
                    return format!("~{rest}");
                }
            }
            s
        })
        .unwrap_or_default();

    let title_line = Line::from(vec![
        Span::styled(
            "codemod",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(DIM)),
        Span::styled(name, Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(status_text, Style::default().fg(color)),
        Span::raw("  "),
        Span::styled(target, Style::default().fg(DIM)),
    ]);

    let y = inner.y + inner.height.saturating_sub(1) / 2;
    f.render_widget(title_line, Rect::new(inner.x, y, inner.width, 1));
}

fn render_help_bar(f: &mut Frame, area: Rect, tasks: &[Task]) {
    let padded = area.inner(Margin::new(1, 0));

    let has_awaiting = tasks
        .iter()
        .any(|t| t.status == TaskStatus::AwaitingTrigger && !t.is_master);
    let has_failed = tasks
        .iter()
        .any(|t| t.status == TaskStatus::Failed && !t.is_master);

    let mut spans: Vec<Span> = Vec::new();
    spans.extend(key_hint("\u{2191}\u{2193}", "navigate"));
    spans.extend(key_hint("\u{23ce}", "logs"));

    if has_awaiting {
        spans.extend(key_hint_colored("t", "trigger", CYAN));
        spans.extend(key_hint_colored("T", "trigger all", CYAN));
    }
    if has_failed {
        spans.extend(key_hint_colored("R", "retry", RED));
    }

    spans.extend(key_hint("s", "settings"));
    spans.extend(key_hint("c", "cancel"));
    spans.extend(key_hint("esc", "back"));
    spans.extend(key_hint("q", "quit"));

    f.render_widget(Line::from(spans), padded);
}
