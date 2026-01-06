use std::io::Write;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use butterflow_core::engine::Engine;
use butterflow_models::{Task, TaskStatus, WorkflowRun, WorkflowStatus};
use crossterm::event::{KeyCode, KeyModifiers};
use portable_pty::MasterPty;
use ratatui::widgets::TableState;
use uuid::Uuid;

use super::pty::run_resume_command_in_terminal;
use super::types::{Popup, Screen, TriggerAction};

/// Application state
pub struct App {
    pub engine: Engine,
    pub limit: usize,
    pub refresh_interval: Duration,
    pub last_refresh: Instant,

    // Current screen
    pub screen: Screen,

    // Runs list state
    pub runs: Vec<WorkflowRun>,
    pub runs_state: TableState,

    // Selected run detail state
    pub selected_run: Option<WorkflowRun>,
    pub tasks: Vec<Task>,
    pub tasks_state: TableState,

    // Selected task for actions screen
    pub selected_task: Option<Task>,

    // Logs scroll
    pub log_scroll: usize,

    // Terminal scroll for terminal view
    pub terminal_scroll: usize,

    // UI state
    pub popup: Popup,
    pub should_quit: bool,

    // Track triggered task to monitor its execution
    pub monitoring_task: Option<Uuid>,

    // Track workflow run being monitored for completion
    pub monitoring_workflow: Option<Uuid>,

    // Terminal view state - task being shown in terminal
    pub terminal_task: Option<Uuid>,

    // PTY-based terminal state
    /// VT100 parser for terminal emulation (handles escape sequences, cursor, colors, etc.)
    pub terminal_parser: Arc<RwLock<vt100::Parser>>,
    /// Writer to send input to the PTY
    pub pty_writer: Option<Box<dyn Write + Send>>,
    /// Master PTY handle for resizing
    pub pty_master: Option<Box<dyn MasterPty + Send>>,
    /// Current terminal size (rows, cols)
    pub terminal_size: (u16, u16),
    /// Total visual lines in logs
    pub total_log_lines: usize,
    /// Height of logs view
    pub log_height: u16,
    /// Flag indicating if PTY process is still running
    pub pty_running: Arc<Mutex<bool>>,
    /// Insert mode flag for terminal - when true, all keystrokes go to PTY
    pub insert_mode: bool,

    /// Start time for animations
    pub start_time: Instant,

    // Command-line flags
    /// Dry run mode - don't make actual changes
    pub dry_run: bool,
    /// Allow fs access
    pub allow_fs: bool,
    /// Allow fetch access
    pub allow_fetch: bool,
    /// Allow child process access
    pub allow_child_process: bool,
}

impl App {
    pub fn new(
        engine: Engine,
        limit: usize,
        refresh_interval: Duration,
        dry_run: bool,
        allow_fs: bool,
        allow_fetch: bool,
        allow_child_process: bool,
    ) -> Self {
        let mut runs_state = TableState::default();
        runs_state.select(Some(0));

        Self {
            engine,
            limit,
            refresh_interval,
            last_refresh: Instant::now() - refresh_interval,
            screen: Screen::Workflows,
            runs: Vec::new(),
            runs_state,
            selected_run: None,
            tasks: Vec::new(),
            tasks_state: TableState::default(),
            selected_task: None,
            log_scroll: 0,
            terminal_scroll: 0,
            popup: Popup::None,
            should_quit: false,
            monitoring_task: None,
            monitoring_workflow: None,
            terminal_task: None,
            // Initialize PTY state with default terminal size
            terminal_parser: Arc::new(RwLock::new(vt100::Parser::new(24, 80, 1000))),
            pty_writer: None,
            pty_master: None,
            terminal_size: (24, 80),
            total_log_lines: 0,
            log_height: 20, // Default estimate
            pty_running: Arc::new(Mutex::new(false)),
            insert_mode: false,
            start_time: Instant::now(),
            dry_run,
            allow_fs,
            allow_fetch,
            allow_child_process,
        }
    }

    /// Check if it's time to refresh data
    pub fn should_refresh(&self) -> bool {
        if self.refresh_interval.is_zero() {
            return false;
        }
        self.last_refresh.elapsed() >= self.refresh_interval
    }

