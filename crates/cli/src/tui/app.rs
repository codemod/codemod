use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use anyhow::Result;
use butterflow_core::engine::Engine;
use butterflow_models::{Task, WorkflowRun};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use uuid::Uuid;

use super::actions::Action;
use super::screens::settings;

/// Which screen is currently displayed
#[derive(Debug, Clone)]
pub enum Screen {
    RunList,
    TaskList { workflow_run_id: Uuid },
    Settings { workflow_run_id: Uuid },
}

/// Application state for the TUI
pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
    pub engine: Engine,

    // Run list state
    pub workflow_runs: Vec<WorkflowRun>,
    pub run_list_state: TableState,

    // Task list state
    pub current_workflow_run: Option<WorkflowRun>,
    pub tasks: Vec<Task>,
    pub task_list_state: TableState,

    // Settings screen state
    pub settings_cursor: usize,

    // Status message
    pub status_message: Option<String>,

    // Run list limit
    pub run_list_limit: usize,

    // Hash of last fetched data — used to skip redraws when nothing changed
    data_hash: u64,
}

impl App {
    /// Create a new App starting at the run list
    pub fn new(engine: Engine, limit: usize) -> Self {
        let mut run_list_state = TableState::default();
        run_list_state.select(Some(0));

        Self {
            screen: Screen::RunList,
            should_quit: false,
            engine,
            workflow_runs: Vec::new(),
            run_list_state,
            current_workflow_run: None,
            tasks: Vec::new(),
            task_list_state: TableState::default(),
            settings_cursor: 0,
            status_message: None,
            run_list_limit: limit,
            data_hash: 0,
        }
    }

    /// Create a new App starting at the task list for a specific run
    pub fn new_for_run(engine: Engine, workflow_run_id: Uuid) -> Self {
        let mut task_list_state = TableState::default();
        task_list_state.select(Some(0));

        Self {
            screen: Screen::TaskList { workflow_run_id },
            should_quit: false,
            engine,
            workflow_runs: Vec::new(),
            run_list_state: TableState::default(),
            current_workflow_run: None,
            tasks: Vec::new(),
            task_list_state,
            settings_cursor: 0,
            status_message: None,
            run_list_limit: 20,
            data_hash: 0,
        }
    }

    /// Refresh data from the engine based on the current screen.
    /// Returns `true` if the data actually changed.
    pub async fn refresh(&mut self) -> Result<bool> {
        match &self.screen {
            Screen::RunList => {
                self.workflow_runs = self
                    .engine
                    .list_workflow_runs(self.run_list_limit)
                    .await
                    .unwrap_or_default();
            }
            Screen::TaskList { workflow_run_id } => {
                let wf_id = *workflow_run_id;
                self.current_workflow_run = self.engine.get_workflow_run(wf_id).await.ok();
                self.tasks = self.engine.get_tasks(wf_id).await.unwrap_or_default();
            }
            Screen::Settings { workflow_run_id } => {
                let wf_id = *workflow_run_id;
                self.current_workflow_run = self.engine.get_workflow_run(wf_id).await.ok();
                self.tasks = self.engine.get_tasks(wf_id).await.unwrap_or_default();
            }
        }

        let new_hash = self.compute_data_hash();
        if new_hash == self.data_hash {
            return Ok(false);
        }
        self.data_hash = new_hash;
        Ok(true)
    }

    fn compute_data_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        // Hash serialized JSON — both Task and WorkflowRun implement Serialize
        if let Ok(json) = serde_json::to_string(&self.workflow_runs) {
            json.hash(&mut hasher);
        }
        if let Ok(json) = serde_json::to_string(&self.tasks) {
            json.hash(&mut hasher);
        }
        if let Ok(json) = serde_json::to_string(&self.current_workflow_run) {
            json.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Handle a key event, returning an optional action
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        // Global quit
        if key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Some(Action::Quit);
        }

