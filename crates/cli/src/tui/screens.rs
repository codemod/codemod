use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{ApprovalPrompt, Screen, TuiState};

pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    match state.screen {
        Screen::Runs => render_runs(frame, state),
        Screen::RunDetail => render_run_detail(frame, state),
    }

    if state.show_log_modal {
        render_log_modal(frame, state);
    }

    if let Some(approval) = &state.approval {
        render_approval_modal(frame, approval);
    }
}

fn render_runs(frame: &mut Frame<'_>, state: &TuiState) {
    let size = frame.area();
    let items = state
        .runs
        .iter()
        .enumerate()
        .map(|(index, run)| {
            let prefix = if index == state.selected_run { ">" } else { " " };
            ListItem::new(Line::from(format!(
                "{prefix} {}  {:?}  {}",
                run.id,
                run.status,
                run.name.clone().unwrap_or_else(|| "unnamed".to_string())
            )))
        })
        .collect::<Vec<_>>();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Workflow Runs (Enter to attach, q to quit)"),
    );
    frame.render_widget(list, size);
}

fn render_run_detail(frame: &mut Frame<'_>, state: &TuiState) {
    let size = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(size);

    let header = if let Some(run) = &state.current_run {
        Paragraph::new(format!(
            "{}  {}  {}",
            run.id,
            state.display_run_status(),
            run.name.clone().unwrap_or_else(|| "unnamed".to_string())
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Run (Enter logs, q back, g trigger, a all, c cancel)"),
        )
    } else {
        Paragraph::new("No run selected")
            .block(Block::default().borders(Borders::ALL).title("Run"))
    };
    frame.render_widget(header, chunks[0]);

    let task_items = state
        .visible_tasks()
        .iter()
        .enumerate()
        .map(|(index, task)| {
            let prefix = if index == state.selected_task { ">" } else { " " };
            let line = format!("{prefix} {}  {:?}  {}", task.id, task.status, task.node_id);
            ListItem::new(line)
        })
        .collect::<Vec<_>>();
    let tasks = List::new(task_items).block(Block::default().borders(Borders::ALL).title("Tasks"));
    frame.render_widget(tasks, chunks[1]);

    if let Some(banner) = &state.banner {
        let banner_area = ratatui::layout::Rect {
            x: size.x,
            y: size.height.saturating_sub(2),
            width: size.width,
            height: 2,
        };
        let banner = Paragraph::new(banner.message.clone()).style(if banner.is_error {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        });
        frame.render_widget(banner, banner_area);
    }
}

fn render_log_modal(frame: &mut Frame<'_>, state: &TuiState) {
    let size = frame.area();
    let area = ratatui::layout::Rect {
        x: size.width / 10,
        y: size.height / 5,
        width: size.width * 4 / 5,
        height: size.height * 3 / 5,
    };
    frame.render_widget(Clear, area);

    let title = state
        .selected_task()
        .map(|task| format!("Logs: {} ({:?})", task.node_id, task.status))
        .unwrap_or_else(|| "Logs".to_string());

    let logs = Paragraph::new(state.selected_task_log_text())
        .scroll((state.log_modal_scroll, 0))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{title}  (q close, ↑/↓ scroll, g top, G bottom)")),
        );
    frame.render_widget(logs, area);
}

fn render_approval_modal(frame: &mut Frame<'_>, approval: &ApprovalPrompt) {
    let size = frame.area();
    let area = ratatui::layout::Rect {
        x: size.width / 8,
        y: size.height / 4,
        width: size.width * 3 / 4,
        height: size.height / 3,
    };
    frame.render_widget(Clear, area);
    let body = match approval {
        ApprovalPrompt::Shell { command, .. } => {
            format!("Approve shell command?\n\n{}\n\n[y] approve  [n] reject", command)
        }
        ApprovalPrompt::Capabilities { modules, .. } => format!(
            "Approve capabilities?\n\n{}\n\n[y] approve  [n] reject",
            modules.join(", ")
        ),
        ApprovalPrompt::AgentSelection {
            options, selected, ..
        } => {
            let options_text = options
                .iter()
                .enumerate()
                .map(|(index, (label, _))| {
                    if index == *selected {
                        format!("> {label}")
                    } else {
                        format!("  {label}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Select coding agent\n\n{}\n\n[Enter] choose  [n] skip",
                options_text
            )
        }
    };
    let modal = Paragraph::new(body)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    "Approval",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )),
        );
    frame.render_widget(modal, area);
}
