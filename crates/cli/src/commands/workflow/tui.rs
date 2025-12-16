use std::collections::HashSet;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use butterflow_core::engine::Engine;
use butterflow_models::{Task, TaskStatus, WorkflowRun, WorkflowStatus};
use clap::Args;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
        Wrap,
    },
    Frame, Terminal,
};
use uuid::Uuid;

use crate::engine::create_engine;

#[derive(Args, Debug)]
pub struct Command {
    /// Number of workflow runs to show
    #[arg(short, long, default_value = "25")]
    limit: usize,

    /// Auto-refresh interval in seconds (0 to disable)
    #[arg(long, default_value = "1")]
    refresh_interval: u64,
}

/// Focus panel in the grid layout
#[derive(Debug, Clone, PartialEq, Copy)]
enum FocusPanel {
    Runs,
    Tasks,
    Triggers,
    Details,
}

impl FocusPanel {
    fn next(self) -> Self {
        match self {
            FocusPanel::Runs => FocusPanel::Tasks,
            FocusPanel::Tasks => FocusPanel::Triggers,
            FocusPanel::Triggers => FocusPanel::Details,
            FocusPanel::Details => FocusPanel::Runs,
        }
    }

    fn prev(self) -> Self {
        match self {
            FocusPanel::Runs => FocusPanel::Details,
            FocusPanel::Tasks => FocusPanel::Runs,
            FocusPanel::Triggers => FocusPanel::Tasks,
            FocusPanel::Details => FocusPanel::Triggers,
        }
    }
}

/// Trigger action type
#[derive(Debug, Clone)]
enum TriggerAction {
    All,
    Selected(Vec<Uuid>),
}

/// Popup dialog type
#[derive(Debug, Clone)]
enum Popup {
    None,
    ConfirmCancel(Uuid),
    ConfirmTrigger(TriggerAction),
    StatusMessage(String, Instant),
    Help,
}

/// Application state
struct App {
    engine: Engine,
    limit: usize,
    refresh_interval: Duration,
    last_refresh: Instant,

    // Focus state
    focus: FocusPanel,

    // Runs list state
    runs: Vec<WorkflowRun>,
    runs_state: TableState,

    // Selected run detail state
    selected_run: Option<WorkflowRun>,
    tasks: Vec<Task>,
    tasks_state: TableState,
    triggers_state: ListState,
    selected_triggers: HashSet<Uuid>,

    // Logs/output (simulated from task logs)
    logs: Vec<String>,
    log_scroll: usize,

    // UI state
    popup: Popup,
    error_message: Option<String>,
    should_quit: bool,
}

impl App {
    fn new(engine: Engine, limit: usize, refresh_interval: Duration) -> Self {
        let mut runs_state = TableState::default();
        runs_state.select(Some(0));

        Self {
            engine,
            limit,
            refresh_interval,
            last_refresh: Instant::now() - refresh_interval,
            focus: FocusPanel::Runs,
            runs: Vec::new(),
            runs_state,
            selected_run: None,
            tasks: Vec::new(),
            tasks_state: TableState::default(),
            triggers_state: ListState::default(),
            selected_triggers: HashSet::new(),
            logs: Vec::new(),
            log_scroll: 0,
            popup: Popup::None,
            error_message: None,
            should_quit: false,
        }
    }

    /// Check if it's time to refresh data
    fn should_refresh(&self) -> bool {
        if self.refresh_interval.is_zero() {
            return false;
        }
        self.last_refresh.elapsed() >= self.refresh_interval
    }

