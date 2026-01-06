use butterflow_models::{TaskStatus, WorkflowStatus};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};

use super::app::App;
use super::pty::resize_pty_and_parser;
use super::types::{Popup, Screen, TriggerAction};
use super::utils::{
    clean_log_line, format_duration, status_color, strip_ansi_codes, task_status_color, truncate,
};

/// Helper to render a premium wave of squares with color gradients
fn render_status_wave(
    elapsed_ms: u128,
    count: usize,
    period_ms: u128,
    is_selected: bool,
    target_rgb: (u8, u8, u8),
) -> Line<'static> {
    // Professional character set with a smaller, more subtle scale
    let frames = ["·", "⬞", "▫", "▪", "■"];
    let (dg_r, dg_g, dg_b) = (80, 80, 90); // Dimmed Gray
    let (peak_r, peak_g, peak_b) = if is_selected {
        (255, 255, 255) // Peak at White when selected for high contrast on green bg
    } else {
        target_rgb
    };

    let mut spans = Vec::with_capacity(count);
    for i in 0..count {
        // Use a sine wave to calculate the frame index and color for each block
        let phase = i as f64 / count as f64;
        let time_factor = (elapsed_ms % period_ms) as f64 / period_ms as f64;

        // Sine wave offset by phase to create the wave motion
        let angle = 2.0 * std::f64::consts::PI * (time_factor - phase);
        let sine_val = (angle.sin() + 1.0) / 2.0; // Normalized to 0.0 - 1.0

        // Character selection
        let frame_idx = (sine_val * (frames.len() - 1) as f64).round() as usize;

        // Color interpolation (Gray to Target/White)
        let r = (dg_r as f64 + (peak_r as f64 - dg_r as f64) * sine_val) as u8;
        let g = (dg_g as f64 + (peak_g as f64 - dg_g as f64) * sine_val) as u8;
        let b = (dg_b as f64 + (peak_b as f64 - dg_b as f64) * sine_val) as u8;

        spans.push(Span::styled(
            frames[frame_idx],
            Style::default().fg(Color::Rgb(r, g, b)),
        ));
    }

    Line::from(spans)
}

/// Get status symbol (animated for active states)
fn status_symbol(status: WorkflowStatus, elapsed_ms: u128, is_selected: bool) -> Line<'static> {
    match status {
        WorkflowStatus::Running => {
            render_status_wave(elapsed_ms, 6, 1200, is_selected, (214, 255, 98))
        }
        WorkflowStatus::Completed => {
            Line::from(Span::styled("✓", Style::default().fg(status_color(status))))
        }
        WorkflowStatus::Failed => {
            Line::from(Span::styled("✗", Style::default().fg(status_color(status))))
        }
        WorkflowStatus::AwaitingTrigger => {
            render_status_wave(elapsed_ms, 6, 2000, is_selected, (255, 220, 100))
        }
        WorkflowStatus::Canceled => {
            Line::from(Span::styled("○", Style::default().fg(status_color(status))))
        }
        WorkflowStatus::Pending => {
            Line::from(Span::styled("◌", Style::default().fg(status_color(status))))
        }
    }
}

/// Get task status symbol (animated for active states)
fn task_status_symbol(status: TaskStatus, elapsed_ms: u128, is_selected: bool) -> Line<'static> {
    match status {
        TaskStatus::Running => render_status_wave(elapsed_ms, 6, 1200, is_selected, (214, 255, 98)),
        TaskStatus::Completed => Line::from(Span::styled(
            "✓",
            Style::default().fg(task_status_color(status)),
        )),
        TaskStatus::Failed => Line::from(Span::styled(
            "✗",
            Style::default().fg(task_status_color(status)),
        )),
        TaskStatus::AwaitingTrigger => {
            render_status_wave(elapsed_ms, 6, 2000, is_selected, (255, 220, 100))
        }
        TaskStatus::Blocked => Line::from(Span::styled(
            "◇",
            Style::default().fg(task_status_color(status)),
        )),
        TaskStatus::WontDo => Line::from(Span::styled(
            "○",
            Style::default().fg(task_status_color(status)),
        )),
        TaskStatus::Pending => Line::from(Span::styled(
            "◌",
            Style::default().fg(task_status_color(status)),
        )),
    }
}

