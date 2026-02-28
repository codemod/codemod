use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use butterflow_core::engine::Engine;
use butterflow_models::{Task, TaskStatus, WorkflowRun, WorkflowStatus};
use crossterm::event::{KeyCode, KeyModifiers};
use portable_pty::MasterPty;
use ratatui::widgets::TableState;
use uuid::Uuid;

use super::pty::spawn_task_in_slot;
use super::terminal_slots::{
    TerminalEntry, TerminalQueueState, TriggerAllContext, MAX_CONCURRENT_PTYS,
};
use super::types::{Popup, Screen, TerminalMode, TriggerAction};

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

    /// Terminal mode: Single (one full screen) or Multi (list of terminals)
    pub terminal_mode: TerminalMode,

    /// Selected index in terminal list (which terminal to view)
    pub selected_terminal_index: usize,

    /// List state for terminal list (left panel in Multi mode)
    pub terminal_list_state: TableState,

    /// Terminal list for Trigger All: one entry per task, no fixed min/max
    pub terminal_entries: Vec<TerminalEntry>,

    /// Task queue state for Trigger All mode
    pub terminal_queue_state: TerminalQueueState,

    /// Last slot dimensions (rows, cols) for spawning - set during render
    pub last_slot_size: (u16, u16),

    // PTY-based terminal state (used in Single mode)
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
            terminal_mode: TerminalMode::Single,
            selected_terminal_index: 0,
            terminal_list_state: TableState::default(),
            terminal_entries: Vec::new(),
            terminal_queue_state: TerminalQueueState::default(),
            last_slot_size: (12, 40),
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

                // In Multi mode: clear completed terminals and start next from queue
                if self.terminal_mode == TerminalMode::Multi
                    && self.terminal_queue_state.context.is_some()
                {
                    let (slot_rows, slot_cols) = self.last_slot_size;
                    // Start next from queue when a slot finishes; keep completed slots for display (Done status)
                    let completed_indices: Vec<usize> = self
                        .terminal_entries
                        .iter()
                        .enumerate()
                        .filter_map(|(i, e)| {
                            e.slot
                                .as_ref()
                                .filter(|s| !s.is_running())
                                .map(|_| i)
                        })
                        .collect();
                    for _idx in completed_indices {
                        // Don't clear slot - keep it so status shows "Done" and user can view output
                        if let Err(e) = self.try_start_next_from_queue(slot_rows, slot_cols) {
                            self.popup = Popup::Error(format!("Failed to start next task: {}", e));
                        }
                    }

                    // Check if all done (no active terminals, queue empty)
                    let any_active = self.terminal_entries.iter().any(|e| {
                        e.slot
                            .as_ref()
                            .map(|s| s.is_running())
                            .unwrap_or(false)
                    });
                    let queue_empty = self.terminal_queue_state.pending_queue.is_empty();
                    if !any_active && queue_empty && self.terminal_queue_state.context.is_some() {
                        let any_failed = self.terminal_entries.iter().any(|e| {
                            e.slot
                                .as_ref()
                                .map(|s| matches!(s.exit_code(), Some(c) if c != 0))
                                .unwrap_or(false)
                        });
                        self.terminal_queue_state.clear();
                        self.monitoring_workflow = None;
                        if any_failed {
                            self.show_status("❌ Some tasks failed".to_string());
                        } else {
                            self.show_status("✅ All tasks completed".to_string());
                        }
                    }
                }
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

                // When TUI spawns tasks with --exit-on-task-complete, each process exits early.
                // Ensure workflow status is updated when all tasks are done.
                let _ = self
                    .engine
                    .update_workflow_status_if_all_tasks_done(run_id)
                    .await;

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

    /// Get workflow file path from a run
    fn get_workflow_path_from_run(run: &WorkflowRun) -> PathBuf {
        run.bundle_path
            .as_ref()
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
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")))
    }

    /// Start the next task from queue: find its entry and spawn PTY there (Trigger All mode)
    fn try_start_next_from_queue(&mut self, slot_rows: u16, slot_cols: u16) -> Result<()> {
        let Some(ctx) = self.terminal_queue_state.context.clone() else {
            return Ok(());
        };
        let Some(next_task_id) = self.terminal_queue_state.pop_next() else {
            return Ok(());
        };
        let Some((entry_idx, _)) = self
            .terminal_entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.task_id == next_task_id)
        else {
            return Ok(());
        };
        spawn_task_in_slot(
            self,
            entry_idx,
            next_task_id,
            &ctx.workflow_path,
            ctx.run_id,
            ctx.target_path.as_deref(),
            slot_rows,
            slot_cols,
        )
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
                // Can navigate to terminal list if we have terminals
                if !self.terminal_entries.is_empty() {
                    self.terminal_mode = TerminalMode::Multi;
                    self.selected_terminal_index = self
                        .selected_terminal_index
                        .min(self.terminal_entries.len().saturating_sub(1));
                    self.terminal_list_state
                        .select(Some(self.selected_terminal_index));
                    self.screen = Screen::Terminal;
                } else if self.monitoring_task.is_some() {
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
                self.terminal_queue_state.clear();
                // Keep terminal_entries so user can press v on Workflows to view terminals
            }
            Screen::Actions => {
                self.screen = Screen::Tasks;
                self.selected_task = None;
            }
            Screen::Terminal => {
                self.screen = Screen::Tasks;
                self.terminal_task = None;
                self.terminal_scroll = 0;
                self.selected_task = None;
                // Keep terminal_entries so user can return with 'v' to watch the list
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
            let awaiting: Vec<Uuid> = self.get_awaiting_tasks().iter().map(|t| t.id).collect();
            if awaiting.is_empty() {
                self.show_status("No tasks awaiting trigger".to_string());
                return Ok(());
            }

            let workflow_path = Self::get_workflow_path_from_run(run);
            let run_id = run.id;
            let target_path = run.target_path.clone();

            // One list entry per task; no fixed min/max
            self.terminal_entries = awaiting
                .iter()
                .map(|&task_id| TerminalEntry {
                    task_id,
                    slot: None,
                })
                .collect();
            self.terminal_queue_state = TerminalQueueState {
                pending_queue: awaiting,
                context: Some(TriggerAllContext {
                    workflow_path: workflow_path.clone(),
                    run_id,
                    target_path: target_path.clone(),
                }),
                started_at: Some(Instant::now()),
            };
            self.terminal_mode = TerminalMode::Multi;
            self.selected_terminal_index = 0;
            self.terminal_list_state = TableState::default();
            self.terminal_list_state.select(Some(0));
            self.screen = Screen::Terminal;
            self.terminal_task = None;
            self.terminal_scroll = 0;
            self.monitoring_workflow = Some(run_id);

            // Default size for initial spawn (will be resized on first render)
            let (slot_rows, slot_cols) = (12, 40);

            // Start up to MAX_CONCURRENT_PTYS tasks initially
            for _ in 0..MAX_CONCURRENT_PTYS {
                if self.terminal_queue_state.pending_queue.is_empty() {
                    break;
                }
                if let Err(e) = self.try_start_next_from_queue(slot_rows, slot_cols) {
                    self.popup = Popup::Error(format!("Failed to start task: {}", e));
                    break;
                }
            }
            self.last_refresh = Instant::now() - self.refresh_interval;
        } else {
            self.show_status("No workflow run selected".to_string());
        }
        Ok(())
    }

    pub async fn do_trigger_single(&mut self, task_id: Uuid) -> Result<()> {
        if let Some(run) = &self.selected_run {
            let workflow_path = Self::get_workflow_path_from_run(run);
            let run_id = run.id;
            let target_path = run.target_path.clone();

            // Add to terminal list if not already there (unified list for Trigger All + single)
            let entry_idx = if let Some((idx, e)) = self
                .terminal_entries
                .iter()
                .enumerate()
                .find(|(_, e)| e.task_id == task_id)
            {
                if e.slot.as_ref().map(|s| s.is_running()).unwrap_or(false) {
                    self.show_status("Task is already running".to_string());
                    return Ok(());
                }
                idx
            } else {
                self.terminal_entries.push(TerminalEntry {
                    task_id,
                    slot: None,
                });
                self.terminal_entries.len() - 1
            };

            // Set context for this run (needed for spawn)
            if self.terminal_queue_state.context.is_none() {
                self.terminal_queue_state.context = Some(TriggerAllContext {
                    workflow_path: workflow_path.clone(),
                    run_id,
                    target_path: target_path.clone(),
                });
            }

            self.terminal_mode = TerminalMode::Multi;
            self.selected_terminal_index = entry_idx;
            self.terminal_list_state.select(Some(entry_idx));
            self.screen = Screen::Terminal;
            self.terminal_task = None;
            self.terminal_scroll = 0;
            self.monitoring_workflow = Some(run_id);

            let (slot_rows, slot_cols) = (12, 40);
            match spawn_task_in_slot(
                self,
                entry_idx,
                task_id,
                &workflow_path,
                run_id,
                target_path.as_deref(),
                slot_rows,
                slot_cols,
            ) {
                Ok(()) => {
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
                // Enter insert mode on Terminal screen (Single or Multi with active slot)
                let can_insert = match self.terminal_mode {
                    TerminalMode::Single => self.pty_writer.is_some(),
                    TerminalMode::Multi => self
                        .terminal_entries
                        .get(self.selected_terminal_index)
                        .and_then(|e| e.slot.as_ref())
                        .and_then(|s| s.writer.as_ref())
                        .is_some(),
                };
                if self.screen == Screen::Terminal && can_insert {
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
                // View terminal list (from Trigger All or single triggers)
                if !self.terminal_entries.is_empty() {
                    self.terminal_mode = TerminalMode::Multi;
                    self.selected_terminal_index = self
                        .selected_terminal_index
                        .min(self.terminal_entries.len().saturating_sub(1));
                    self.terminal_list_state
                        .select(Some(self.selected_terminal_index));
                    self.screen = Screen::Terminal;
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
                // Slot navigation (1-4, Tab, arrows) in Multi mode
                self.handle_terminal_slot_navigation(key, modifiers);
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
        let bytes: Option<Vec<u8>> = match key {
            KeyCode::Char(c) => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    let ctrl_char = (c.to_ascii_lowercase() as u8)
                        .wrapping_sub(b'a')
                        .wrapping_add(1);
                    if ctrl_char <= 26 {
                        Some(vec![ctrl_char])
                    } else {
                        Some(c.to_string().into_bytes())
                    }
                } else if modifiers.contains(KeyModifiers::ALT) {
                    let mut seq = vec![0x1b];
                    seq.extend(c.to_string().bytes());
                    Some(seq)
                } else {
                    Some(c.to_string().into_bytes())
                }
            }
            KeyCode::Enter => Some(b"\r".to_vec()),
            KeyCode::Backspace => Some(b"\x7f".to_vec()),
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

        if let Some(bytes) = bytes {
            match self.terminal_mode {
                TerminalMode::Single => {
                    if let Some(ref mut writer) = self.pty_writer {
                        let _ = writer.write_all(&bytes);
                        let _ = writer.flush();
                    }
                }
                TerminalMode::Multi => {
                    if let Some(entry) = self.terminal_entries.get_mut(self.selected_terminal_index)
                    {
                        if let Some(ref mut writer) =
                            entry.slot.as_mut().and_then(|s| s.writer.as_mut())
                        {
                            let _ = writer.write_all(&bytes);
                            let _ = writer.flush();
                        }
                    }
                }
            }
        }
    }

    /// Handle terminal list navigation in Multi mode
    fn handle_terminal_slot_navigation(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        if self.terminal_mode != TerminalMode::Multi {
            return;
        }
        let len = self.terminal_entries.len();
        if len == 0 {
            return;
        }
        let max_idx = len.saturating_sub(1);
        match key {
            KeyCode::Char('1') => self.selected_terminal_index = 0.min(max_idx),
            KeyCode::Char('2') => self.selected_terminal_index = 1.min(max_idx),
            KeyCode::Char('3') => self.selected_terminal_index = 2.min(max_idx),
            KeyCode::Char('4') => self.selected_terminal_index = 3.min(max_idx),
            KeyCode::Tab => {
                if modifiers.contains(KeyModifiers::SHIFT) {
                    self.selected_terminal_index =
                        self.selected_terminal_index.saturating_sub(1);
                } else {
                    self.selected_terminal_index =
                        (self.selected_terminal_index + 1).min(max_idx);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_terminal_index = self.selected_terminal_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected_terminal_index =
                    (self.selected_terminal_index + 1).min(max_idx);
            }
            _ => {}
        }
        self.terminal_list_state
            .select(Some(self.selected_terminal_index));
    }
}