    /// Refresh all data
    async fn refresh(&mut self) -> Result<()> {
        self.error_message = None;

        // Refresh runs list
        match self.engine.list_workflow_runs(self.limit).await {
            Ok(runs) => {
                self.runs = runs;
                // Ensure selection is valid
                if !self.runs.is_empty() {
                    let max_idx = self.runs.len().saturating_sub(1);
                    if self.runs_state.selected().unwrap_or(0) > max_idx {
                        self.runs_state.select(Some(max_idx));
                    }
                }
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to list runs: {e}"));
            }
        }

        // Refresh selected run details
        if let Some(idx) = self.runs_state.selected() {
            if let Some(run) = self.runs.get(idx) {
                let run_id = run.id;

                match self.engine.get_workflow_run(run_id).await {
                    Ok(run) => {
                        self.selected_run = Some(run);
                    }
                    Err(e) => {
                        if self.error_message.is_none() {
                            self.error_message = Some(format!("Failed to get run: {e}"));
                        }
                    }
                }

                match self.engine.get_tasks(run_id).await {
                    Ok(tasks) => {
                        // Collect logs from tasks
                        self.logs = tasks
                            .iter()
                            .flat_map(|t| {
                                t.logs
                                    .iter()
                                    .map(|log| format!("[{}] {}", truncate(&t.node_id, 12), log))
                            })
                            .collect();

                        self.tasks = tasks;
                        // Ensure selection is valid
                        if !self.tasks.is_empty() && self.tasks_state.selected().is_none() {
                            self.tasks_state.select(Some(0));
                        }
                    }
                    Err(e) => {
                        if self.error_message.is_none() {
                            self.error_message = Some(format!("Failed to get tasks: {e}"));
                        }
                    }
                }
            }
        }

        self.last_refresh = Instant::now();
        Ok(())
    }