/// Render breadcrumb navigation
pub fn render_breadcrumb(f: &mut Frame, app: &App, area: Rect) {
    // Theme colors
    let brand_green = Color::Rgb(214, 255, 98); // Codemod Green #d6ff62
    let bg_color = Color::Rgb(20, 20, 25); // Dark background
    let text_color = Color::Rgb(170, 170, 180);
    let dimmed_color = Color::Rgb(80, 80, 90);

    let mut spans = vec![
        // Brand Logo Area - minimalist
        Span::styled(" ⚡", Style::default().fg(brand_green).bold()),
        Span::styled(" CODEMOD ", Style::default().fg(Color::White).bold()),
    ];

    // Build breadcrumb path with minimalist dividers
    let sep = Span::styled(" › ", Style::default().fg(dimmed_color));

    // WORKFLOWS
    let workflows_style = if app.screen == Screen::Workflows {
        Style::default().fg(brand_green).bold()
    } else {
        Style::default().fg(text_color)
    };

    spans.push(sep.clone());
    spans.push(Span::styled("Workflows", workflows_style));

    if app.screen != Screen::Workflows {
        spans.push(sep.clone());

        let run_name = app
            .selected_run
            .as_ref()
            .and_then(|r| r.workflow.nodes.first())
            .map(|n| truncate(&n.name, 20))
            .unwrap_or_else(|| "Tasks".to_string());

        let tasks_style = if app.screen == Screen::Tasks {
            Style::default().fg(brand_green).bold()
        } else {
            Style::default().fg(text_color)
        };

        spans.push(Span::styled(run_name, tasks_style));
    }

    if app.screen == Screen::Actions || app.screen == Screen::Terminal {
        spans.push(sep.clone());

        let task_name = if app.screen == Screen::Terminal {
            app.terminal_task
                .and_then(|id| app.tasks.iter().find(|t| t.id == id))
                .map(|t| truncate(&t.node_id, 20))
                .unwrap_or_else(|| "Terminal".to_string())
        } else {
            app.selected_task
                .as_ref()
                .map(|t| truncate(&t.node_id, 20))
                .unwrap_or_else(|| "Actions".to_string())
        };

        let action_style = if app.screen == Screen::Actions {
            Style::default().fg(brand_green).bold()
        } else {
            Style::default().fg(text_color)
        };

        spans.push(Span::styled(task_name, action_style));
    }

    if app.screen == Screen::Terminal {
        spans.push(sep);
        spans.push(Span::styled(
            "Terminal",
            Style::default().fg(brand_green).bold(),
        ));
    }

    let breadcrumb = Paragraph::new(Line::from(spans)).style(Style::default().bg(bg_color));

    f.render_widget(breadcrumb, area);
}

