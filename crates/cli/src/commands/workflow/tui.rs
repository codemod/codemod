use std::io::{self, Read, Stdout, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use butterflow_core::engine::Engine;
use butterflow_models::{Task, TaskStatus, WorkflowRun, WorkflowStatus};
use clap::Args;

use crate::engine::create_engine;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame, Terminal,
};
use uuid::Uuid;

/// Run workflow resume command in a PTY (pseudo-terminal) for full interactivity
///
/// This spawns the command in a real PTY, allowing:
/// - Programs to detect they're running in a terminal
/// - Full color and formatting support
/// - Interactive prompts and user input
/// - Proper signal handling (Ctrl+C, etc.)
fn run_resume_command_in_terminal(
    app: &mut App,
    workflow_path: &Path,
    run_id: Uuid,
    task_ids: Option<Vec<Uuid>>,
    trigger_all: bool,
    target_path: Option<&Path>,
) -> Result<()> {
    // Get the current executable path
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;

    // Create PTY system
    let pty_system = NativePtySystem::default();

    // Get terminal size from app state
    let (rows, cols) = app.terminal_size;

    // Create PTY pair
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("Failed to open PTY")?;

    // Build command using portable-pty's CommandBuilder
    let mut cmd = CommandBuilder::new(&exe_path);
    cmd.arg("workflow");
    cmd.arg("resume");
    cmd.arg("--workflow");
    cmd.arg(workflow_path.to_string_lossy().as_ref());
    cmd.arg("--id");
    cmd.arg(run_id.to_string());
    // Note: We don't use --allow-dirty or --no-interactive so that prompts are shown

    // Add target path if available
    if let Some(target) = target_path {
        cmd.arg("--target");
        cmd.arg(target.to_string_lossy().as_ref());
    }

    if trigger_all {
        cmd.arg("--trigger-all");
    } else if let Some(ids) = task_ids {
        for task_id in ids {
            cmd.arg("--tasks_ids");
            cmd.arg(task_id.to_string());
        }
    }
    if app.allow_fs {
        cmd.arg("--allow-fs");
    }
    if app.allow_fetch {
        cmd.arg("--allow-fetch");
    }
    if app.allow_child_process {
        cmd.arg("--allow-child-process");
    }

    if app.dry_run {
        cmd.arg("--dry-run");
    }

    // Spawn child process in the PTY
    let _child = pair
        .slave
        .spawn_command(cmd)
        .context("Failed to spawn command in PTY")?;

    // Get writer for sending input to the PTY
    let writer = pair
        .master
        .take_writer()
        .context("Failed to get PTY writer")?;

    // Get reader for reading output from the PTY
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("Failed to get PTY reader")?;

    // Store the writer for input
    app.pty_writer = Some(writer);
    // Store the master for resizing
    app.pty_master = Some(pair.master);

    // Reset the terminal parser for fresh output
    {
        let mut parser = app.terminal_parser.write().unwrap();
        *parser = vt100::Parser::new(rows, cols, 1000); // 1000 lines scrollback
    }

    // Mark PTY as running
    {
        let mut running = app.pty_running.lock().unwrap();
        *running = true;
    }

    // Spawn a background thread to read PTY output and feed it to the parser
    // We use std::thread instead of tokio because portable-pty uses blocking I/O
    let parser_clone = app.terminal_parser.clone();
    let running_clone = app.pty_running.clone();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited
                    let mut running = running_clone.lock().unwrap();
                    *running = false;
                    break;
                }
                Ok(n) => {
                    // Feed the raw bytes to the VT100 parser
                    // This handles all escape sequences, colors, cursor positioning, etc.
                    let mut parser = parser_clone.write().unwrap();
                    parser.process(&buf[..n]);
                }
                Err(e) => {
                    // Error reading - log and exit
                    eprintln!("PTY read error: {}", e);
                    let mut running = running_clone.lock().unwrap();
                    *running = false;
                    break;
                }
            }
        }
    });

    Ok(())
}
#[derive(Args, Debug)]
pub struct Command {
    /// Number of workflow runs to show
    #[arg(short, long, default_value = "25")]
    limit: usize,