    /// Get awaiting trigger tasks
    fn get_awaiting_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::AwaitingTrigger)
            .collect()
    }

    /// Show a status message popup
    fn show_status(&mut self, msg: String) {
        self.popup = Popup::StatusMessage(msg, Instant::now());
    }

    /// Show confirmation for triggering all awaiting tasks
    fn trigger_all(&mut self) {
        let awaiting = self.get_awaiting_tasks();
        if awaiting.is_empty() {
            self.show_status("No tasks awaiting trigger".to_string());
            return;
        }
        self.popup = Popup::ConfirmTrigger(TriggerAction::All);
    }

    /// Actually trigger all awaiting tasks (after confirmation)
    async fn do_trigger_all(&mut self) -> Result<()> {
        if let Some(run) = &self.selected_run {
            match self.engine.trigger_all(run.id).await {
                Ok(triggered) => {
                    if triggered {
                        self.show_status("Triggered all awaiting tasks".to_string());
                    } else {
                        self.show_status("No tasks awaiting trigger".to_string());
                    }
                }
                Err(e) => {
                    self.error_message = Some(format!("Failed to trigger: {e}"));
                }
            }
            // Force refresh
            self.last_refresh = Instant::now() - self.refresh_interval - Duration::from_secs(1);
        }
        Ok(())
    }

    /// Show confirmation for triggering selected tasks
    fn trigger_selected(&mut self) {
        if self.selected_triggers.is_empty() {
            self.show_status("No tasks selected".to_string());
            return;
        }
        let task_ids: Vec<Uuid> = self.selected_triggers.iter().copied().collect();
        self.popup = Popup::ConfirmTrigger(TriggerAction::Selected(task_ids));
    }

    /// Actually trigger selected tasks (after confirmation)
    async fn do_trigger_selected(&mut self, task_ids: Vec<Uuid>) -> Result<()> {
        if let Some(run) = &self.selected_run {
            let count = task_ids.len();

            match self.engine.resume_workflow(run.id, task_ids).await {
                Ok(()) => {
                    self.show_status(format!("Triggered {count} task(s)"));
                    self.selected_triggers.clear();
                }
                Err(e) => {
                    self.error_message = Some(format!("Failed to trigger: {e}"));
                }
            }
            // Force refresh
            self.last_refresh = Instant::now() - self.refresh_interval - Duration::from_secs(1);
        }
        Ok(())
    }

    /// Cancel a workflow run
    async fn cancel_workflow(&mut self, run_id: Uuid) -> Result<()> {
        match self.engine.cancel_workflow(run_id).await {
            Ok(()) => {
                self.show_status("Workflow canceled".to_string());
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to cancel: {e}"));
            }
        }
        self.popup = Popup::None;
        // Force refresh
        self.last_refresh = Instant::now() - self.refresh_interval - Duration::from_secs(1);
        Ok(())
    }

    /// Handle keyboard input
    async fn handle_input(&mut self, key: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        // Handle popup dismissal
        match &self.popup {
            Popup::StatusMessage(_, _) | Popup::Help => {
                self.popup = Popup::None;
                return Ok(());
            }
            Popup::ConfirmCancel(run_id) => {
                let run_id = *run_id;
                match key {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        self.cancel_workflow(run_id).await?;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        self.popup = Popup::None;
                    }
                    _ => {}
                }
                return Ok(());
            }

            Popup::ConfirmTrigger(action) => {
                let action = action.clone();
                match key {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        self.popup = Popup::None;
                        match action {
                            TriggerAction::All => {
                                self.do_trigger_all().await?;
                            }
                            TriggerAction::Selected(task_ids) => {
                                self.do_trigger_selected(task_ids).await?;
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        self.popup = Popup::None;
                    }
                    _ => {}
                }
                return Ok(());
            }
            Popup::None => {}
        }

        // Global keys
        match key {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return Ok(());
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
                return Ok(());
            }
            KeyCode::Char('r') => {
                // Force refresh
                self.last_refresh = Instant::now() - self.refresh_interval - Duration::from_secs(1);
                return Ok(());
            }
            KeyCode::Char('?') => {
                self.popup = Popup::Help;
                return Ok(());
            }
            KeyCode::Tab => {
                self.focus = self.focus.next();
                return Ok(());
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev();
                return Ok(());
            }
            _ => {}
        }

        // Panel-specific keys
        match self.focus {
            FocusPanel::Runs => self.handle_runs_input(key).await?,
            FocusPanel::Tasks => self.handle_tasks_input(key),
            FocusPanel::Triggers => self.handle_triggers_input(key),
            FocusPanel::Details => self.handle_details_input(key),
        }

        Ok(())
    }

    /// Handle input in runs panel
    async fn handle_runs_input(&mut self, key: KeyCode) -> Result<()> {
        let len = self.runs.len();
        if len == 0 {
            return Ok(());
        }

        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.runs_state.selected().unwrap_or(0);
                self.runs_state.select(Some(i.saturating_sub(1)));
                // Reset tasks selection when changing run
                self.tasks_state = TableState::default();
                self.triggers_state = ListState::default();
                self.selected_triggers.clear();
                // Force refresh for new run
                self.last_refresh = Instant::now() - self.refresh_interval - Duration::from_secs(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.runs_state.selected().unwrap_or(0);
                self.runs_state.select(Some((i + 1).min(len - 1)));
                // Reset tasks selection when changing run
                self.tasks_state = TableState::default();
                self.triggers_state = ListState::default();
                self.selected_triggers.clear();
                // Force refresh for new run
                self.last_refresh = Instant::now() - self.refresh_interval - Duration::from_secs(1);
            }
            KeyCode::Char('c') => {
                // Cancel selected workflow
                if let Some(i) = self.runs_state.selected() {
                    if let Some(run) = self.runs.get(i) {
                        if run.status == WorkflowStatus::Running
                            || run.status == WorkflowStatus::AwaitingTrigger
                        {
                            self.popup = Popup::ConfirmCancel(run.id);
                        } else {
                            self.show_status(
                                "Can only cancel Running or AwaitingTrigger".to_string(),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle input in tasks panel
    fn handle_tasks_input(&mut self, key: KeyCode) {
        let len = self.tasks.len();
        if len == 0 {
            return;
        }

        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.tasks_state.selected().unwrap_or(0);
                self.tasks_state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.tasks_state.selected().unwrap_or(0);
                self.tasks_state.select(Some((i + 1).min(len - 1)));
            }
            _ => {}
        }
    }

    /// Handle input in triggers panel
    fn handle_triggers_input(&mut self, key: KeyCode) {
        let awaiting = self.get_awaiting_tasks();
        let len = awaiting.len();

        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                if len > 0 {
                    let i = self.triggers_state.selected().unwrap_or(0);
                    self.triggers_state.select(Some(i.saturating_sub(1)));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 {
                    let i = self.triggers_state.selected().unwrap_or(0);
                    self.triggers_state.select(Some((i + 1).min(len - 1)));
                }
            }
            KeyCode::Char(' ') => {
                // Toggle selection
                if let Some(i) = self.triggers_state.selected() {
                    if let Some(task) = awaiting.get(i) {
                        let task_id = task.id;
                        if self.selected_triggers.contains(&task_id) {
                            self.selected_triggers.remove(&task_id);
                        } else {
                            self.selected_triggers.insert(task_id);
                        }
                    }
                }
            }
            KeyCode::Char('a') => {
                self.trigger_all();
            }
            KeyCode::Char('t') | KeyCode::Enter => {
                self.trigger_selected();
            }
            _ => {}
        }
    }

    /// Handle input in details panel (scroll logs)
    fn handle_details_input(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.log_scroll < self.logs.len().saturating_sub(1) {
                    self.log_scroll += 1;
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.log_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.log_scroll = self.logs.len().saturating_sub(1);
            }
            _ => {}
        }
    }
}

/// Get color for workflow status
fn status_color(status: WorkflowStatus) -> Color {
    match status {
        WorkflowStatus::Running => Color::Green,
        WorkflowStatus::Completed => Color::Cyan,
        WorkflowStatus::Failed => Color::Red,
        WorkflowStatus::AwaitingTrigger => Color::Yellow,
        WorkflowStatus::Canceled => Color::DarkGray,
        WorkflowStatus::Pending => Color::Blue,
    }
}

/// Get color for task status
fn task_status_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Running => Color::Green,
        TaskStatus::Completed => Color::Cyan,
        TaskStatus::Failed => Color::Red,
        TaskStatus::AwaitingTrigger => Color::Yellow,
        TaskStatus::Blocked => Color::Magenta,
        TaskStatus::WontDo => Color::DarkGray,
        TaskStatus::Pending => Color::Blue,
    }
}