/// Render the Workflows screen (Step 1)
pub fn render_workflows_screen(f: &mut Frame, app: &mut App, area: Rect, elapsed_ms: u128) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    // Left: Workflows list
    let header_style = Style::default().fg(Color::Rgb(214, 255, 98)).bold(); // Brand green for headers

    let header_cells = ["", "ID", "Status", "Name", "Started"]
        .iter()
        .map(|h| Cell::from(format!(" {} ", h)).style(header_style));
    let header_row = Row::new(header_cells)
        .height(1)
        .bottom_margin(1)
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.runs.iter().enumerate().map(|(i, run)| {
        let name = run
            .workflow
            .nodes
            .first()
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let started = run.started_at.format("%Y-%m-%d %H:%M").to_string();
        let is_selected = app.runs_state.selected() == Some(i);

        // Cells without extra manual padding
        let mut status_line_spans = vec![];
        status_line_spans.extend(status_symbol(run.status, elapsed_ms, is_selected).spans);

        // Brand green color for selected row
        let brand_green = Color::Rgb(214, 255, 98);
        let row_style = if is_selected {
            Style::default().fg(brand_green)
        } else {
            Style::default()
        };

        // Add ">" symbol for selected row
        let indicator = if is_selected {
            Cell::from(">").style(Style::default().fg(brand_green).bold())
        } else {
            Cell::from(" ")
        };

        Row::new(vec![
            indicator,
            Cell::from(truncate(&run.id.to_string(), 16)).style(row_style),
            Cell::from(Line::from(status_line_spans)),
            Cell::from(name).style(row_style),
            Cell::from(started).style(row_style),
        ])
        .height(1)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(1), // Indicator column
            Constraint::Length(20),
            Constraint::Length(7), // Premium wave is 6 chars + padding
            Constraint::Min(40),
            Constraint::Length(18),
        ],
    )
    .header(header_row)
    .block(
        Block::default()
            .borders(Borders::NONE)
            .padding(ratatui::widgets::Padding::new(1, 1, 1, 1)),
    )
    .row_highlight_style(Style::default()) // No color change on selection
    .highlight_symbol("");

    f.render_stateful_widget(table, chunks[0], &mut app.runs_state);

    // Right: Preview of selected workflow - Minimalist with background
    let detail_bg = Color::Rgb(25, 25, 30);
    let preview_block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default().bg(detail_bg))
        .padding(ratatui::widgets::Padding::new(3, 2, 2, 1));

    let preview_content: Vec<Line> = if let Some(idx) = app.runs_state.selected() {
        if let Some(run) = app.runs.get(idx) {
            let name = run
                .workflow
                .nodes
                .first()
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "unknown".to_string());

            let duration = run
                .ended_at
                .map(|end| end.signed_duration_since(run.started_at).num_seconds())
                .unwrap_or_else(|| {
                    chrono::Utc::now()
                        .signed_duration_since(run.started_at)
                        .num_seconds()
                });

            let mut status_line_spans = vec![Span::styled("  ", Style::default())];
            status_line_spans.extend(status_symbol(run.status, elapsed_ms, false).spans);
            status_line_spans.push(Span::styled(
                format!(" {:?}", run.status),
                Style::default().fg(status_color(run.status)).bold(),
            ));

            vec![
                Line::from(vec![Span::styled(
                    "Name ",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::styled(
                    format!("  {}", name),
                    Style::default().bold(),
                )]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Run ID ",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::raw(format!("  {}", run.id))]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Status ",
                    Style::default().fg(Color::DarkGray),
                )]),
                {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(status_symbol(run.status, elapsed_ms, false).spans);
                    spans.push(Span::styled(
                        format!(" {:?}", run.status),
                        Style::default().fg(status_color(run.status)).bold(),
                    ));
                    Line::from(spans)
                },
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Duration ",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::raw(format!("  {}", format_duration(duration)))]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Started ",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::raw(format!(
                    "  {}",
                    run.started_at.format("%Y-%m-%d %H:%M:%S")
                ))]),
                Line::from(""),
                Line::from(""),
                Line::styled(
                    "Press Enter to view tasks",
                    Style::default().fg(Color::Rgb(100, 180, 255)).italic(),
                ),
            ]
        } else {
            vec![Line::styled(
                "No workflow selected",
                Style::default().fg(Color::DarkGray),
            )]
        }
    } else {
        vec![Line::styled(
            "No workflow selected",
            Style::default().fg(Color::DarkGray),
        )]
    };

    let preview = Paragraph::new(preview_content).block(preview_block);

    f.render_widget(preview, chunks[1]);
}

