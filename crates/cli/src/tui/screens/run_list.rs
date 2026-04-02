use butterflow_models::WorkflowRun;
use chrono::Utc;
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Padding, Row, Table, TableState},
    Frame,
};

use super::{
    format_duration, key_hint, render_status_line, status_bar_height, workflow_status_color,
    workflow_status_icon, workflow_status_label, StatusLine, ACCENT, BODY_BG, DIM, HEADER_BG,
    SURFACE, TEXT, TEXT_MUTED,
};

/// Render the run list screen
pub fn render(
    f: &mut Frame,
    area: Rect,
    runs: &[WorkflowRun],
    table_state: &mut TableState,
    status: Option<&StatusLine>,
) {
    let status_height = status_bar_height(status);
    let chunks = Layout::vertical([
        Constraint::Length(3), // title bar
        Constraint::Length(1), // spacing
        Constraint::Min(0),    // table
        Constraint::Length(1), // help bar
        Constraint::Length(status_height), // status bar
    ])
    .split(area);

    // -- Title bar --
    render_title_bar(f, chunks[0]);

    // -- Content area with horizontal padding --
    let content = chunks[2].inner(Margin::new(1, 0));
    f.render_widget(
        Block::default().style(Style::default().bg(BODY_BG)),
        chunks[2],
    );

    if runs.is_empty() {
        render_empty_state(f, content);
        render_help_bar(f, chunks[3]);
        render_status_line(f, chunks[4], status);
        return;
    }

    // -- Column headers (rendered as part of the table) --
    let header = Row::new(vec![
        Cell::from(Span::styled("STATUS", Style::default().fg(TEXT_MUTED))),
        Cell::from(Span::styled("NAME", Style::default().fg(TEXT_MUTED))),
        Cell::from(Span::styled("TARGET", Style::default().fg(TEXT_MUTED))),
        Cell::from(Span::styled("STARTED", Style::default().fg(TEXT_MUTED))),
        Cell::from(Span::styled("DURATION", Style::default().fg(TEXT_MUTED))),
    ])
    .height(1)
    .bottom_margin(1);

    // -- Table rows --
    let rows: Vec<Row> = runs
        .iter()
        .map(|run| {
            let icon = workflow_status_icon(run.status);
            let color = workflow_status_color(run.status);
            let label = workflow_status_label(run.status);

            let id_str = run.id.to_string();
            let name = run.name.as_deref().unwrap_or(&id_str[..8]);

            let target = run
                .target_path
                .as_ref()
                .map(|p| {
                    let s = p.display().to_string();
                    // Shorten home dir
                    if let Some(home) = dirs::home_dir() {
                        if let Some(rest) = s.strip_prefix(&home.display().to_string()) {
                            return format!("~{rest}");
                        }
                    }
                    s
                })
                .unwrap_or_default();

            let started = run.started_at.format("%b %d, %H:%M").to_string();

            let duration = if let Some(ended) = run.ended_at {
                let secs = (ended - run.started_at).num_seconds();
                format_duration(secs)
            } else if run.status == butterflow_models::WorkflowStatus::Running
                || run.status == butterflow_models::WorkflowStatus::AwaitingTrigger
            {
                let secs = (Utc::now() - run.started_at).num_seconds();
                format!("{}\u{2026}", format_duration(secs))
            } else {
                "\u{2014}".to_string()
            };

            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(format!("{icon} "), Style::default().fg(color)),
                    Span::styled(label, Style::default().fg(color)),
                ])),
                Cell::from(Span::styled(name.to_string(), Style::default().fg(TEXT))),
                Cell::from(Span::styled(target, Style::default().fg(TEXT_MUTED))),
                Cell::from(Span::styled(started, Style::default().fg(TEXT_MUTED))),
                Cell::from(Span::styled(duration, Style::default().fg(TEXT_MUTED))),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Percentage(25),
            Constraint::Percentage(35),
            Constraint::Length(14),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .style(Style::default().bg(BODY_BG))
    .row_highlight_style(Style::default().bg(SURFACE));

    f.render_stateful_widget(table, content, table_state);

    // -- Help bar --
    render_help_bar(f, chunks[3]);
    render_status_line(f, chunks[4], status);
}

fn render_title_bar(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .style(Style::default().bg(HEADER_BG))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let title = Line::from(vec![
        Span::styled(
            "codemod",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(DIM)),
        Span::styled("workflow runs", Style::default().fg(TEXT)),
    ]);

    // Center vertically in the 3-row area
    let y = inner.y + inner.height.saturating_sub(1) / 2;
    let line_area = Rect::new(inner.x, y, inner.width, 1);
    f.render_widget(title, line_area);
}

fn render_empty_state(f: &mut Frame, area: Rect) {
    let y = area.y + area.height / 2;
    let line = Line::from(vec![
        Span::styled("No workflow runs yet. ", Style::default().fg(TEXT_MUTED)),
        Span::styled(
            "Run a workflow with: codemod workflow run -w <path>",
            Style::default().fg(DIM),
        ),
    ]);
    let line_area = Rect::new(area.x, y, area.width, 1);
    f.render_widget(line, line_area);
}

fn render_help_bar(f: &mut Frame, area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(BODY_BG)),
        area,
    );
    let padded = area.inner(Margin::new(1, 0));
    let mut spans: Vec<Span> = Vec::new();
    spans.extend(key_hint("\u{2191}\u{2193}", "navigate"));
    spans.extend(key_hint("\u{23ce}", "open"));
    spans.extend(key_hint("q", "quit"));

    f.render_widget(Line::from(spans), padded);
}