    /// Refresh data based on current screen
    pub async fn refresh(&mut self) -> Result<()> {
        // Don't clear error popup on refresh, let user dismiss it

        match self.screen {
            Screen::Workflows => {
                self.refresh_runs().await?;
            }
            Screen::Tasks => {
                self.refresh_runs().await?;
                self.refresh_tasks().await?;
            }
            Screen::Terminal => {
                self.refresh_runs().await?;
                self.refresh_tasks().await?;
                // PTY output is handled by the vt100 parser - no manual scrolling needed
            }
            Screen::Actions => {
                self.refresh_runs().await?;
                self.refresh_tasks().await?;

                // Check workflow status if monitoring
                if let Some(workflow_id) = self.monitoring_workflow {
                    if let Ok(status) = self.engine.get_workflow_status(workflow_id).await {
                        match status {
                            WorkflowStatus::Completed => {
                                self.monitoring_workflow = None;
                                self.monitoring_task = None;
                                self.show_status("✅ Workflow completed successfully".to_string());
                            }
                            WorkflowStatus::Failed => {
                                self.monitoring_workflow = None;
                                self.monitoring_task = None;
                                self.show_status("❌ Workflow failed".to_string());
                            }
                            WorkflowStatus::Canceled => {
                                self.monitoring_workflow = None;
                                self.monitoring_task = None;
                                self.show_status("❌ Workflow was canceled".to_string());
                            }
                            WorkflowStatus::AwaitingTrigger => {
                                // Check if there are still tasks awaiting trigger
                                if let Ok(tasks) = self.engine.get_tasks(workflow_id).await {
                                    let awaiting_count = tasks
                                        .iter()
                                        .filter(|t| t.status == TaskStatus::AwaitingTrigger)
                                        .count();
                                    if awaiting_count > 0 {
                                        self.monitoring_workflow = None;
                                        self.monitoring_task = None;
                                        self.show_status(format!(
                                            "⏸️ Workflow paused: {} task(s) awaiting manual trigger",
                                            awaiting_count
                                        ));
                                    }
                                }
                            }
                            _ => {
                                // Still running, continue monitoring
                            }
                        }
                    }
                }

                if let Some(selected) = &self.selected_task {
                    let task_id = selected.id;
                    let old_status = selected.status;
                    let old_logs_count = selected.logs.len();
                    let monitoring_id = self.monitoring_task;

                    if let Some(task) = self.tasks.iter().find(|t| t.id == task_id) {
                        self.selected_task = Some(task.clone());

                        // Check if we're monitoring this task
                        if let Some(mon_id) = monitoring_id {
                            if mon_id == task_id {
                                let new_logs_count = task.logs.len();
                                let new_status = task.status;

                                // Check if status changed to terminal state
                                if old_status != new_status
                                    && (new_status == TaskStatus::Completed
                                        || new_status == TaskStatus::Failed)
                                {
                                    // Don't clear monitoring_task here, let workflow monitoring handle it
                                    let status_msg = if new_status == TaskStatus::Completed {
                                        "Task completed successfully".to_string()
                                    } else {
                                        "Task failed".to_string()
                                    };
                                    // Auto-scroll to bottom of logs to see latest output
                                    self.log_scroll = new_logs_count.saturating_sub(1);
                                    // Only show status if workflow is not being monitored
                                    if self.monitoring_workflow.is_none() {
                                        self.show_status(status_msg);
                                    }
                                } else if new_logs_count > old_logs_count {
                                    // Auto-scroll to bottom if new logs appeared
                                    self.log_scroll = new_logs_count.saturating_sub(1);
                                }
                            }
                        }
                    } else {
                        self.selected_task = None;
                    }
                }
            }
        }

        self.last_refresh = Instant::now();
        Ok(())
    }

    pub async fn refresh_runs(&mut self) -> Result<()> {
        let selected_id = self
            .runs_state
            .selected()
            .and_then(|idx| self.runs.get(idx).map(|r| r.id));

        match self.engine.list_workflow_runs(self.limit).await {
            Ok(mut runs) => {
                runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                self.runs = runs;

                if !self.runs.is_empty() {
                    let new_idx = if let Some(id) = selected_id {
                        self.runs.iter().position(|r| r.id == id).unwrap_or(0)
                    } else {
                        0
                    };
                    self.runs_state.select(Some(new_idx));
                } else {
                    self.runs_state.select(None);
                }
            }
            Err(e) => {
                let error_msg = format!("Failed to list runs: {}", e);
                self.popup = Popup::Error(error_msg);
            }
        }
        Ok(())
    }