/// Render the Tasks screen (Step 2)
pub fn render_tasks_screen(f: &mut Frame, app: &mut App, area: Rect, elapsed_ms: u128) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    // Left: Tasks list
    let header_style = Style::default().fg(Color::Rgb(214, 255, 98)).bold(); // Brand green

    let header_cells = ["", "Node ID", "Status", "Matrix"]
        .iter()
        .map(|h| Cell::from(*h).style(header_style));
    let header_row = Row::new(header_cells)
        .height(1)
        .bottom_margin(1)
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.tasks.iter().enumerate().map(|(i, task)| {
        let matrix_info = task
            .matrix_values
            .as_ref()
            .map(|m| {
                let mut entries: Vec<_> = m.iter().collect();
                entries.sort_by(|(k1, v1), (k2, v2)| {
                    k1.cmp(k2).then_with(|| {
                        serde_json::to_string(v1)
                            .unwrap_or_default()
                            .cmp(&serde_json::to_string(v2).unwrap_or_default())
                    })
                });
                entries
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "-".to_string());
        let is_selected = app.tasks_state.selected() == Some(i);

        // Brand green color for selected row
        let brand_green = Color::Rgb(214, 255, 98);
        let row_style = if is_selected {
            Style::default().fg(brand_green)
        } else {
            Style::default()
        };

        // Add ">" symbol for selected row
        let indicator = if is_selected {
            Cell::from(">").style(Style::default().fg(brand_green).bold())
        } else {
            Cell::from(" ")
        };

        Row::new(vec![
            indicator,
            Cell::from(truncate(&task.node_id, 20)).style(row_style),
            Cell::from(task_status_symbol(task.status, elapsed_ms, is_selected)),
            Cell::from(truncate(&matrix_info, 20)).style(row_style),
        ])
        .height(1)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(1), // Indicator column
            Constraint::Min(15),
            Constraint::Length(7), // Tightened for premium wave (6 chars)
            Constraint::Min(15),
        ],
    )
    .header(header_row)
    .block(
        Block::default()
            .borders(Borders::NONE)
            .padding(ratatui::widgets::Padding::new(1, 1, 1, 1)),
    )
    .row_highlight_style(Style::default()) // No color change on selection
    .highlight_symbol("");

    f.render_stateful_widget(table, chunks[0], &mut app.tasks_state);

    // Right: Task preview - Minimalist with background
    let detail_bg = Color::Rgb(25, 25, 30);
    let preview_block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default().bg(detail_bg))
        .padding(ratatui::widgets::Padding::new(3, 2, 2, 1));

    let preview_content: Vec<Line> = if let Some(idx) = app.tasks_state.selected() {
        if let Some(task) = app.tasks.get(idx) {
            let mut lines = vec![
                Line::from(vec![Span::styled(
                    "Node ",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::styled(
                    format!("  {}", task.node_id),
                    Style::default().bold(),
                )]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Status ",
                    Style::default().fg(Color::DarkGray),
                )]),
                {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(task_status_symbol(task.status, elapsed_ms, false).spans);
                    spans.push(Span::styled(
                        format!(" {:?}", task.status),
                        Style::default().fg(task_status_color(task.status)).bold(),
                    ));
                    Line::from(spans)
                },
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Task ID ",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::raw(format!(
                    "  {}",
                    truncate(&task.id.to_string(), 30)
                ))]),
            ];

            if let Some(matrix) = &task.matrix_values {
                lines.push(Line::from(""));
                lines.push(Line::styled(
                    "Matrix Values:",
                    Style::default().fg(Color::DarkGray),
                ));
                let mut matrix_entries: Vec<_> = matrix.iter().collect();
                matrix_entries.sort_by(|(k1, v1), (k2, v2)| {
                    k1.cmp(k2).then_with(|| {
                        serde_json::to_string(v1)
                            .unwrap_or_default()
                            .cmp(&serde_json::to_string(v2).unwrap_or_default())
                    })
                });
                for (k, v) in matrix_entries {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(k, Style::default().fg(Color::Rgb(250, 180, 100))),
                        Span::raw(": "),
                        Span::raw(v.to_string()),
                    ]));
                }
            }

            if !task.logs.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::styled(
                    format!("Logs: {} entries", task.logs.len()),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            lines.push(Line::from(""));
            lines.push(Line::styled(
                "Press Enter to view details",
                Style::default().fg(Color::Rgb(100, 180, 255)).italic(),
            ));

            lines
        } else {
            vec![Line::styled(
                "No task selected",
                Style::default().fg(Color::DarkGray),
            )]
        }
    } else {
        vec![Line::styled(
            "No task selected",
            Style::default().fg(Color::DarkGray),
        )]
    };

    let preview = Paragraph::new(preview_content).block(preview_block);

    f.render_widget(preview, chunks[1]);
}