/// Format duration from seconds
fn format_duration(seconds: i64) -> String {
    if seconds < 0 {
        return "-".to_string();
    }
    let secs = seconds as u64;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Truncate string to max length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}

/// Get block style based on focus
fn block_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Render the runs list panel (top-left)
fn render_runs_panel(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let header_cells = ["ID", "Status", "Name"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).bold()));
    let header_row = Row::new(header_cells).height(1);

    let rows = app.runs.iter().map(|run| {
        let name = run
            .workflow
            .nodes
            .first()
            .map(|n| truncate(&n.name, 15))
            .unwrap_or_else(|| "unknown".to_string());

        let status_str = match run.status {
            WorkflowStatus::Running => "●",
            WorkflowStatus::Completed => "✓",
            WorkflowStatus::Failed => "✗",
            WorkflowStatus::AwaitingTrigger => "◎",
            WorkflowStatus::Canceled => "○",
            WorkflowStatus::Pending => "◌",
        };

        Row::new(vec![
            Cell::from(truncate(&run.id.to_string(), 8)),
            Cell::from(status_str).style(Style::default().fg(status_color(run.status))),
            Cell::from(name),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Min(10),
        ],
    )
    .header(header_row)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Workflow Runs ({}) ", app.runs.len()))
            .border_style(block_style(focused)),
    )
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .highlight_symbol("▶ ");

    f.render_stateful_widget(table, area, &mut app.runs_state);
}

/// Render the tasks panel (top-right)
fn render_tasks_panel(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let header_cells = ["Node", "Status"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).bold()));
    let header_row = Row::new(header_cells).height(1);

    let rows = app.tasks.iter().map(|task| {
        let status_str = match task.status {
            TaskStatus::Running => "●",
            TaskStatus::Completed => "✓",
            TaskStatus::Failed => "✗",
            TaskStatus::AwaitingTrigger => "◎",
            TaskStatus::Blocked => "◇",
            TaskStatus::WontDo => "○",
            TaskStatus::Pending => "◌",
        };

        Row::new(vec![
            Cell::from(truncate(&task.node_id, 20)),
            Cell::from(status_str).style(Style::default().fg(task_status_color(task.status))),
        ])
    });

    let title = if let Some(run) = &app.selected_run {
        format!(
            " Tasks ({}) - {} ",
            app.tasks.len(),
            truncate(&run.id.to_string(), 8)
        )
    } else {
        " Tasks ".to_string()
    };

    let table = Table::new(rows, [Constraint::Min(15), Constraint::Length(3)])
        .header(header_row)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(block_style(focused)),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(table, area, &mut app.tasks_state);
}