    /// Auto-refresh interval in seconds (0 to disable)
    #[arg(long, default_value = "1")]
    refresh_interval: u64,

    /// Dry run mode - don't make actual changes
    #[arg(long)]
    dry_run: bool,

    /// Allow fs access
    #[arg(long)]
    allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    allow_child_process: bool,
}

/// Current screen in the step-by-step flow
#[derive(Debug, Clone, PartialEq, Copy)]
enum Screen {
    /// Step 1: List of workflow runs
    Workflows,
    /// Step 2: Tasks/Nodes for selected workflow
    Tasks,
    /// Step 3: Actions (triggers, logs, details) for selected task
    Actions,
    /// Terminal view for running task execution
    Terminal,
}

/// Trigger action type
#[derive(Debug, Clone)]
enum TriggerAction {
    All,
    Single(Uuid),
}

/// Popup dialog type
#[derive(Debug)]
enum Popup {
    None,
    ConfirmCancel(Uuid),
    ConfirmTrigger(TriggerAction),
    ConfirmQuit,
    StatusMessage(String, Instant),
    Error(String),
    Help,
}

/// Application state
struct App {
    engine: Engine,
    limit: usize,
    refresh_interval: Duration,
    last_refresh: Instant,

    // Current screen
    screen: Screen,

    // Runs list state
    runs: Vec<WorkflowRun>,
    runs_state: TableState,

    // Selected run detail state
    selected_run: Option<WorkflowRun>,
    tasks: Vec<Task>,
    tasks_state: TableState,

    // Selected task for actions screen
    selected_task: Option<Task>,

    // Logs scroll
    log_scroll: usize,

    // Terminal scroll for terminal view
    terminal_scroll: usize,

    // UI state
    popup: Popup,
    should_quit: bool,

    // Track triggered task to monitor its execution
    monitoring_task: Option<Uuid>,

    // Track workflow run being monitored for completion
    monitoring_workflow: Option<Uuid>,

    // Terminal view state - task being shown in terminal
    terminal_task: Option<Uuid>,

    // PTY-based terminal state
    /// VT100 parser for terminal emulation (handles escape sequences, cursor, colors, etc.)
    terminal_parser: Arc<RwLock<vt100::Parser>>,
    /// Writer to send input to the PTY
    pty_writer: Option<Box<dyn Write + Send>>,
    /// Master PTY handle for resizing
    pty_master: Option<Box<dyn MasterPty + Send>>,
    /// Current terminal size (rows, cols)
    terminal_size: (u16, u16),
    /// Total visual lines in logs
    total_log_lines: usize,
    /// Height of logs view
    log_height: u16,
    /// Flag indicating if PTY process is still running
    pty_running: Arc<Mutex<bool>>,
    /// Insert mode flag for terminal - when true, all keystrokes go to PTY
    insert_mode: bool,

    /// Start time for animations
    start_time: Instant,

    // Command-line flags
    /// Dry run mode - don't make actual changes
    dry_run: bool,
    /// Allow fs access
    allow_fs: bool,
    /// Allow fetch access
    allow_fetch: bool,
    /// Allow child process access
    allow_child_process: bool,
}