/// Render the Actions screen (Step 3)
pub fn render_actions_screen(f: &mut Frame, app: &mut App, area: Rect, elapsed_ms: u128) {
    // Layout
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    // Styles
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().add_modifier(Modifier::BOLD);

    // Left: Task details and actions
    let details_block = Block::default()
        .borders(Borders::NONE)
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 1));

    let details_content: Vec<Line> = if let Some(task) = &app.selected_task {
        let mut lines = vec![
            Line::styled("DETAILS", Style::default().fg(Color::DarkGray).bold()),
            Line::from(""),
            Line::from(vec![
                Span::styled("Node: ", label_style),
                Span::styled(task.node_id.clone(), value_style),
            ]),
            Line::from({
                let mut spans = vec![Span::styled("Status: ", label_style)];
                spans.extend(task_status_symbol(task.status, elapsed_ms, false).spans);
                spans.push(Span::styled(
                    format!(" {:?}", task.status),
                    Style::default().fg(task_status_color(task.status)).bold(),
                ));
                spans
            }),
            Line::from(vec![
                Span::styled("ID: ", label_style),
                Span::raw(truncate(&task.id.to_string(), 12)),
            ]),
        ];

        if let Some(matrix) = &task.matrix_values {
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "MATRIX",
                Style::default().fg(Color::DarkGray).bold(),
            ));
            let mut matrix_entries: Vec<_> = matrix.iter().collect();
            matrix_entries.sort_by(|(k1, v1), (k2, v2)| {
                k1.cmp(k2).then_with(|| {
                    serde_json::to_string(v1)
                        .unwrap_or_default()
                        .cmp(&serde_json::to_string(v2).unwrap_or_default())
                })
            });
            for (k, v) in matrix_entries {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(k, Style::default().fg(Color::Rgb(250, 180, 100))),
                    Span::raw(": "),
                    Span::raw(v.to_string()),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "ACTIONS",
            Style::default().fg(Color::DarkGray).bold(),
        ));
        lines.push(Line::from(""));

        if task.status == TaskStatus::AwaitingTrigger {
            lines.push(Line::from(vec![
                Span::styled(
                    " t ",
                    Style::default().bg(Color::Green).fg(Color::Black).bold(),
                ),
                Span::raw(" Trigger this task"),
            ]));
        } else {
            lines.push(Line::styled(
                " (No actions available)",
                Style::default().fg(Color::DarkGray),
            ));
        }

        let awaiting_count = app.get_awaiting_tasks().len();
        if awaiting_count > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    " a ",
                    Style::default().bg(Color::Yellow).fg(Color::Black).bold(),
                ),
                Span::raw(format!(" Trigger all awaiting ({})", awaiting_count)),
            ]));
        }

        lines
    } else {
        vec![Line::styled(
            "No task selected",
            Style::default().fg(Color::DarkGray),
        )]
    };

    let details = Paragraph::new(details_content)
        .block(details_block)
        .wrap(Wrap { trim: false });

    f.render_widget(details, chunks[0]);

    // Right: Logs - Minimalist with background
    let logs_bg = Color::Rgb(25, 25, 30);
    let logs_block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default().bg(logs_bg))
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 1));

    let logs_content: Vec<Line> = if let Some(task) = &app.selected_task {
        if task.logs.is_empty() {
            vec![
                Line::from(""),
                Line::styled("No logs available", Style::default().fg(Color::DarkGray)),
            ]
        } else {
            let mut lines = vec![Line::from("")];
            let mut last_log: Option<String> = None;
            let mut line_number = 0;

            for log in task.logs.iter() {
                // Split on newlines to handle multi-line log entries
                for raw_line in log.lines() {
                    let cleaned_log = clean_log_line(raw_line);
                    let cleaned_log = strip_ansi_codes(&cleaned_log);

                    // Skip empty lines
                    if cleaned_log.is_empty() {
                        continue;
                    }

                    if let Some(ref last) = last_log {
                        if cleaned_log == *last {
                            continue;
                        }
                    }
                    last_log = Some(cleaned_log.clone());

                    line_number += 1;

                    let (style, prefix) = if cleaned_log.contains("ERROR")
                        || cleaned_log.contains("error:")
                        || cleaned_log.contains("failed")
                    {
                        (Style::default().fg(Color::Red), " ✗ ")
                    } else if cleaned_log.contains("WARN") || cleaned_log.contains("warning:") {
                        (Style::default().fg(Color::Yellow), " ⚠ ")
                    } else if cleaned_log.contains("INFO") || cleaned_log.contains("info:") {
                        (Style::default().fg(Color::Cyan), " ℹ ")
                    } else {
                        (Style::default().fg(Color::DarkGray), "   ")
                    };

                    // Apply syntax highlighting if possible (simple heuristic)
                    let styled_log = if cleaned_log.starts_with(">") || cleaned_log.starts_with("$")
                    {
                        // Command
                        Span::styled(cleaned_log, Style::default().fg(Color::Green))
                    } else {
                        Span::styled(cleaned_log, style)
                    };

                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:>3} ", line_number),
                            Style::default().fg(Color::Rgb(60, 60, 60)),
                        ),
                        Span::raw(prefix),
                        styled_log,
                    ]));
                }
            }

            // Store total counted lines
            app.total_log_lines = lines.len();
            lines
        }
    } else {
        app.total_log_lines = 1;
        vec![Line::styled(
            "No logs",
            Style::default().fg(Color::DarkGray),
        )]
    };

    // Correctly apply scroll clamping during render just in case input handler missed it (e.g. resize)
    let logs_area = chunks[1]; // Correct reference to the chunk
    app.log_height = logs_block.inner(logs_area).height;

    let max_scroll = app
        .total_log_lines
        .saturating_sub(app.log_height.saturating_sub(2) as usize)
        .max(0); // Safely calc max scroll
    if app.log_scroll > max_scroll {
        app.log_scroll = max_scroll;
    }

    let logs = Paragraph::new(logs_content)
        .block(logs_block)
        .scroll((app.log_scroll as u16, 0))
        .wrap(Wrap { trim: false });

    f.render_widget(logs, logs_area);
}

