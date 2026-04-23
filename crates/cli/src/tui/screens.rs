use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{ApprovalPrompt, Screen, TuiState};

fn log_modal_copy_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "ctrl+c/cmd+c copy"
    } else {
        "ctrl+c copy"
    }
}

fn workflow_status_style(status: butterflow_models::WorkflowStatus) -> Style {
    match status {
        butterflow_models::WorkflowStatus::AwaitingTrigger => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        butterflow_models::WorkflowStatus::Running => Style::default()
            .fg(Color::Rgb(255, 165, 0))
            .add_modifier(Modifier::BOLD),
        butterflow_models::WorkflowStatus::Failed => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        butterflow_models::WorkflowStatus::Completed => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        _ => Style::default(),
    }
}

fn task_status_style(status: butterflow_models::TaskStatus) -> Style {
    match status {
        butterflow_models::TaskStatus::AwaitingTrigger => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        butterflow_models::TaskStatus::Running => Style::default()
            .fg(Color::Rgb(255, 165, 0))
            .add_modifier(Modifier::BOLD),
        butterflow_models::TaskStatus::Failed => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        butterflow_models::TaskStatus::Completed => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        _ => Style::default(),
    }
}
pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    if let Some(approval) = &state.approval {
        frame.render_widget(Clear, frame.area());
        render_approval_modal(frame, approval);
        return;
    }

    if state.show_log_modal {
        frame.render_widget(Clear, frame.area());
        render_log_modal(frame, state);
        return;
    }

    match state.screen {
        Screen::Runs => render_runs(frame, state),
        Screen::RunDetail => render_run_detail(frame, state),
    }
}

fn render_runs(frame: &mut Frame<'_>, state: &TuiState) {
    let size = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);
    let title_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(chunks[0]);
    let header = Paragraph::new("Workflow Runs").block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, title_chunks[1]);

    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(chunks[1]);

    let header_row_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content_chunks[0]);
    let table_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content_chunks[1]);

    let status_width = 16usize;
    let content_width = table_chunks[1].width.saturating_sub(2) as usize;
    let elapsed_width = state
        .runs
        .iter()
        .map(TuiState::workflow_elapsed_text)
        .map(|text| text.chars().count())
        .max()
        .unwrap_or(1)
        .max("Elapsed".chars().count());
    let name_width = content_width.saturating_sub(2 + status_width + 2 + elapsed_width);
    let runs_header = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<status_width$}", "Status", status_width = status_width),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{:<name_width$}",
                truncate_text("Workflow", name_width),
                name_width = name_width
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{:>elapsed_width$}",
                "Elapsed",
                elapsed_width = elapsed_width
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(runs_header), header_row_chunks[1]);

    let items = state
        .runs
        .iter()
        .enumerate()
        .map(|(index, run)| {
            let is_selected = index == state.selected_run;
            let prefix = if is_selected { "▶" } else { " " };
            let status_text = TuiState::workflow_status_text(run.status);
            let elapsed_text = TuiState::workflow_elapsed_text(run);
            let status_style = workflow_status_style(run.status);
            let item = ListItem::new(Line::from(vec![
                Span::raw(format!("{prefix} ")),
                Span::styled(
                    format!("{status_text:<status_width$}", status_width = status_width),
                    status_style,
                ),
                Span::raw("  "),
                Span::raw(format!(
                    "{:<name_width$}",
                    truncate_text(&TuiState::workflow_run_display_name(run), name_width),
                    name_width = name_width
                )),
                Span::raw("  "),
                Span::raw(format!(
                    "{:>elapsed_width$}",
                    elapsed_text,
                    elapsed_width = elapsed_width
                )),
            ]));
            if is_selected {
                item.style(
                    Style::default()
                        .bg(Color::Rgb(45, 45, 45))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                item
            }
        })
        .collect::<Vec<_>>();

    let list = List::new(items);
    frame.render_widget(list, table_chunks[1]);
    render_help_bar(frame, chunks[2], "Enter attach  q quit");
}