    pub async fn refresh_tasks(&mut self) -> Result<()> {
        if let Some(idx) = self.runs_state.selected() {
            if let Some(run) = self.runs.get(idx) {
                let run_id = run.id;
                let selected_task_id = self
                    .tasks_state
                    .selected()
                    .and_then(|idx| self.tasks.get(idx).map(|t| t.id));

                match self.engine.get_workflow_run(run_id).await {
                    Ok(run) => {
                        self.selected_run = Some(run);
                    }
                    Err(e) => {
                        if let Popup::None = self.popup {
                            let error_msg = format!("Failed to get run: {}", e);
                            self.popup = Popup::Error(error_msg);
                        }
                    }
                }

                match self.engine.get_tasks(run_id).await {
                    Ok(mut tasks) => {
                        tasks.sort_by(|a, b| {
                            let status_order = |s: TaskStatus| match s {
                                TaskStatus::Running => 0,
                                TaskStatus::Pending => 1,
                                TaskStatus::AwaitingTrigger => 2,
                                TaskStatus::Blocked => 3,
                                TaskStatus::Completed => 4,
                                TaskStatus::Failed => 5,
                                TaskStatus::WontDo => 6,
                            };
                            let matrix_cmp = |a: &Option<
                                std::collections::HashMap<String, serde_json::Value>,
                            >,
                                              b: &Option<
                                std::collections::HashMap<String, serde_json::Value>,
                            >| {
                                match (a, b) {
                                    (None, None) => std::cmp::Ordering::Equal,
                                    (None, Some(_)) => std::cmp::Ordering::Less,
                                    (Some(_), None) => std::cmp::Ordering::Greater,
                                    (Some(a_map), Some(b_map)) => {
                                        let mut a_vec: Vec<_> = a_map.iter().collect();
                                        let mut b_vec: Vec<_> = b_map.iter().collect();

                                        a_vec.sort_by(|(k1, v1), (k2, v2)| {
                                            k1.cmp(k2).then_with(|| {
                                                serde_json::to_string(v1).unwrap_or_default().cmp(
                                                    &serde_json::to_string(v2).unwrap_or_default(),
                                                )
                                            })
                                        });
                                        b_vec.sort_by(|(k1, v1), (k2, v2)| {
                                            k1.cmp(k2).then_with(|| {
                                                serde_json::to_string(v1).unwrap_or_default().cmp(
                                                    &serde_json::to_string(v2).unwrap_or_default(),
                                                )
                                            })
                                        });

                                        for ((ak, av), (bk, bv)) in a_vec.iter().zip(b_vec.iter()) {
                                            match ak.cmp(bk) {
                                                std::cmp::Ordering::Equal => {
                                                    let a_str = serde_json::to_string(av)
                                                        .unwrap_or_default();
                                                    let b_str = serde_json::to_string(bv)
                                                        .unwrap_or_default();
                                                    match a_str.cmp(&b_str) {
                                                        std::cmp::Ordering::Equal => continue,
                                                        other => return other,
                                                    }
                                                }
                                                other => return other,
                                            }
                                        }
                                        a_vec.len().cmp(&b_vec.len())
                                    }
                                }
                            };
                            status_order(a.status)
                                .cmp(&status_order(b.status))
                                .then_with(|| {
                                    (a.is_master, b.is_master).cmp(&(false, false)).reverse()
                                })
                                .then_with(|| match (a.master_task_id, b.master_task_id) {
                                    (Some(a_master), Some(b_master)) => a_master.cmp(&b_master),
                                    (Some(_), None) => std::cmp::Ordering::Less,
                                    (None, Some(_)) => std::cmp::Ordering::Greater,
                                    (None, None) => std::cmp::Ordering::Equal,
                                })
                                .then_with(|| matrix_cmp(&a.matrix_values, &b.matrix_values))
                                .then_with(|| a.node_id.cmp(&b.node_id))
                                .then_with(|| a.id.cmp(&b.id))
                        });
                        self.tasks = tasks;

                        if !self.tasks.is_empty() {
                            let new_idx = if let Some(id) = selected_task_id {
                                self.tasks.iter().position(|t| t.id == id)
                            } else {
                                None
                            };

                            if let Some(idx) = new_idx {
                                let max_idx = self.tasks.len().saturating_sub(1);
                                self.tasks_state.select(Some(idx.min(max_idx)));
                                // Update selected_task with the latest task data
                                if let Some(task) = self.tasks.get(idx) {
                                    self.selected_task = Some(task.clone());
                                }
                            } else if self.tasks_state.selected().is_some() {
                                let old_idx = self.tasks_state.selected().unwrap_or(0);
                                let max_idx = self.tasks.len().saturating_sub(1);
                                let selected_idx = old_idx.min(max_idx);
                                self.tasks_state.select(Some(selected_idx));
                                // Update selected_task with the latest task data
                                if let Some(task) = self.tasks.get(selected_idx) {
                                    self.selected_task = Some(task.clone());
                                }
                            } else {
                                self.tasks_state.select(Some(0));
                                // Update selected_task with the latest task data
                                if let Some(task) = self.tasks.first() {
                                    self.selected_task = Some(task.clone());
                                }
                            }
                        } else {
                            self.tasks_state.select(None);
                            self.selected_task = None;
                        }
                    }
                    Err(e) => {
                        if let Popup::None = self.popup {
                            let error_msg = format!("Failed to get tasks: {}", e);
                            self.popup = Popup::Error(error_msg);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Get awaiting trigger tasks
    pub fn get_awaiting_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::AwaitingTrigger)
            .collect()
    }

    /// Show a status message popup
    pub fn show_status(&mut self, msg: String) {
        self.popup = Popup::StatusMessage(msg, Instant::now());
    }

    /// Navigate to next screen
    pub async fn go_forward(&mut self) -> Result<()> {
        match self.screen {
            Screen::Workflows => {
                if self.runs_state.selected().is_some() && !self.runs.is_empty() {
                    self.screen = Screen::Tasks;
                    self.tasks_state = TableState::default();
                    self.tasks_state.select(Some(0));
                    // Force refresh for tasks
                    self.last_refresh =
                        Instant::now() - self.refresh_interval - Duration::from_secs(1);
                }
            }
            Screen::Tasks => {
                if let Some(idx) = self.tasks_state.selected() {
                    if let Some(task) = self.tasks.get(idx) {
                        self.selected_task = Some(task.clone());
                        self.screen = Screen::Actions;
                        self.log_scroll = 0;
                    }
                }
            }
            Screen::Actions => {
                // Can navigate to terminal if task is being monitored
                if self.monitoring_task.is_some() {
                    if let Some(task) = &self.selected_task {
                        self.terminal_task = Some(task.id);
                        self.screen = Screen::Terminal;
                        self.terminal_scroll = task.logs.len().saturating_sub(1);
                    }
                }
            }
            Screen::Terminal => {
                // Already at terminal, no action
            }
        }
        Ok(())
    }

    /// Navigate to previous screen
    pub fn go_back(&mut self) {
        match self.screen {
            Screen::Workflows => {
                // Already at first screen, quit or do nothing
            }
            Screen::Tasks => {
                self.screen = Screen::Workflows;
                self.selected_run = None;
                self.tasks.clear();
                self.tasks_state = TableState::default();
            }
            Screen::Actions => {
                self.screen = Screen::Tasks;
                self.selected_task = None;
            }
            Screen::Terminal => {
                self.screen = Screen::Actions;
                self.terminal_task = None;
                self.terminal_scroll = 0;
            }
        }
    }

    /// Trigger all awaiting tasks
    pub fn trigger_all(&mut self) {
        let awaiting = self.get_awaiting_tasks();
        if awaiting.is_empty() {
            self.show_status("No tasks awaiting trigger".to_string());
            return;
        }
        self.popup = Popup::ConfirmTrigger(TriggerAction::All);
    }

    /// Trigger the currently selected task
    pub fn trigger_current_task(&mut self) {
        if let Some(task) = &self.selected_task {
            if task.status == TaskStatus::AwaitingTrigger {
                self.popup = Popup::ConfirmTrigger(TriggerAction::Single(task.id));
            } else {
                self.show_status("Task is not awaiting trigger".to_string());
            }
        }
    }

    pub async fn do_trigger_all(&mut self) -> Result<()> {
        if let Some(run) = &self.selected_run {
            let run_id = run.id;
            let run_clone = run.clone();

            // Get workflow path
            let bundle_path = run_clone.bundle_path.as_ref();
            let workflow_file_path = bundle_path
                .map(|p| {
                    let workflow_yaml = p.join("workflow.yaml");
                    if workflow_yaml.exists() {
                        workflow_yaml
                    } else {
                        let butterflow_yaml = p.join("butterflow.yaml");
                        if butterflow_yaml.exists() {
                            butterflow_yaml
                        } else {
                            p.join("workflow.yaml")
                        }
                    }
                })
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| {
                        // Fallback to /tmp if current directory is unavailable
                        std::path::PathBuf::from("/tmp")
                    })
                });

            // Switch to terminal screen and run command
            self.terminal_task = None;
            self.screen = Screen::Terminal;
            self.terminal_scroll = 0;

            // Run workflow resume command in background
            let target_path = run_clone.target_path.as_deref();
            match run_resume_command_in_terminal(
                self,
                &workflow_file_path,
                run_id,
                None,
                true,
                target_path,
            ) {
                Ok(()) => {
                    self.monitoring_workflow = Some(run_id);
                    self.last_refresh = Instant::now() - self.refresh_interval;
                }
                Err(e) => {
                    let error_msg = format!("Failed to run command: {}", e);
                    self.popup = Popup::Error(error_msg);
                }
            }
        } else {
            self.show_status("No workflow run selected".to_string());
        }
        Ok(())
    }

    pub async fn do_trigger_single(&mut self, task_id: Uuid) -> Result<()> {
        if let Some(run) = &self.selected_run {
            let run_id = run.id;
            let run_clone = run.clone();

            // Get workflow path
            let bundle_path = run_clone.bundle_path.as_ref();
            let workflow_file_path = bundle_path
                .map(|p| {
                    let workflow_yaml = p.join("workflow.yaml");
                    if workflow_yaml.exists() {
                        workflow_yaml
                    } else {
                        let butterflow_yaml = p.join("butterflow.yaml");
                        if butterflow_yaml.exists() {
                            butterflow_yaml
                        } else {
                            p.join("workflow.yaml")
                        }
                    }
                })
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| {
                        // Fallback to /tmp if current directory is unavailable
                        std::path::PathBuf::from("/tmp")
                    })
                });