/// Render the Terminal screen
pub fn render_terminal_screen(f: &mut Frame, app: &mut App, area: Rect) {
    // Check if PTY is still running
    let pty_running = {
        let running = app
            .pty_running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *running
    };

    let title = if app.insert_mode {
        " Terminal [-- INSERT --] "
    } else if pty_running {
        " Terminal "
    } else if app.pty_writer.is_some() {
        " Terminal [Process Exited] "
    } else {
        " Terminal [Idle] "
    };

    let border_color = if app.insert_mode {
        Color::Red
    } else if pty_running {
        Color::Green
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(title, Style::default().bold()))
        .border_style(Style::default().fg(border_color));

    let inner_area = block.inner(area);
    f.render_widget(block, area);

    // Update terminal size if the area changed
    let new_rows = inner_area.height;
    let new_cols = inner_area.width;
    resize_pty_and_parser(app, new_rows, new_cols);

    // Get content from the vt100 parser
    let parser = app
        .terminal_parser
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let screen = parser.screen();

    // Build terminal content from the screen
    let mut terminal_content: Vec<Line> = Vec::new();

    for row in 0..screen.size().0 {
        let mut spans: Vec<Span> = Vec::new();

        for col in 0..screen.size().1 {
            // Defensive: handle potential cell access failure gracefully
            let cell = match screen.cell(row, col) {
                Some(cell) => cell,
                None => {
                    // If cell is unavailable, use a default empty cell
                    spans.push(Span::raw(" "));
                    continue;
                }
            };
            let ch = cell.contents();

            // Convert vt100 color to ratatui color
            let fg_color = match cell.fgcolor() {
                vt100::Color::Default => Color::Reset, // Use Reset instead of White for better blending
                vt100::Color::Idx(0) => Color::Black,
                vt100::Color::Idx(1) => Color::Red,
                vt100::Color::Idx(2) => Color::Green,
                vt100::Color::Idx(3) => Color::Yellow,
                vt100::Color::Idx(4) => Color::Blue,
                vt100::Color::Idx(5) => Color::Magenta,
                vt100::Color::Idx(6) => Color::Cyan,
                vt100::Color::Idx(7) => Color::Gray,
                vt100::Color::Idx(8) => Color::DarkGray,
                vt100::Color::Idx(9) => Color::LightRed,
                vt100::Color::Idx(10) => Color::LightGreen,
                vt100::Color::Idx(11) => Color::LightYellow,
                vt100::Color::Idx(12) => Color::LightBlue,
                vt100::Color::Idx(13) => Color::LightMagenta,
                vt100::Color::Idx(14) => Color::LightCyan,
                vt100::Color::Idx(15) => Color::White,
                vt100::Color::Idx(idx) => Color::Indexed(idx),
                vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
            };

            let bg_color = match cell.bgcolor() {
                vt100::Color::Default => Color::Reset,
                vt100::Color::Idx(0) => Color::Black,
                vt100::Color::Idx(1) => Color::Red,
                vt100::Color::Idx(2) => Color::Green,
                vt100::Color::Idx(3) => Color::Yellow,
                vt100::Color::Idx(4) => Color::Blue,
                vt100::Color::Idx(5) => Color::Magenta,
                vt100::Color::Idx(6) => Color::Cyan,
                vt100::Color::Idx(7) => Color::Gray,
                vt100::Color::Idx(8) => Color::DarkGray,
                vt100::Color::Idx(idx) => Color::Indexed(idx),
                vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
            };

            let mut style = Style::default().fg(fg_color).bg(bg_color);

            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }

            // Use the character or a space if empty
            let display_char = if ch.is_empty() {
                " ".to_string()
            } else {
                ch.to_string()
            };
            spans.push(Span::styled(display_char, style));
        }

        terminal_content.push(Line::from(spans));
    }

    let terminal = Paragraph::new(terminal_content);
    f.render_widget(terminal, inner_area);
}