impl App {
    fn new(
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
    fn should_refresh(&self) -> bool {
        if self.refresh_interval.is_zero() {
            return false;
        }
        self.last_refresh.elapsed() >= self.refresh_interval
    }

    /// Refresh data based on current screen
    async fn refresh(&mut self) -> Result<()> {
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

    async fn refresh_runs(&mut self) -> Result<()> {
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

    async fn refresh_tasks(&mut self) -> Result<()> {
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

    /// Navigate to next screen
    async fn go_forward(&mut self) -> Result<()> {
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
    fn go_back(&mut self) {
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
    fn trigger_all(&mut self) {
        let awaiting = self.get_awaiting_tasks();
        if awaiting.is_empty() {
            self.show_status("No tasks awaiting trigger".to_string());
            return;
        }
        self.popup = Popup::ConfirmTrigger(TriggerAction::All);
    }

    /// Trigger the currently selected task
    fn trigger_current_task(&mut self) {
        if let Some(task) = &self.selected_task {
            if task.status == TaskStatus::AwaitingTrigger {
                self.popup = Popup::ConfirmTrigger(TriggerAction::Single(task.id));
            } else {
                self.show_status("Task is not awaiting trigger".to_string());
            }
        }
    }

    async fn do_trigger_all(&mut self) -> Result<()> {
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
                .unwrap_or_else(|| std::env::current_dir().unwrap());

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

    async fn do_trigger_single(&mut self, task_id: Uuid) -> Result<()> {
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
                .unwrap_or_else(|| std::env::current_dir().unwrap());

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

    async fn cancel_workflow(&mut self, run_id: Uuid) -> Result<()> {
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
    async fn handle_input(&mut self, key: KeyCode, modifiers: KeyModifiers) -> Result<()> {
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

    async fn handle_workflows_input(&mut self, key: KeyCode) -> Result<()> {
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

    fn handle_actions_input(&mut self, key: KeyCode, modifiers: KeyModifiers) {
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

    fn handle_terminal_input(&mut self, key: KeyCode, modifiers: KeyModifiers) {
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

/// Strip ANSI escape codes from a string
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Start of ANSI escape sequence
            // Look for '[' which starts CSI sequence
            if let Some('[') = chars.next() {
                // Skip until we find a letter (the command)
                for next_ch in chars.by_ref() {
                    if next_ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Clean log line by removing timestamps and logging prefixes
fn clean_log_line(log: &str) -> String {
    let mut cleaned = log.trim();

    // Handle carriage returns - only keep text after the last \r
    if let Some(pos) = cleaned.rfind('\r') {
        cleaned = &cleaned[pos + 1..];
    }

    // Remove timestamp patterns like [2025-12-22T21:16:43Z]
    if let Some(pos) = cleaned.find(']') {
        if cleaned[..pos].contains('T') && cleaned[..pos].chars().any(|c| c.is_ascii_digit()) {
            cleaned = &cleaned[pos + 1..];
        }
    }

    // Remove logging level prefixes like [ERROR], [WARN], etc.
    for prefix in &["[ERROR]", "[WARN]", "[INFO]", "[DEBUG]", "[TRACE]"] {
        if cleaned.starts_with(prefix) {
            cleaned = &cleaned[prefix.len()..];
            break;
        }
    }

    // Remove "ERROR" word if it appears at the start
    if cleaned.starts_with("ERROR") {
        cleaned = &cleaned[5..];
    }

    // Remove module paths like butterflow_core::engine::
    if let Some(pos) = cleaned.find("::") {
        if let Some(pos2) = cleaned[pos + 2..].find("::") {
            if let Some(pos3) = cleaned[pos + 2 + pos2 + 2..].find(' ') {
                cleaned = &cleaned[pos + 2 + pos2 + 2 + pos3 + 1..];
            }
        }
    }

    // Remove "Task ... step ... failed:" prefix
    if let Some(pos) = cleaned.find("step ") {
        if let Some(pos2) = cleaned[pos..].find(" failed") {
            if let Some(pos3) = cleaned[pos + pos2 + 7..].find(':') {
                cleaned = &cleaned[pos + pos2 + 7 + pos3 + 1..];
            }
        }
    }

    // Remove "execution failed:" prefix
    if let Some(pos) = cleaned.find("execution failed:") {
        cleaned = &cleaned[pos + 17..];
    }
    // Trim whitespace
    cleaned.trim().to_string()
}

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
/// Render the top navigation bar
/// Render the top navigation bar with a premium look
fn render_breadcrumb(f: &mut Frame, app: &App, area: Rect) {
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
fn render_workflows_screen(f: &mut Frame, app: &mut App, area: Rect, elapsed_ms: u128) {
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
fn render_tasks_screen(f: &mut Frame, app: &mut App, area: Rect, elapsed_ms: u128) {
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
fn render_actions_screen(f: &mut Frame, app: &mut App, area: Rect, elapsed_ms: u128) {
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
/// Render the Terminal screen
fn render_terminal_screen(f: &mut Frame, app: &mut App, area: Rect) {
    // Check if PTY is still running
    let pty_running = {
        let running = app.pty_running.lock().unwrap();
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

    if app.terminal_size != (new_rows, new_cols) && new_rows > 0 && new_cols > 0 {
        app.terminal_size = (new_rows, new_cols);
        // Resize the parser to match
        let mut parser = app.terminal_parser.write().unwrap();
        parser.set_size(new_rows, new_cols);

        // Resize the PTY
        if let Some(master) = &mut app.pty_master {
            let _ = master.resize(PtySize {
                rows: new_rows,
                cols: new_cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    // Get content from the vt100 parser
    let parser = app.terminal_parser.read().unwrap();
    let screen = parser.screen();

    // Build terminal content from the screen
    let mut terminal_content: Vec<Line> = Vec::new();

    for row in 0..screen.size().0 {
        let mut spans: Vec<Span> = Vec::new();

        for col in 0..screen.size().1 {
            let cell = screen.cell(row, col).unwrap();
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
fn render_footer(f: &mut Frame, app: &App, area: Rect) {
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

/// Render popup dialogs
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

/// Main UI render function
fn ui(f: &mut Frame, app: &mut App) {
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

/// Run the TUI event loop
async fn run_tui(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        // Refresh data if needed
        if app.should_refresh() {
            app.refresh().await?;
        }

        // Auto-dismiss status messages after 2 seconds (unless monitoring)
        if let Popup::StatusMessage(_, instant) = &app.popup {
            if instant.elapsed() > Duration::from_secs(2) && app.monitoring_task.is_none() {
                app.popup = Popup::None;
            }
        }

        // If monitoring a task or workflow, refresh more frequently to see logs and status
        if (app.monitoring_task.is_some() || app.monitoring_workflow.is_some())
            && app.last_refresh.elapsed() >= Duration::from_millis(200)
        {
            app.refresh().await?;
        }

        terminal.hide_cursor().context("Failed to hide cursor")?;

        // Render
        terminal.draw(|f| ui(f, app))?;

        // Handle events with timeout for periodic refresh
        let timeout = if app.refresh_interval.is_zero() {
            Duration::from_millis(33) // ~30 FPS for smooth animations
        } else {
            Duration::from_millis(33).min(app.refresh_interval)
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
    // Create a minimal engine first - we'll recreate it with proper workflow path when needed
    // For now, use current directory as placeholder
    let workflow_file_path = std::env::current_dir()?;
    let target_path = std::env::current_dir()?;

    // Create engine using create_engine like resume.rs
    let (_, config) = create_engine(
        workflow_file_path,
        target_path,
        false, // dry_run
        false, // allow_dirty - respect git checks
        std::collections::HashMap::new(),
        None,
        None,  // capabilities - will be resolved from workflow run when needed
        false, // no_interactive - TUI is interactive
    )?;

    let engine = Engine::with_workflow_run_config(config);

    let refresh_interval = if args.refresh_interval == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(args.refresh_interval)
    };

    let mut app = App::new(
        engine,
        args.limit,
        refresh_interval,
        args.dry_run,
        args.allow_fs,
        args.allow_fetch,
        args.allow_child_process,
    );

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