        match &self.screen {
            Screen::RunList => self.handle_run_list_key(key),
            Screen::TaskList { workflow_run_id } => {
                let wf_id = *workflow_run_id;
                self.handle_task_list_key(key, wf_id)
            }
            Screen::Settings { workflow_run_id } => {
                let wf_id = *workflow_run_id;
                self.handle_settings_key(key, wf_id)
            }
        }
    }

    fn handle_run_list_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_run_list_cursor(1);
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_run_list_cursor(-1);
                None
            }
            KeyCode::Enter => {
                if let Some(idx) = self.run_list_state.selected() {
                    if let Some(run) = self.workflow_runs.get(idx) {
                        return Some(Action::NavigateToTaskList(run.id));
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn handle_task_list_key(&mut self, key: KeyEvent, workflow_run_id: Uuid) -> Option<Action> {
        let visible_tasks: Vec<&Task> = self.tasks.iter().filter(|t| !t.is_master).collect();

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_task_list_cursor(1);
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_task_list_cursor(-1);
                None
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                if let Some(idx) = self.task_list_state.selected() {
                    if let Some(task) = visible_tasks.get(idx) {
                        return Some(Action::ViewLogs(workflow_run_id, task.id));
                    }
                }
                None
            }
            KeyCode::Char('t') => {
                if let Some(idx) = self.task_list_state.selected() {
                    if let Some(task) = visible_tasks.get(idx) {
                        if task.status == butterflow_models::TaskStatus::AwaitingTrigger {
                            return Some(Action::TriggerTask(workflow_run_id, task.id));
                        }
                    }
                }
                None
            }
            KeyCode::Char('T') => Some(Action::TriggerAll(workflow_run_id)),
            KeyCode::Char('R') => {
                if let Some(idx) = self.task_list_state.selected() {
                    if let Some(task) = visible_tasks.get(idx) {
                        if task.status == butterflow_models::TaskStatus::Failed {
                            return Some(Action::RetryFailed(workflow_run_id, task.id));
                        }
                    }
                }
                None
            }
            KeyCode::Char('s') => Some(Action::NavigateToSettings(workflow_run_id)),
            KeyCode::Char('c') => Some(Action::CancelWorkflow(workflow_run_id)),
            KeyCode::Esc => Some(Action::NavigateToRunList),
            _ => None,
        }
    }

    fn move_run_list_cursor(&mut self, delta: i32) {
        let len = self.workflow_runs.len();
        if len == 0 {
            return;
        }
        let current = self.run_list_state.selected().unwrap_or(0);
        let new = if delta > 0 {
            (current + delta as usize).min(len - 1)
        } else {
            current.saturating_sub((-delta) as usize)
        };
        self.run_list_state.select(Some(new));
    }

    fn move_task_list_cursor(&mut self, delta: i32) {
        let visible_len = self.tasks.iter().filter(|t| !t.is_master).count();
        if visible_len == 0 {
            return;
        }
        let current = self.task_list_state.selected().unwrap_or(0);
        let new = if delta > 0 {
            (current + delta as usize).min(visible_len - 1)
        } else {
            current.saturating_sub((-delta) as usize)
        };
        self.task_list_state.select(Some(new));
    }

    /// Navigate to the task list for a specific run
    pub fn navigate_to_task_list(&mut self, workflow_run_id: Uuid) {
        self.screen = Screen::TaskList { workflow_run_id };
        self.task_list_state = TableState::default();
        self.task_list_state.select(Some(0));
        self.current_workflow_run = None;
        self.tasks.clear();
    }

    /// Navigate back to the run list
    pub fn navigate_to_run_list(&mut self) {
        self.screen = Screen::RunList;
        self.current_workflow_run = None;
        self.tasks.clear();
    }

    /// Navigate to the settings screen for a specific run
    pub fn navigate_to_settings(&mut self, workflow_run_id: Uuid) {
        self.screen = Screen::Settings { workflow_run_id };
        self.settings_cursor = 0;
    }

    /// Navigate back from settings to the task list
    pub fn navigate_back_from_settings(&mut self) {
        if let Screen::Settings { workflow_run_id } = self.screen {
            self.screen = Screen::TaskList { workflow_run_id };
        }
    }

    fn handle_settings_key(&mut self, key: KeyEvent, _workflow_run_id: Uuid) -> Option<Action> {
        let max = settings::settings_count();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if self.settings_cursor + 1 < max {
                    self.settings_cursor += 1;
                }
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_cursor = self.settings_cursor.saturating_sub(1);
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.settings_cursor {
                    0 => {
                        let current = self.engine.is_dry_run();
                        self.engine.set_dry_run(!current);
                    }
                    1 => self.toggle_capability(LlrtSupportedModules::Fs),
                    2 => self.toggle_capability(LlrtSupportedModules::Fetch),
                    3 => self.toggle_capability(LlrtSupportedModules::ChildProcess),
                    _ => {}
                }
                None
            }
            KeyCode::Esc => Some(Action::NavigateBackFromSettings),
            _ => None,
        }
    }

    fn toggle_capability(&mut self, module: LlrtSupportedModules) {
        let caps = self.engine.get_capabilities().clone();
        match caps {
            None => {
                // None = nothing enabled → create a set with just the toggled module
                let mut set = HashSet::new();
                set.insert(module);
                self.engine.set_capabilities(Some(set));
            }
            Some(mut set) => {
                if set.contains(&module) {
                    set.remove(&module);
                } else {
                    set.insert(module);
                }
                self.engine.set_capabilities(Some(set));
            }
        }
    }
}