/// Render footer with keybindings
pub fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let mode_bg = if app.insert_mode && app.screen == Screen::Terminal {
        Color::Rgb(214, 255, 98) // Brand green for insert mode
    } else {
        Color::Rgb(60, 60, 70) // Darker gray for normal mode
    };
    let mode_fg = if app.insert_mode && app.screen == Screen::Terminal {
        Color::Black
    } else {
        Color::White
    };

    let mode = if app.insert_mode && app.screen == Screen::Terminal {
        " INSERT "
    } else {
        " NORMAL "
    };

    let hints = match app.screen {
        Screen::Workflows => " ▲/▼ Navigate • Enter Select • c Cancel • r Refresh • ? Help • q Quit ",
        Screen::Tasks => " ▲/▼ Navigate • Enter Select • a Trigger All • Esc Back • r Refresh • ? Help • q Quit ",
        Screen::Actions => {
            " ▲/▼ Scroll • t Trigger • a Trigger All • v Terminal • Esc Back • r Refresh • ? Help • q Quit "
        }
        Screen::Terminal => {
            if app.insert_mode {
                " Type to input • Enter Submit • Ctrl+C Interrupt • Esc Exit Insert Mode "
            } else {
                " i Insert • Ctrl+C Interrupt • Esc Back • q Quit "
            }
        }
    };

    let spans = vec![
        Span::styled(mode, Style::default().bg(mode_bg).fg(mode_fg).bold()),
        Span::styled(" ", Style::default().bg(Color::Rgb(30, 30, 35))),
        Span::styled(
            hints,
            Style::default()
                .fg(Color::Rgb(140, 140, 150))
                .bg(Color::Rgb(30, 30, 35)),
        ),
    ];

    let footer = Paragraph::new(Line::from(spans))
        .alignment(ratatui::layout::Alignment::Left)
        .block(Block::default().style(Style::default().bg(Color::Rgb(30, 30, 35))));

    f.render_widget(footer, area);
}