/// Render the triggers panel (bottom-left)
fn render_triggers_panel(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let awaiting_tasks = app.get_awaiting_tasks();

    if awaiting_tasks.is_empty() {
        let no_triggers = Paragraph::new("No tasks awaiting trigger")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Manual Triggers ")
                    .border_style(block_style(focused)),
            );
        f.render_widget(no_triggers, area);
    } else {
        let trigger_items: Vec<ListItem> = awaiting_tasks
            .iter()
            .map(|task| {
                let is_selected = app.selected_triggers.contains(&task.id);
                let checkbox = if is_selected { "[✓]" } else { "[ ]" };

                let matrix_info = task
                    .matrix_values
                    .as_ref()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();

                let content = if matrix_info.is_empty() {
                    format!("{} {}", checkbox, task.node_id)
                } else {
                    format!(
                        "{} {} ({})",
                        checkbox,
                        task.node_id,
                        truncate(&matrix_info, 15)
                    )
                };

                let style = if is_selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                };

                ListItem::new(content).style(style)
            })
            .collect();

        let triggers_list = List::new(trigger_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(
                        " Triggers ({}) [a:all t:selected] ",
                        awaiting_tasks.len()
                    ))
                    .border_style(block_style(focused)),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");

        f.render_stateful_widget(triggers_list, area, &mut app.triggers_state);
    }
}

/// Render the details/logs panel (bottom-right)
fn render_details_panel(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let inner_height = area.height.saturating_sub(2) as usize;

    let content: Vec<Line> = if let Some(run) = &app.selected_run {
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

        let mut lines = vec![
            Line::from(vec![
                Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
                Span::raw(name.clone()),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:?}", run.status),
                    Style::default().fg(status_color(run.status)).bold(),
                ),
            ]),
            Line::from(vec![
                Span::styled("Duration: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format_duration(duration)),
            ]),
            Line::from(vec![
                Span::styled("Started: ", Style::default().fg(Color::DarkGray)),
                Span::raw(run.started_at.format("%Y-%m-%d %H:%M:%S").to_string()),
            ]),
            Line::from(""),
            Line::styled("─── Logs ───", Style::default().fg(Color::DarkGray)),
        ];

        // Add logs with scrolling
        let log_start = app.log_scroll.min(app.logs.len().saturating_sub(1));
        for log in app
            .logs
            .iter()
            .skip(log_start)
            .take(inner_height.saturating_sub(6))
        {
            lines.push(Line::from(Span::styled(
                truncate(log, area.width.saturating_sub(4) as usize),
                Style::default().fg(Color::White),
            )));
        }

        lines
    } else {
        vec![Line::from(Span::styled(
            "Select a workflow run",
            Style::default().fg(Color::DarkGray),
        ))]
    };

    let details = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Details & Logs ")
                .border_style(block_style(focused)),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(details, area);
}

/// Render error bar if there's an error
fn render_error_bar(f: &mut Frame, app: &App, area: Rect) {
    if let Some(err) = &app.error_message {
        let error_bar = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::White).bg(Color::Red))
            .wrap(Wrap { trim: true });
        f.render_widget(error_bar, area);
    }
}

/// Render footer with keybindings
fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let focus_indicator = match app.focus {
        FocusPanel::Runs => "Runs",
        FocusPanel::Tasks => "Tasks",
        FocusPanel::Triggers => "Triggers",
        FocusPanel::Details => "Logs",
    };

    let text = format!(
        " [{}] Tab:Switch │ ↑↓/jk:Navigate │ c:Cancel │ r:Refresh │ ?:Help │ q:Quit ",
        focus_indicator
    );

    let footer = Paragraph::new(text)
        .style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(footer, area);
}