            // Switch to terminal screen and run command
            self.terminal_task = Some(task_id);
            self.screen = Screen::Terminal;
            self.terminal_scroll = 0;

            // Run workflow resume command in background
            let target_path = run_clone.target_path.as_deref();
            match run_resume_command_in_terminal(
                self,
                &workflow_file_path,
                run_id,
                Some(vec![task_id]),
                false,
                target_path,
            ) {
                Ok(()) => {
                    self.monitoring_task = Some(task_id);
                    self.monitoring_workflow = Some(run_id);
                    self.last_refresh = Instant::now() - self.refresh_interval;
                }
                Err(e) => {
                    let error_msg = format!("Failed to run command: {}", e);
                    self.popup = Popup::Error(error_msg);
                }
            }
        } else {
            self.show_status("No workflow run selected".to_string());
        }
        Ok(())
    }

    pub async fn cancel_workflow(&mut self, run_id: Uuid) -> Result<()> {
        match self.engine.cancel_workflow(run_id).await {
            Ok(()) => {
                self.show_status("Workflow canceled".to_string());
            }
            Err(e) => {
                let error_msg = format!("Failed to cancel workflow: {}", e);
                self.popup = Popup::Error(error_msg);
            }
        }
        self.popup = Popup::None;
        self.last_refresh = Instant::now() - self.refresh_interval - Duration::from_secs(1);
        Ok(())
    }

    /// Handle keyboard input
    pub async fn handle_input(&mut self, key: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        // Handle popup dismissal first
        match &mut self.popup {
            Popup::StatusMessage(_, _) | Popup::Help | Popup::Error(_) => {
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
                            TriggerAction::Single(task_id) => {
                                self.do_trigger_single(task_id).await?;
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
            Popup::ConfirmQuit => {
                match key {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        self.should_quit = true;
                        self.popup = Popup::None;
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

        // If in insert mode on Terminal screen, forward all keys to PTY (except Esc to exit)
        if self.screen == Screen::Terminal && self.insert_mode {
            if key == KeyCode::Esc {
                // Exit insert mode
                self.insert_mode = false;
                return Ok(());
            }
            // Forward everything else to the terminal (including Enter, Ctrl+C, etc.)
            self.handle_terminal_input(key, modifiers);
            return Ok(());
        }

        // Global keys (only when not in insert mode)
        match key {
            KeyCode::Char('q') => {
                // Don't quit if in terminal screen - user might want to type 'q'
                if self.screen != Screen::Terminal {
                    // Check if task or workflow is running
                    if self.monitoring_task.is_some() || self.monitoring_workflow.is_some() {
                        self.popup = Popup::ConfirmQuit;
                    } else {
                        self.should_quit = true;
                    }
                    return Ok(());
                }
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+C: if on terminal screen with PTY, send to PTY
                if self.screen == Screen::Terminal && self.pty_writer.is_some() {
                    self.handle_terminal_input(key, modifiers);
                    return Ok(());
                }
                // Otherwise, confirm quit if running
                if self.monitoring_task.is_some() || self.monitoring_workflow.is_some() {
                    self.popup = Popup::ConfirmQuit;
                } else {
                    self.should_quit = true;
                }
                return Ok(());
            }
            KeyCode::Char('r') => {
                if self.screen != Screen::Terminal {
                    self.last_refresh =
                        Instant::now() - self.refresh_interval - Duration::from_secs(1);
                    return Ok(());
                }
            }
            KeyCode::Char('?') => {
                if self.screen != Screen::Terminal {
                    self.popup = Popup::Help;
                    return Ok(());
                }
            }
            KeyCode::Char('i') => {
                // Enter insert mode on Terminal screen
                if self.screen == Screen::Terminal && self.pty_writer.is_some() {
                    self.insert_mode = true;
                    return Ok(());
                }
            }
            KeyCode::Esc | KeyCode::Backspace => {
                // On terminal screen, Backspace should go to terminal in insert mode (handled above)
                // Esc exits insert mode (handled above) or goes back
                if self.screen != Screen::Workflows {
                    self.go_back();
                    return Ok(());
                }
            }
            KeyCode::Char('v') => {
                // Open terminal view if monitoring a task
                if self.monitoring_task.is_some() {
                    if let Some(task_id) = self.monitoring_task {
                        self.terminal_task = Some(task_id);
                        self.screen = Screen::Terminal;
                        if let Some(task) = self.tasks.iter().find(|t| t.id == task_id) {
                            self.terminal_scroll = task.logs.len().saturating_sub(1);
                        }
                    }
                }
                return Ok(());
            }
            KeyCode::Enter => {
                // On Terminal screen, don't navigate - wait for insert mode
                if self.screen != Screen::Terminal {
                    self.go_forward().await?;
                    return Ok(());
                }
            }
            _ => {}
        }

        // Screen-specific keys
        match self.screen {
            Screen::Workflows => self.handle_workflows_input(key).await?,
            Screen::Tasks => self.handle_tasks_input(key),
            Screen::Actions => self.handle_actions_input(key, modifiers),
            Screen::Terminal => {
                // In normal mode, only navigation keys work
                // 'i' to enter insert mode is handled above
            }
        }

        Ok(())
    }

    pub async fn handle_workflows_input(&mut self, key: KeyCode) -> Result<()> {
        let len = self.runs.len();
        if len == 0 {
            return Ok(());
        }

        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.runs_state.selected().unwrap_or(0);
                self.runs_state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.runs_state.selected().unwrap_or(0);
                self.runs_state.select(Some((i + 1).min(len - 1)));
            }
            KeyCode::Char('c') => {
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
            KeyCode::Home | KeyCode::Char('g') => {
                self.runs_state.select(Some(0));
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.runs_state.select(Some(len.saturating_sub(1)));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn handle_tasks_input(&mut self, key: KeyCode) {
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
            KeyCode::Home | KeyCode::Char('g') => {
                self.tasks_state.select(Some(0));
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.tasks_state.select(Some(len.saturating_sub(1)));
            }
            KeyCode::Char('a') => {
                self.trigger_all();
            }
            _ => {}
        }
    }

    pub fn handle_actions_input(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        // Calculate max scroll for proper clamping (prevent overscroll)
        // Ensure at least one page of content is visible if possible
        // max_scroll = total_lines - page_height. If total < height, max_scroll is 0.
        let page_height = self.log_height.saturating_sub(2) as usize; // Subtract borders
        let max_scroll = self.total_log_lines.saturating_sub(page_height).max(0);

        // Handle Ctrl+key combinations first
        if modifiers.contains(KeyModifiers::CONTROL) {
            match key {
                KeyCode::Char('d') => {
                    // Page down
                    self.log_scroll = (self.log_scroll + page_height).min(max_scroll);
                    return;
                }
                KeyCode::Char('u') => {
                    // Page up
                    self.log_scroll = self.log_scroll.saturating_sub(page_height);
                    return;
                }
                _ => {}
            }
        }

        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.log_scroll = (self.log_scroll + 1).min(max_scroll);
            }
            KeyCode::PageDown => {
                self.log_scroll = (self.log_scroll + page_height).min(max_scroll);
            }
            KeyCode::PageUp => {
                self.log_scroll = self.log_scroll.saturating_sub(page_height);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.log_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.log_scroll = max_scroll;
            }
            KeyCode::Char('t') => {
                self.trigger_current_task();
            }
            KeyCode::Char('a') => {
                self.trigger_all();
            }
            _ => {}
        }
    }

    pub fn handle_terminal_input(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        // Send input directly to PTY if we have a writer
        // This provides true terminal interactivity
        if let Some(ref mut writer) = self.pty_writer {
            // Convert key to terminal escape sequence bytes
            let bytes: Option<Vec<u8>> = match key {
                KeyCode::Char(c) => {
                    if modifiers.contains(KeyModifiers::CONTROL) {
                        // Convert Ctrl+key to control character (e.g., Ctrl+C = 0x03)
                        let ctrl_char = (c.to_ascii_lowercase() as u8)
                            .wrapping_sub(b'a')
                            .wrapping_add(1);
                        if ctrl_char <= 26 {
                            Some(vec![ctrl_char])
                        } else {
                            Some(c.to_string().into_bytes())
                        }
                    } else if modifiers.contains(KeyModifiers::ALT) {
                        // Alt+key sends ESC followed by the character
                        let mut seq = vec![0x1b];
                        seq.extend(c.to_string().bytes());
                        Some(seq)
                    } else {
                        Some(c.to_string().into_bytes())
                    }
                }
                KeyCode::Enter => Some(b"\r".to_vec()), // Carriage return
                KeyCode::Backspace => Some(b"\x7f".to_vec()), // DEL character
                KeyCode::Tab => Some(b"\t".to_vec()),
                KeyCode::Esc => Some(b"\x1b".to_vec()),
                KeyCode::Up => Some(b"\x1b[A".to_vec()),
                KeyCode::Down => Some(b"\x1b[B".to_vec()),
                KeyCode::Right => Some(b"\x1b[C".to_vec()),
                KeyCode::Left => Some(b"\x1b[D".to_vec()),
                KeyCode::Home => Some(b"\x1b[H".to_vec()),
                KeyCode::End => Some(b"\x1b[F".to_vec()),
                KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
                KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
                KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
                KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
                KeyCode::F(n) => {
                    // F1-F4 use different codes than F5-F12
                    let seq = match n {
                        1 => b"\x1bOP".to_vec(),
                        2 => b"\x1bOQ".to_vec(),
                        3 => b"\x1bOR".to_vec(),
                        4 => b"\x1bOS".to_vec(),
                        5 => b"\x1b[15~".to_vec(),
                        6 => b"\x1b[17~".to_vec(),
                        7 => b"\x1b[18~".to_vec(),
                        8 => b"\x1b[19~".to_vec(),
                        9 => b"\x1b[20~".to_vec(),
                        10 => b"\x1b[21~".to_vec(),
                        11 => b"\x1b[23~".to_vec(),
                        12 => b"\x1b[24~".to_vec(),
                        _ => return,
                    };
                    Some(seq)
                }
                _ => None,
            };

            // Write to PTY
            if let Some(bytes) = bytes {
                let _ = writer.write_all(&bytes);
                let _ = writer.flush();
            }
        }

        // If no PTY or unhandled key, navigation is disabled when PTY is active
        // (All keys go to the terminal when it's running)
    }
}