fn render_run_detail(frame: &mut Frame<'_>, state: &TuiState) {
    let size = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(size);

    let header = if let Some(run) = &state.current_run {
        let header_row_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(chunks[0]);
        let status_style = workflow_status_style(run.status);
        let mut lines = vec![Line::from(vec![
            Span::raw(state.display_workflow_name()),
            Span::raw("  "),
            Span::styled(state.display_run_status(), status_style),
        ])];
        if let Some(target_path) = state.display_target_path() {
            lines.push(Line::from(target_path));
        }
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::BOTTOM)),
            header_row_chunks[1],
        );
        None
    } else {
        Some(Paragraph::new("No run selected").block(Block::default().borders(Borders::BOTTOM)))
    };
    if let Some(header) = header {
        frame.render_widget(header, chunks[0]);
    }

    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(chunks[1]);

    let header_row_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content_chunks[0]);
    let table_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content_chunks[1]);

    let content_width = table_chunks[1].width.saturating_sub(2) as usize;
    let task_viewport_height = table_chunks[1].height as usize;
    let elapsed_width = state
        .visible_task_window(task_viewport_height)
        .iter()
        .map(|task| state.task_elapsed_text(task).chars().count())
        .max()
        .unwrap_or(1)
        .max("Elapsed".chars().count());
    let min_step_width = 12usize;
    let min_status_width = 6usize;
    let preferred_status_width = 16usize;
    let min_progress_width = 10usize;
    let preferred_progress_width = 18usize;
    let available_for_progress = content_width
        .saturating_sub(min_step_width)
        .saturating_sub(2)
        .saturating_sub(preferred_status_width)
        .saturating_sub(2)
        .saturating_sub(elapsed_width)
        .saturating_sub(2);
    let progress_width = available_for_progress.clamp(min_progress_width, preferred_progress_width);
    let available_for_status = content_width
        .saturating_sub(progress_width)
        .saturating_sub(2)
        .saturating_sub(elapsed_width)
        .saturating_sub(2)
        .saturating_sub(min_step_width)
        .saturating_sub(2);
    let status_width = available_for_status.clamp(min_status_width, preferred_status_width);
    let step_width = content_width
        .saturating_sub(status_width)
        .saturating_sub(2)
        .saturating_sub(elapsed_width)
        .saturating_sub(2)
        .saturating_sub(progress_width)
        .saturating_sub(2);
    let tasks_header = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<step_width$}", "Step", step_width = step_width),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{:<status_width$}",
                truncate_text("Status", status_width),
                status_width = status_width
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{:>elapsed_width$}",
                "Elapsed",
                elapsed_width = elapsed_width
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{:<progress_width$}",
                "Progress",
                progress_width = progress_width
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(tasks_header), header_row_chunks[1]);

    let task_items = state
        .visible_task_window(task_viewport_height)
        .iter()
        .enumerate()
        .map(|(index, task)| {
            let visible_index = state.task_list_scroll + index;
            let is_selected = visible_index == state.selected_task;
            let prefix = if is_selected { "▶" } else { " " };
            let step_name = state.task_display_name(task);
            let status = compact_status_text(task.status, status_width);
            let elapsed = state.task_elapsed_text(task);
            let truncated_name = truncate_text(&step_name, step_width);
            let progress_bar = state
                .task_progress_bar(task, progress_width)
                .unwrap_or_else(|| " ".repeat(progress_width));
            let status_style = task_status_style(task.status);

            let item = ListItem::new(Line::from(vec![
                Span::raw(format!(
                    "{prefix} {truncated_name:<step_width$}",
                    step_width = step_width
                )),
                Span::raw("  "),
                Span::styled(
                    format!("{status:<status_width$}", status_width = status_width),
                    status_style,
                ),
                Span::raw(format!(
                    "  {elapsed:>elapsed_width$}",
                    elapsed_width = elapsed_width
                )),
                Span::styled(
                    format!(
                        "  {progress_bar:<progress_width$}",
                        progress_width = progress_width
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            if is_selected {
                item.style(
                    Style::default()
                        .bg(Color::Rgb(45, 45, 45))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let tasks = List::new(task_items);
    frame.render_widget(tasks, table_chunks[1]);
    let footer_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(chunks[2]);
    if let Some(detail) = state.selected_task_completion_detail() {
        let detail_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(footer_chunks[0]);
        frame.render_widget(
            Paragraph::new(detail).style(Style::default().fg(Color::DarkGray)),
            detail_chunks[1],
        );
    }
    render_help_bar(frame, footer_chunks[2], &state.task_help_text());
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

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let title = state
        .selected_task()
        .map(|task| {
            Line::from(vec![
                Span::raw(format!("Logs: {} (", task.node_id)),
                Span::styled(format!("{:?}", task.status), task_status_style(task.status)),
                Span::raw(")"),
            ])
        })
        .unwrap_or_else(|| Line::from("Logs"));

    let logs = Paragraph::new(state.selected_task_log_text())
        .scroll((state.log_modal_scroll, 0))
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(logs, chunks[0]);
    render_help_bar(
        frame,
        chunks[1],
        &format!(
            "↑/↓ scroll  g top  G bottom  {}  q/esc close",
            log_modal_copy_hint()
        ),
    );
    if let Some(notice) = state.log_modal_notice_text() {
        frame.render_widget(
            Paragraph::new(notice).style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }
}

fn render_help_bar(frame: &mut Frame<'_>, area: ratatui::layout::Rect, text: &str) {
    let left_padding = 2;
    let mut x = area.x.saturating_add(left_padding);
    for segment in text.split("  ").filter(|segment| !segment.is_empty()) {
        let mut parts = segment.splitn(2, ' ');
        let key = parts.next().unwrap_or_default();
        let label = parts.next().unwrap_or_default();

        let key_width = key.chars().count() as u16 + 2;
        if x + key_width > area.x + area.width {
            break;
        }

        let key_area = Rect {
            x,
            y: area.y,
            width: key_width,
            height: area.height,
        };
        let key_widget = Paragraph::new(key)
            .style(
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center);
        frame.render_widget(key_widget, key_area);
        x += key_width + 1;

        if !label.is_empty() {
            let label_width = label.chars().count() as u16;
            if x + label_width > area.x + area.width {
                break;
            }
            let label_area = Rect {
                x,
                y: area.y,
                width: label_width,
                height: area.height,
            };
            let label_widget = Paragraph::new(label).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_widget(label_widget, label_area);
            x += label_width + 2;
        }
    }
}

fn truncate_text(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let char_count = text.chars().count();
    if char_count <= max_width {
        return text.to_string();
    }

    if max_width == 1 {
        return "…".to_string();
    }

    let mut truncated = text.chars().take(max_width - 1).collect::<String>();
    truncated.push('…');
    truncated
}

fn compact_status_text(status: butterflow_models::TaskStatus, max_width: usize) -> String {
    let candidates: &[&str] = match status {
        butterflow_models::TaskStatus::AwaitingTrigger => {
            &["Awaiting trigger", "Awaiting", "Await"]
        }
        butterflow_models::TaskStatus::Running => &["Running", "Run"],
        butterflow_models::TaskStatus::Failed => &["Failed", "Fail"],
        butterflow_models::TaskStatus::Completed => &["Completed", "Done"],
        butterflow_models::TaskStatus::Pending => &["Pending", "Pend"],
        butterflow_models::TaskStatus::Blocked => &["Blocked", "Block"],
        butterflow_models::TaskStatus::WontDo => &["Won't do", "Skip"],
    };

    candidates
        .iter()
        .find(|candidate| candidate.chars().count() <= max_width)
        .map(|candidate| (*candidate).to_string())
        .unwrap_or_else(|| truncate_text(candidates.last().copied().unwrap_or(""), max_width))
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let (title, body, help_text) = match approval {
        ApprovalPrompt::WorktreeConsent { .. } => (
            "Trigger All".to_string(),
            "Trigger all pending tasks?\n\nThis will use git worktrees for the bulk run."
                .to_string(),
            "y/Enter approve  esc cancel".to_string(),
        ),
        ApprovalPrompt::PullRequestConsent { title, head, .. } => (
            "Create Pull Request".to_string(),
            format!("Create pull request for completed task?\n\nTitle: {title}\nBranch: {head}"),
            "y/Enter approve  esc cancel".to_string(),
        ),
        ApprovalPrompt::ManualPullRequestConsent { title, head, .. } => (
            "Create Pull Request".to_string(),
            format!("Create pull request now?\n\nTitle: {title}\nBranch: {head}"),
            "y/Enter approve  esc cancel".to_string(),
        ),
        ApprovalPrompt::Shell { command, .. } => (
            "Approval".to_string(),
            format!("Approve shell command?\n\n{command}"),
            "y approve  n/esc reject".to_string(),
        ),
        ApprovalPrompt::Capabilities { modules, .. } => (
            "Approval".to_string(),
            format!("Approve capabilities?\n\n{}", modules.join(", ")),
            "y approve  n/esc reject".to_string(),
        ),
        ApprovalPrompt::AgentSelection {
            options, selected, ..
        } => {
            let options_text = options
                .iter()
                .enumerate()
                .map(|(index, (label, _))| {
                    if index == *selected {
                        format!("▶ {label}")
                    } else {
                        format!("  {label}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                "Select Coding Agent".to_string(),
                options_text,
                "↑/↓ move  Enter choose  esc skip".to_string(),
            )
        }
        ApprovalPrompt::Selection {
            title,
            options,
            selected,
            ..
        } => {
            let options_text = options
                .iter()
                .enumerate()
                .map(|(index, (_, label))| {
                    if index == *selected {
                        format!("▶ {label}")
                    } else {
                        format!("  {label}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                title.clone(),
                options_text,
                "↑/↓ move  Enter choose  esc cancel".to_string(),
            )
        }
    };
    let modal = Paragraph::new(body).wrap(Wrap { trim: false }).block(
        Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    );
    frame.render_widget(modal, chunks[0]);
    render_help_bar(frame, chunks[1], &help_text);
}

#[cfg(test)]
mod tests {
    use super::{log_modal_copy_hint, render};
    use crate::tui::app::{Screen, TaskProgressView, TuiState};
    use butterflow_models::{Task, TaskStatus, Workflow, WorkflowRun, WorkflowStatus};
    use chrono::{Duration, Utc};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use uuid::Uuid;

    fn render_state(state: &TuiState, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn sample_run(
        name: &str,
        status: WorkflowStatus,
        started_at: chrono::DateTime<Utc>,
    ) -> WorkflowRun {
        WorkflowRun {
            id: Uuid::new_v4(),
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at,
            ended_at: None,
            capabilities: None,
            name: Some(name.to_string()),
            target_path: None,
        }
    }

    #[test]
    fn render_runs_keeps_elapsed_column_aligned() {
        let now = Utc::now();
        let mut state = TuiState::default();
        let mut first = sample_run(
            "debarrel",
            WorkflowStatus::Completed,
            now - Duration::minutes(4),
        );
        first.ended_at = Some(first.started_at + Duration::minutes(4) + Duration::seconds(8));
        let mut second = sample_run(
            "i18n-codemod",
            WorkflowStatus::Completed,
            now - Duration::minutes(13),
        );
        second.ended_at = Some(second.started_at + Duration::minutes(13) + Duration::seconds(51));
        let second_elapsed = TuiState::workflow_elapsed_text(&second);
        state.runs = vec![first, second];

        let lines = render_state(&state, 80, 12);
        let header = lines
            .iter()
            .find(|line| line.contains("Workflow") && line.contains("Elapsed"))
            .unwrap();
        let row = lines
            .iter()
            .find(|line| line.contains("i18n-codemod"))
            .unwrap();

        let header_elapsed = header.find("Elapsed").unwrap();
        let row_elapsed = row.find(&second_elapsed).unwrap();
        assert_eq!(
            row_elapsed + second_elapsed.len(),
            header_elapsed + "Elapsed".len()
        );
    }

    #[test]
    fn render_run_detail_shows_left_edge_selection_and_progress_bar() {
        let run_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.screen = Screen::RunDetail;
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status: WorkflowStatus::Running,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now() - Duration::minutes(5),
            ended_at: None,
            capabilities: None,
            name: Some("debarrel".to_string()),
            target_path: None,
        });
        state.tasks.push(Task {
            id: task_id,
            workflow_run_id: run_id,
            node_id: "apply-transforms".to_string(),
            status: TaskStatus::Running,
            started_at: Some(Utc::now() - Duration::minutes(1)),
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });
        state.task_progress.insert(
            task_id,
            TaskProgressView {
                processed_files: 3,
                total_files: Some(10),
            },
        );

        let lines = render_state(&state, 100, 14);
        let task_row = lines
            .iter()
            .find(|line| line.contains("apply-transforms") && line.contains('['))
            .unwrap();

        assert!(task_row.find("▶ ").is_some());
        assert!(task_row.contains('['));
        assert!(task_row.contains('>'));
        assert!(task_row.contains(']'));
    }

    #[test]
    fn log_modal_copy_hint_matches_platform() {
        if cfg!(target_os = "macos") {
            assert_eq!(log_modal_copy_hint(), "ctrl+c/cmd+c copy");
        } else {
            assert_eq!(log_modal_copy_hint(), "ctrl+c copy");
        }
    }

    #[test]
    fn render_log_modal_places_notice_below_help_bar() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState {
            screen: Screen::RunDetail,
            ..TuiState::default()
        };
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "apply-transforms".to_string(),
            status: TaskStatus::Running,
            started_at: Some(Utc::now() - Duration::minutes(1)),
            ended_at: None,
            logs: (0..8).map(|index| format!("line {index}")).collect(),
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });
        state.open_log_modal(6);
        state.set_log_modal_notice("Copied full log to clipboard");

        let lines = render_state(&state, 100, 20);
        let hint_line = lines
            .iter()
            .position(|line| line.contains("copy") && line.contains("close"))
            .unwrap();
        let notice_line = lines
            .iter()
            .position(|line| line.contains("Copied full log to clipboard"))
            .unwrap();

        assert!(notice_line > hint_line);
    }

    #[test]
    fn render_log_modal_title_includes_task_status() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState {
            screen: Screen::RunDetail,
            ..TuiState::default()
        };
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "install-skill".to_string(),
            status: TaskStatus::Failed,
            started_at: Some(Utc::now() - Duration::minutes(1)),
            ended_at: Some(Utc::now()),
            logs: vec!["boom".to_string()],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: Some("boom".to_string()),
        });
        state.open_log_modal(6);

        let lines = render_state(&state, 100, 20);
        assert!(lines
            .iter()
            .any(|line| line.contains("Logs: install-skill (Failed)")));
    }

    #[test]
    fn render_selection_modal_places_help_bar_at_bottom() {
        let state = TuiState {
            approval: Some(crate::tui::app::ApprovalPrompt::Selection {
                request_id: Uuid::new_v4(),
                title: "Choose install scope".to_string(),
                options: vec![
                    ("project".to_string(), "project".to_string()),
                    ("user".to_string(), "user (~/.claude/skills)".to_string()),
                ],
                selected: 0,
            }),
            ..TuiState::default()
        };

        let lines = render_state(&state, 80, 24);
        assert!(lines
            .iter()
            .any(|line| line.contains("Choose install scope")));
        assert!(lines
            .iter()
            .any(|line| line.contains("Enter") && line.contains("choose")));
    }

    #[test]
    fn render_worktree_consent_modal_text() {
        let state = TuiState {
            approval: Some(crate::tui::app::ApprovalPrompt::WorktreeConsent {
                task_ids: vec![Uuid::new_v4()],
            }),
            ..TuiState::default()
        };

        let lines = render_state(&state, 80, 24);
        assert!(lines.iter().any(|line| line.contains("Trigger All")));
        assert!(lines.iter().any(|line| line.contains("git worktrees")));
        assert!(lines
            .iter()
            .any(|line| line.contains("approve") && line.contains("cancel")));
    }

    #[test]
    fn render_manual_pull_request_consent_modal_text() {
        let state = TuiState {
            approval: Some(crate::tui::app::ApprovalPrompt::ManualPullRequestConsent {
                task_id: Uuid::new_v4(),
                title: "Draft PR".to_string(),
                head: "codemod-branch".to_string(),
            }),
            ..TuiState::default()
        };

        let lines = render_state(&state, 80, 24);
        assert!(lines
            .iter()
            .any(|line| line.contains("Create Pull Request")));
        assert!(lines.iter().any(|line| line.contains("codemod-branch")));
        assert!(lines
            .iter()
            .any(|line| line.contains("approve") && line.contains("cancel")));
    }
}