/// Render popup dialogs
fn render_popup(f: &mut Frame, app: &App) {
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
                        .title(" Confirm Cancel ")
                        .border_style(Style::default().fg(Color::Yellow)),
                )
                .alignment(ratatui::layout::Alignment::Center);
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
                TriggerAction::Selected(ids) => (
                    " Trigger Selected ",
                    format!("Trigger {} selected task(s)?", ids.len()),
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
                        .title(title)
                        .border_style(Style::default().fg(Color::Green)),
                )
                .alignment(ratatui::layout::Alignment::Center);
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
                        .title(" Status ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .alignment(ratatui::layout::Alignment::Center);
            f.render_widget(popup, popup_area);
        }
        Popup::Help => {
            let popup_area = centered_rect(70, 70, f.area());
            f.render_widget(Clear, popup_area);

            let text = vec![
                Line::from(""),
                Line::styled(" Navigation ", Style::default().bold().fg(Color::Yellow)),
                Line::from("  Tab / Shift+Tab  Switch between panels"),
                Line::from("  ↑/k ↓/j          Navigate within panel"),
                Line::from(""),
                Line::styled(" Actions ", Style::default().bold().fg(Color::Yellow)),
                Line::from("  c                Cancel selected workflow"),
                Line::from("  r                Force refresh"),
                Line::from("  Space            Toggle trigger selection"),
                Line::from("  a                Trigger all awaiting"),
                Line::from("  t / Enter        Trigger selected"),
                Line::from(""),
                Line::styled(" General ", Style::default().bold().fg(Color::Yellow)),
                Line::from("  ?                Show this help"),
                Line::from("  q / Ctrl+C       Quit"),
                Line::from(""),
                Line::from(Span::styled(
                    "Press any key to close",
                    Style::default().fg(Color::DarkGray),
                )),
            ];

            let popup = Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Help ")
                    .border_style(Style::default().fg(Color::Cyan)),
            );
            f.render_widget(popup, popup_area);
        }
    }
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

/// Render the UI in a grid layout
fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Main layout: content + footer + optional error bar
    let main_chunks = if app.error_message.is_some() {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),   // Main grid
                Constraint::Length(1), // Footer
                Constraint::Length(1), // Error bar
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),   // Main grid
                Constraint::Length(1), // Footer
            ])
            .split(area)
    };

    let content_area = main_chunks[0];
    let footer_area = main_chunks[1];

    // Split content into top and bottom rows
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(content_area);

    // Split top row: Runs (left) | Tasks (right)
    let top_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[0]);

    // Split bottom row: Triggers (left) | Details (right)
    let bottom_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[1]);

    // Render all panels
    render_runs_panel(f, app, top_cols[0], app.focus == FocusPanel::Runs);
    render_tasks_panel(f, app, top_cols[1], app.focus == FocusPanel::Tasks);
    render_triggers_panel(f, app, bottom_cols[0], app.focus == FocusPanel::Triggers);
    render_details_panel(f, app, bottom_cols[1], app.focus == FocusPanel::Details);

    // Render footer
    render_footer(f, app, footer_area);

    // Render error bar if present
    if app.error_message.is_some() && main_chunks.len() > 2 {
        render_error_bar(f, app, main_chunks[2]);
    }

    // Render popup if any
    render_popup(f, app);
}

/// Run the TUI event loop
async fn run_tui(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        // Refresh data if needed
        if app.should_refresh() {
            app.refresh().await?;
        }

        // Auto-dismiss status messages after 2 seconds
        if let Popup::StatusMessage(_, instant) = &app.popup {
            if instant.elapsed() > Duration::from_secs(2) {
                app.popup = Popup::None;
            }
        }

        // Render
        terminal.draw(|f| ui(f, app))?;

        // Handle events with timeout for periodic refresh
        let timeout = if app.refresh_interval.is_zero() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(100).min(app.refresh_interval)
        };

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                app.handle_input(key.code, key.modifiers).await?;
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Initialize and run the TUI
pub async fn handler(args: &Command) -> Result<()> {
    // Create engine for read-only operations
    let (engine, _) = create_engine(
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        None,
        None,
        false,
    )?;

    let refresh_interval = if args.refresh_interval == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(args.refresh_interval)
    };

    let mut app = App::new(engine, args.limit, refresh_interval);

    // Setup terminal
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to setup terminal")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    // Run the TUI
    let result = run_tui(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .context("Failed to restore terminal")?;
    terminal.show_cursor().context("Failed to show cursor")?;

    result
}