/// Create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Render popup dialogs
pub fn render_popup(f: &mut Frame, app: &App) {
    match &app.popup {
        Popup::None => {}
        Popup::ConfirmCancel(run_id) => {
            let popup_area = centered_rect(50, 30, f.area());
            f.render_widget(Clear, popup_area);

            let text = vec![
                Line::from(""),
                Line::from(format!(
                    "Cancel workflow {}?",
                    truncate(&run_id.to_string(), 12)
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "This action cannot be undone.",
                    Style::default().fg(Color::Yellow),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("y", Style::default().fg(Color::Green).bold()),
                    Span::raw(": Yes  "),
                    Span::styled("n", Style::default().fg(Color::Red).bold()),
                    Span::raw(": No"),
                ]),
            ];

            let popup = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(" Confirm Cancel ")
                        .border_style(Style::default().fg(Color::Yellow)),
                )
                .alignment(ratatui::layout::Alignment::Center);
            f.render_widget(popup, popup_area);
        }
        Popup::ConfirmQuit => {
            let popup_area = centered_rect(50, 25, f.area());
            f.render_widget(Clear, popup_area);
            let text = vec![
                Line::from(""),
                Line::from("  A task or workflow is currently running."),
                Line::from(""),
                Line::from("  Are you sure you want to quit?"),
                Line::from(""),
                Line::from("  Press 'y' to quit, 'n' to cancel"),
                Line::from(""),
            ];
            let popup = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::Yellow))
                        .title(" Confirm Quit "),
                )
                .alignment(ratatui::layout::Alignment::Left)
                .wrap(Wrap { trim: false });
            f.render_widget(popup, popup_area);
        }
        Popup::ConfirmTrigger(action) => {
            let popup_area = centered_rect(55, 35, f.area());
            f.render_widget(Clear, popup_area);

            let (title, desc) = match action {
                TriggerAction::All => {
                    let count = app.get_awaiting_tasks().len();
                    (
                        " Trigger All Tasks ",
                        format!("Trigger all {} awaiting task(s)?", count),
                    )
                }
                TriggerAction::Single(task_id) => (
                    " Trigger Task ",
                    format!("Trigger task {}?", truncate(&task_id.to_string(), 12)),
                ),
            };

            let text = vec![
                Line::from(""),
                Line::from(desc),
                Line::from(""),
                Line::from(Span::styled(
                    "This will resume the workflow execution.",
                    Style::default().fg(Color::Yellow),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("y", Style::default().fg(Color::Green).bold()),
                    Span::raw(": Yes, trigger  "),
                    Span::styled("n", Style::default().fg(Color::Red).bold()),
                    Span::raw(": No, cancel"),
                ]),
            ];

            let popup = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(title)
                        .border_style(Style::default().fg(Color::Green)),
                )
                .alignment(ratatui::layout::Alignment::Center);
            f.render_widget(popup, popup_area);
        }
        Popup::Error(msg) => {
            let popup_area = centered_rect(70, 40, f.area());
            f.render_widget(Clear, popup_area);

            let text = vec![
                Line::from(""),
                Line::styled(" ✗ Error", Style::default().fg(Color::Red).bold()),
                Line::from(""),
                Line::from(msg.as_str()),
                Line::from(""),
                Line::from(Span::styled(
                    "Press any key to close",
                    Style::default().fg(Color::DarkGray),
                )),
            ];

            let popup = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(" Error ")
                        .border_style(Style::default().fg(Color::Red)),
                )
                .alignment(ratatui::layout::Alignment::Center)
                .wrap(Wrap { trim: true });
            f.render_widget(popup, popup_area);
        }
        Popup::StatusMessage(msg, _) => {
            let popup_area = centered_rect(60, 20, f.area());
            f.render_widget(Clear, popup_area);

            let text = vec![
                Line::from(""),
                Line::from(msg.as_str()),
                Line::from(""),
                Line::from(Span::styled(
                    "Press any key to continue",
                    Style::default().fg(Color::DarkGray),
                )),
            ];

            let popup = Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(" Status ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .alignment(ratatui::layout::Alignment::Center);
            f.render_widget(popup, popup_area);
        }
        Popup::Help => {
            let popup_area = centered_rect(70, 70, f.area());
            f.render_widget(Clear, popup_area);

            let brand_green = Color::Rgb(214, 255, 98);
            let text = vec![
                Line::from(""),
                Line::styled(" Navigation ", Style::default().bold().fg(brand_green)),
                Line::from("  ↑/k ↓/j          Navigate / Scroll"),
                Line::from("  Enter            Go to next step"),
                Line::from("  Esc / Backspace  Go back"),
                Line::from("  g / G            Go to first / last"),
                Line::from("  Ctrl+u / Ctrl+d  Half-page up / down (logs)"),
                Line::from(""),
                Line::styled(" Actions ", Style::default().bold().fg(brand_green)),
                Line::from("  c                Cancel workflow (Step 1)"),
                Line::from("  t                Trigger current task (Step 3)"),
                Line::from("  a                Trigger all awaiting"),
                Line::from("  v                Open terminal view"),
                Line::from("  r                Force refresh"),
                Line::from(""),
                Line::styled(" General ", Style::default().bold().fg(brand_green)),
                Line::from("  ?                Show this help"),
                Line::from("  q / Ctrl+C       Quit"),
                Line::from(""),
                Line::from(Span::styled(
                    "Press any key to close",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                )),
            ];

            let popup = Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Span::styled(
                        " Help ",
                        Style::default().bold().fg(Color::Rgb(170, 170, 180)),
                    ))
                    .border_style(Style::default().fg(Color::Rgb(60, 60, 70))),
            );
            f.render_widget(popup, popup_area);
        }
    }
}

/// Main UI render function
pub fn ui(f: &mut Frame, app: &mut App) {
    let elapsed_ms = app.start_time.elapsed().as_millis();
    let area = f.area();

    // Main layout: breadcrumb + content + footer
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Breadcrumb
            Constraint::Min(10),   // Content
            Constraint::Length(1), // Footer
        ])
        .split(area);

    let breadcrumb_area = main_chunks[0];
    let content_area = main_chunks[1];
    let footer_area = main_chunks[2];

    // Render breadcrumb
    render_breadcrumb(f, app, breadcrumb_area);

    // Render current screen
    match app.screen {
        Screen::Workflows => render_workflows_screen(f, app, content_area, elapsed_ms),
        Screen::Tasks => render_tasks_screen(f, app, content_area, elapsed_ms),
        Screen::Actions => render_actions_screen(f, app, content_area, elapsed_ms),
        Screen::Terminal => render_terminal_screen(f, app, content_area),
    }

    // Render footer
    render_footer(f, app, footer_area);

    // Render popup if any
    render_popup(f, app);
}
