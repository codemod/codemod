use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::SyncSender;

use anyhow::Result;
use butterflow_core::ai_handoff::AgentOption;
use butterflow_core::config::ShellCommandExecutionRequest;
use butterflow_models::{Task, TaskStatus, Workflow, WorkflowRun, WorkflowStatus};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::widgets::TableState;
use uuid::Uuid;

use super::event::AppEvent;
use super::screens::settings;
use super::screens::StatusLine;
use super::task_visibility::task_visible_in_list;

pub const USE_BUILT_IN_AGENT: &str = "__use_built_in__";

#[derive(Debug, Clone)]
pub enum Screen {
    RunList,
    TaskList { workflow_run_id: Uuid },
    Settings { workflow_run_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOverrides {
    pub dry_run: bool,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
}

impl SessionOverrides {
    pub fn new(dry_run: bool, capabilities: Option<HashSet<LlrtSupportedModules>>) -> Self {
        Self {
            dry_run,
            capabilities,
        }
    }

    pub fn toggle_dry_run(&mut self) {
        self.dry_run = !self.dry_run;
    }

    pub fn toggle_capability(&mut self, module: LlrtSupportedModules) {
        match &mut self.capabilities {
            Some(set) => {
                if set.contains(&module) {
                    set.remove(&module);
                } else {
                    set.insert(module);
                }
            }
            None => {
                let mut set = HashSet::new();
                set.insert(module);
                self.capabilities = Some(set);
            }
        }
    }

    pub fn seed_from_workflow_run(&mut self, workflow_run: &WorkflowRun) {
        if self.capabilities.is_none() {
            self.capabilities = workflow_run.capabilities.clone();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogView {
    pub task_id: Uuid,
    pub node_id: String,
    pub status: TaskStatus,
    pub lines: Vec<String>,
    pub error: Option<String>,
}

impl LogView {
    pub fn from_task(task: &Task) -> Self {
        Self {
            task_id: task.id,
            node_id: task.node_id.clone(),
            status: task.status,
            lines: task.logs.clone(),
            error: task.error.clone(),
        }
    }
}

#[derive(Debug)]
pub struct PendingShellApproval {
    pub request: ShellCommandExecutionRequest,
    response_tx: SyncSender<Result<bool>>,
}

impl PendingShellApproval {
    pub fn new(
        request: ShellCommandExecutionRequest,
        response_tx: SyncSender<Result<bool>>,
    ) -> Self {
        Self {
            request,
            response_tx,
        }
    }

    pub fn respond(self, approved: bool) {
        if let Err(e) = self.response_tx.send(Ok(approved)) {
            log::warn!("Failed to send shell approval response: {}", e);
        }
    }

    pub fn fail(self, error: anyhow::Error) {
        if let Err(e) = self.response_tx.send(Err(error)) {
            log::warn!("Failed to send shell approval error: {}", e);
        }
    }
}

#[derive(Debug)]
pub struct PendingCapabilityApproval {
    pub modules: Vec<LlrtSupportedModules>,
    response_tx: SyncSender<Result<()>>,
}

impl PendingCapabilityApproval {
    pub fn new(modules: Vec<LlrtSupportedModules>, response_tx: SyncSender<Result<()>>) -> Self {
        Self {
            modules,
            response_tx,
        }
    }

    pub fn respond(self, approved: bool) {
        let result = if approved {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Aborting due to capabilities warning"))
        };
        if let Err(e) = self.response_tx.send(result) {
            log::warn!("Failed to send capability approval response: {}", e);
        }
    }

    pub fn fail(self, error: anyhow::Error) {
        if let Err(e) = self.response_tx.send(Err(error)) {
            log::warn!("Failed to send capability approval error: {}", e);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSelectionItem {
    pub canonical: String,
    pub label: String,
    pub is_available: bool,
}

impl AgentSelectionItem {
    pub fn from_agent_option(agent: &AgentOption) -> Self {
        Self {
            canonical: agent.canonical.to_string(),
            label: agent.label.to_string(),
            is_available: agent.is_available(),
        }
    }
}

#[derive(Debug)]
pub struct PendingAgentSelection {
    pub options: Vec<AgentSelectionItem>,
    response_tx: SyncSender<Result<Option<String>>>,
}

impl PendingAgentSelection {
    pub fn new(
        options: Vec<AgentSelectionItem>,
        response_tx: SyncSender<Result<Option<String>>>,
    ) -> Self {
        Self {
            options,
            response_tx,
        }
    }

    pub fn respond(self, selection: Option<String>) {
        if let Err(e) = self.response_tx.send(Ok(selection)) {
            log::warn!("Failed to send agent selection response: {}", e);
        }
    }

    pub fn fail(self, error: anyhow::Error) {
        if let Err(e) = self.response_tx.send(Err(error)) {
            log::warn!("Failed to send agent selection error: {}", e);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEffect {
    Refresh,
    LoadLogs {
        workflow_run_id: Uuid,
        task_id: Uuid,
    },
    TriggerTask {
        workflow_run_id: Uuid,
        task_id: Uuid,
    },
    TriggerAll {
        workflow_run_id: Uuid,
    },
    RetryTask {
        workflow_run_id: Uuid,
        task_id: Uuid,
    },
    CancelWorkflow {
        workflow_run_id: Uuid,
    },
}

impl AppEffect {
    pub fn should_refresh_after(self) -> bool {
        matches!(
            self,
            AppEffect::TriggerTask { .. }
                | AppEffect::TriggerAll { .. }
                | AppEffect::RetryTask { .. }
                | AppEffect::CancelWorkflow { .. }
        )
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum EffectResult {
    Refreshed {
        workflow_runs: Vec<WorkflowRun>,
        current_workflow_run: Option<WorkflowRun>,
        tasks: Vec<Task>,
    },
    LogsLoaded(Option<LogView>),
    Status(StatusLine),
    Noop,
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
    pub workflow_runs: Vec<WorkflowRun>,
    pub run_list_state: TableState,
    pub current_workflow_run: Option<WorkflowRun>,
    pub tasks: Vec<Task>,
    pub task_list_state: TableState,
    selected_task_id: Option<Uuid>,
    pub settings_cursor: usize,
    pub run_list_limit: usize,
    pub status_line: Option<StatusLine>,
    pub log_view: Option<LogView>,
    pub log_scroll: u16,
    pub log_follow: bool,
    shell_approval: Option<PendingShellApproval>,
    capability_approval: Option<PendingCapabilityApproval>,
    agent_selection: Option<PendingAgentSelection>,
    pub agent_selection_cursor: usize,
    pub session_overrides: SessionOverrides,
    base_overrides: SessionOverrides,
    overrides_seeded_from_run: bool,
    data_hash: u64,
}

impl App {
    pub fn new(
        dry_run: bool,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
        limit: usize,
    ) -> Self {
        let mut run_list_state = TableState::default();
        run_list_state.select(Some(0));
        let base_overrides = SessionOverrides::new(dry_run, capabilities);

        Self {
            screen: Screen::RunList,
            should_quit: false,
            workflow_runs: Vec::new(),
            run_list_state,
            current_workflow_run: None,
            tasks: Vec::new(),
            task_list_state: TableState::default(),
            selected_task_id: None,
            settings_cursor: 0,
            run_list_limit: limit,
            status_line: None,
            log_view: None,
            log_scroll: 0,
            log_follow: true,
            shell_approval: None,
            capability_approval: None,
            agent_selection: None,
            agent_selection_cursor: 0,
            session_overrides: base_overrides.clone(),
            base_overrides,
            overrides_seeded_from_run: true,
            data_hash: 0,
        }
    }

    pub fn new_for_run(
        dry_run: bool,
        capabilities: Option<HashSet<LlrtSupportedModules>>,
        workflow_run_id: Uuid,
    ) -> Self {
        let mut task_list_state = TableState::default();
        task_list_state.select(Some(0));
        let base_overrides = SessionOverrides::new(dry_run, capabilities);

        Self {
            screen: Screen::TaskList { workflow_run_id },
            should_quit: false,
            workflow_runs: Vec::new(),
            run_list_state: TableState::default(),
            current_workflow_run: None,
            tasks: Vec::new(),
            task_list_state,
            selected_task_id: None,
            settings_cursor: 0,
            run_list_limit: 20,
            status_line: None,
            log_view: None,
            log_scroll: 0,
            log_follow: true,
            shell_approval: None,
            capability_approval: None,
            agent_selection: None,
            agent_selection_cursor: 0,
            session_overrides: base_overrides.clone(),
            base_overrides,
            overrides_seeded_from_run: false,
            data_hash: 0,
        }
    }

    pub fn initial_effects(&self) -> Vec<AppEffect> {
        vec![AppEffect::Refresh]
    }

    pub fn reduce(&mut self, event: AppEvent) -> Vec<AppEffect> {
        match event {
            AppEvent::Tick => vec![AppEffect::Refresh],
            AppEvent::Resize(_, _) => Vec::new(),
            AppEvent::Scroll(delta) => {
                self.handle_scroll(delta);
                Vec::new()
            }
            AppEvent::Mouse(mouse) => self.handle_mouse(mouse),
            AppEvent::Key(key) => self.handle_key(key),
        }
    }

    pub fn apply_effect_result(&mut self, result: EffectResult) -> bool {
        match result {
            EffectResult::Refreshed {
                workflow_runs,
                current_workflow_run,
                mut tasks,
            } => {
                self.workflow_runs = workflow_runs;
                self.current_workflow_run = current_workflow_run;
                sort_tasks(
                    &mut tasks,
                    self.current_workflow_run.as_ref().map(|r| &r.workflow),
                );
                self.tasks = tasks;
                self.sync_task_selection();

                if !self.overrides_seeded_from_run {
                    if let Some(run) = &self.current_workflow_run {
                        self.session_overrides.seed_from_workflow_run(run);
                    }
                    self.overrides_seeded_from_run = true;
                }

                self.sync_log_view();

                let new_hash = self.compute_data_hash();
                if new_hash == self.data_hash {
                    return self.has_live_updates();
                }
                self.data_hash = new_hash;
                true
            }
            EffectResult::LogsLoaded(log_view) => {
                let changed = self.log_view != log_view;
                self.log_view = log_view;
                changed
            }
            EffectResult::Status(status_line) => {
                let changed = self.status_line.as_ref() != Some(&status_line);
                self.status_line = Some(status_line);
                changed
            }
            EffectResult::Noop => false,
        }
    }

    pub fn current_workflow_run_id(&self) -> Option<Uuid> {
        match self.screen {
            Screen::RunList => None,
            Screen::TaskList { workflow_run_id } | Screen::Settings { workflow_run_id } => {
                Some(workflow_run_id)
            }
        }
    }

    fn compute_data_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        hash_workflow_runs(&mut hasher, &self.workflow_runs);
        hash_optional_workflow_run(&mut hasher, self.current_workflow_run.as_ref());
        hash_tasks(&mut hasher, &self.tasks);
        hash_status_line(&mut hasher, self.status_line.as_ref());
        hash_log_view(&mut hasher, self.log_view.as_ref());
        hasher.finish()
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<AppEffect> {
        if self.shell_approval.is_some() {
            return self.handle_shell_approval_key(key);
        }
        if self.capability_approval.is_some() {
            return self.handle_capability_approval_key(key);
        }
        if self.agent_selection.is_some() {
            return self.handle_agent_selection_key(key);
        }

        if key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.should_quit = true;
            return Vec::new();
        }

        if self.log_view.is_some() {
            return self.handle_log_view_key(key);
        }

        match self.screen.clone() {
            Screen::RunList => self.handle_run_list_key(key),
            Screen::TaskList { workflow_run_id } => self.handle_task_list_key(key, workflow_run_id),
            Screen::Settings { workflow_run_id } => self.handle_settings_key(key, workflow_run_id),
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<AppEffect> {
        if self.shell_approval.is_some()
            || self.capability_approval.is_some()
            || self.agent_selection.is_some()
        {
            return Vec::new();
        }

        if self.log_view.is_some() {
            return match mouse.kind {
                MouseEventKind::ScrollDown => {
                    self.scroll_log_view(3);
                    Vec::new()
                }
                MouseEventKind::ScrollUp => {
                    self.scroll_log_view(-3);
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => match self.screen.clone() {
                Screen::RunList => {
                    self.move_run_list_cursor(1);
                    Vec::new()
                }
                Screen::TaskList { .. } => {
                    self.move_task_list_cursor(1);
                    Vec::new()
                }
                Screen::Settings { .. } => {
                    self.move_settings_cursor(1);
                    Vec::new()
                }
            },
            MouseEventKind::ScrollUp => match self.screen.clone() {
                Screen::RunList => {
                    self.move_run_list_cursor(-1);
                    Vec::new()
                }
                Screen::TaskList { .. } => {
                    self.move_task_list_cursor(-1);
                    Vec::new()
                }
                Screen::Settings { .. } => {
                    self.move_settings_cursor(-1);
                    Vec::new()
                }
            },
            _ => Vec::new(),
        }
    }

    fn handle_scroll(&mut self, delta: i32) {
        if self.shell_approval.is_some()
            || self.capability_approval.is_some()
            || self.agent_selection.is_some()
        {
            return;
        }

        if self.log_view.is_some() {
            self.scroll_log_view(delta);
            return;
        }

        match self.screen.clone() {
            Screen::RunList => self.move_run_list_cursor(delta),
            Screen::TaskList { .. } => self.move_task_list_cursor(delta),
            Screen::Settings { .. } => self.move_settings_cursor(delta),
        }
    }

    fn handle_log_view_key(&mut self, key: KeyEvent) -> Vec<AppEffect> {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('l') => {
                self.log_view = None;
                self.log_scroll = 0;
                self.log_follow = true;
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_log_view(1);
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_log_view(-1);
                Vec::new()
            }
            KeyCode::PageDown => {
                self.scroll_log_view(10);
                Vec::new()
            }
            KeyCode::PageUp => {
                self.scroll_log_view(-10);
                Vec::new()
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.log_scroll = 0;
                self.log_follow = false;
                Vec::new()
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.log_scroll = 0;
                self.log_follow = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_shell_approval_key(&mut self, key: KeyEvent) -> Vec<AppEffect> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.resolve_shell_approval(true, false);
                Vec::new()
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.resolve_shell_approval(false, false);
                Vec::new()
            }
            KeyCode::Char('q') | KeyCode::Char('c')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.resolve_shell_approval(false, true);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_capability_approval_key(&mut self, key: KeyEvent) -> Vec<AppEffect> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.resolve_capability_approval(true, false);
                Vec::new()
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.resolve_capability_approval(false, false);
                Vec::new()
            }
            KeyCode::Char('q') | KeyCode::Char('c')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.resolve_capability_approval(false, true);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_agent_selection_key(&mut self, key: KeyEvent) -> Vec<AppEffect> {
        let option_count = self
            .agent_selection
            .as_ref()
            .map(|selection| selection.options.len())
            .unwrap_or(0);

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if option_count > 0 {
                    self.agent_selection_cursor =
                        (self.agent_selection_cursor + 1).min(option_count - 1);
                }
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.agent_selection_cursor = self.agent_selection_cursor.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Enter => {
                let selected = self
                    .agent_selection
                    .as_ref()
                    .and_then(|selection| selection.options.get(self.agent_selection_cursor))
                    .map(|item| {
                        if item.canonical == USE_BUILT_IN_AGENT {
                            None
                        } else {
                            Some(item.canonical.clone())
                        }
                    })
                    .unwrap_or(None);
                self.resolve_agent_selection(selected, false);
                Vec::new()
            }
            KeyCode::Esc => {
                self.resolve_agent_selection(None, false);
                Vec::new()
            }
            KeyCode::Char('q') | KeyCode::Char('c')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.resolve_agent_selection(None, true);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_run_list_key(&mut self, key: KeyEvent) -> Vec<AppEffect> {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_run_list_cursor(1);
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_run_list_cursor(-1);
                Vec::new()
            }
            KeyCode::Enter => {
                if let Some(idx) = self.run_list_state.selected() {
                    if let Some(run) = self.workflow_runs.get(idx) {
                        self.navigate_to_task_list(run.id);
                        return vec![AppEffect::Refresh];
                    }
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_task_list_key(&mut self, key: KeyEvent, workflow_run_id: Uuid) -> Vec<AppEffect> {
        let visible_tasks = self.visible_tasks();

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_task_list_cursor(1);
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_task_list_cursor(-1);
                Vec::new()
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                if let Some(idx) = self.task_list_state.selected() {
                    if let Some(task) = visible_tasks.get(idx) {
                        return vec![AppEffect::LoadLogs {
                            workflow_run_id,
                            task_id: task.id,
                        }];
                    }
                }
                Vec::new()
            }
            KeyCode::Char('t') => {
                if let Some(idx) = self.task_list_state.selected() {
                    if let Some(task) = visible_tasks.get(idx) {
                        if task.status == TaskStatus::AwaitingTrigger {
                            return vec![AppEffect::TriggerTask {
                                workflow_run_id,
                                task_id: task.id,
                            }];
                        }
                    }
                }
                Vec::new()
            }
            KeyCode::Char('T') => vec![AppEffect::TriggerAll { workflow_run_id }],
            KeyCode::Char('R') => {
                if let Some(idx) = self.task_list_state.selected() {
                    if let Some(task) = visible_tasks.get(idx) {
                        if task.status == TaskStatus::Failed {
                            return vec![AppEffect::RetryTask {
                                workflow_run_id,
                                task_id: task.id,
                            }];
                        }
                    }
                }
                Vec::new()
            }
            KeyCode::Char('s') => {
                self.screen = Screen::Settings { workflow_run_id };
                self.settings_cursor = 0;
                Vec::new()
            }
            KeyCode::Char('c') => {
                let is_cancelable = self.current_workflow_run.as_ref().is_some_and(|run| {
                    run.status == WorkflowStatus::Running
                        || run.status == WorkflowStatus::AwaitingTrigger
                });
                if is_cancelable {
                    vec![AppEffect::CancelWorkflow { workflow_run_id }]
                } else {
                    Vec::new()
                }
            }
            KeyCode::Esc => {
                self.navigate_to_run_list();
                vec![AppEffect::Refresh]
            }
            _ => Vec::new(),
        }
    }

    fn handle_settings_key(&mut self, key: KeyEvent, workflow_run_id: Uuid) -> Vec<AppEffect> {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_settings_cursor(1);
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_settings_cursor(-1);
                Vec::new()
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.settings_cursor {
                    0 => self.session_overrides.toggle_dry_run(),
                    1 => self
                        .session_overrides
                        .toggle_capability(LlrtSupportedModules::Fs),
                    2 => self
                        .session_overrides
                        .toggle_capability(LlrtSupportedModules::Fetch),
                    3 => self
                        .session_overrides
                        .toggle_capability(LlrtSupportedModules::ChildProcess),
                    _ => {}
                }
                self.status_line = None;
                Vec::new()
            }
            KeyCode::Esc => {
                self.screen = Screen::TaskList { workflow_run_id };
                Vec::new()
            }
            _ => Vec::new(),
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

    fn move_settings_cursor(&mut self, delta: i32) {
        let len = settings::settings_count();
        if len == 0 {
            self.settings_cursor = 0;
            return;
        }

        if delta > 0 {
            self.settings_cursor = (self.settings_cursor + delta as usize).min(len - 1);
        } else {
            self.settings_cursor = self.settings_cursor.saturating_sub((-delta) as usize);
        }
    }

    fn move_task_list_cursor(&mut self, delta: i32) {
        let visible_task_ids: Vec<Uuid> = self
            .tasks
            .iter()
            .filter(|task| task_visible_in_list(task, &self.tasks))
            .map(|task| task.id)
            .collect();
        let len = visible_task_ids.len();
        if len == 0 {
            self.task_list_state.select(None);
            self.selected_task_id = None;
            return;
        }
        let current = self.task_list_state.selected().unwrap_or(0);
        let new = if delta > 0 {
            (current + delta as usize).min(len - 1)
        } else {
            current.saturating_sub((-delta) as usize)
        };
        self.task_list_state.select(Some(new));
        self.selected_task_id = visible_task_ids.get(new).copied();
    }

    fn navigate_to_task_list(&mut self, workflow_run_id: Uuid) {
        self.screen = Screen::TaskList { workflow_run_id };
        self.task_list_state = TableState::default();
        self.task_list_state.select(Some(0));
        self.current_workflow_run = None;
        self.tasks.clear();
        self.selected_task_id = None;
        self.log_view = None;
        self.log_scroll = 0;
        self.log_follow = true;
        self.reject_shell_approval(None);
        self.reject_capability_approval(None);
        self.reject_agent_selection(None);
        self.session_overrides = self.base_overrides.clone();
        self.overrides_seeded_from_run = false;
    }

    fn navigate_to_run_list(&mut self) {
        self.screen = Screen::RunList;
        self.current_workflow_run = None;
        self.tasks.clear();
        self.task_list_state.select(None);
        self.selected_task_id = None;
        self.log_view = None;
        self.log_scroll = 0;
        self.log_follow = true;
        self.reject_shell_approval(None);
        self.reject_capability_approval(None);
        self.reject_agent_selection(None);
        self.session_overrides = self.base_overrides.clone();
        self.overrides_seeded_from_run = true;
    }

    fn sync_log_view(&mut self) {
        let Some(current_log_view) = self.log_view.as_ref() else {
            return;
        };

        if let Some(task) = self
            .tasks
            .iter()
            .find(|task| task.id == current_log_view.task_id)
        {
            let previous_line_count = current_log_view.lines.len();
            self.log_view = Some(LogView::from_task(task));
            if self.log_follow {
                self.log_scroll = 0;
            } else if task.logs.len() < previous_line_count {
                self.log_scroll = 0;
                self.log_follow = true;
            }
        } else {
            self.log_view = None;
            self.log_scroll = 0;
            self.log_follow = true;
        }
    }

    fn visible_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|task| task_visible_in_list(task, &self.tasks))
            .collect()
    }

    fn sync_task_selection(&mut self) {
        let visible_task_ids: Vec<Uuid> = self
            .tasks
            .iter()
            .filter(|task| task_visible_in_list(task, &self.tasks))
            .map(|task| task.id)
            .collect();

        if visible_task_ids.is_empty() {
            self.task_list_state.select(None);
            self.selected_task_id = None;
            return;
        }

        let selected_idx = self
            .selected_task_id
            .and_then(|task_id| {
                visible_task_ids
                    .iter()
                    .position(|visible_id| *visible_id == task_id)
            })
            .unwrap_or(0);

        self.task_list_state.select(Some(selected_idx));
        self.selected_task_id = visible_task_ids.get(selected_idx).copied();
    }

    pub fn has_shell_approval(&self) -> bool {
        self.shell_approval.is_some()
    }

    pub fn has_capability_approval(&self) -> bool {
        self.capability_approval.is_some()
    }

    pub fn has_agent_selection(&self) -> bool {
        self.agent_selection.is_some()
    }

    pub fn shell_approval_request(&self) -> Option<&ShellCommandExecutionRequest> {
        self.shell_approval
            .as_ref()
            .map(|approval| &approval.request)
    }

    pub fn present_shell_approval(&mut self, approval: PendingShellApproval) -> bool {
        self.log_view = None;
        self.log_scroll = 0;
        self.log_follow = true;
        self.shell_approval = Some(approval);
        true
    }

    pub fn capability_approval_modules(&self) -> Option<&[LlrtSupportedModules]> {
        self.capability_approval
            .as_ref()
            .map(|approval| approval.modules.as_slice())
    }

    pub fn agent_selection_options(&self) -> Option<&[AgentSelectionItem]> {
        self.agent_selection
            .as_ref()
            .map(|selection| selection.options.as_slice())
    }

    pub fn present_capability_approval(&mut self, approval: PendingCapabilityApproval) -> bool {
        self.log_view = None;
        self.log_scroll = 0;
        self.log_follow = true;
        self.capability_approval = Some(approval);
        true
    }

    pub fn present_agent_selection(&mut self, selection: PendingAgentSelection) -> bool {
        self.log_view = None;
        self.log_scroll = 0;
        self.log_follow = true;
        self.agent_selection_cursor = 0;
        self.agent_selection = Some(selection);
        true
    }

    fn scroll_log_view(&mut self, delta: i32) {
        if delta > 0 {
            let abs_delta = delta.min(u16::MAX as i32) as u16;
            self.log_scroll = self.log_scroll.saturating_add(abs_delta);
            self.log_follow = false;
        } else if delta < 0 {
            let abs_delta = (-delta).min(u16::MAX as i32) as u16;
            self.log_scroll = self.log_scroll.saturating_sub(abs_delta);
            if self.log_scroll == 0 {
                self.log_follow = true;
            }
        }
    }

    pub fn reject_shell_approval(&mut self, error: Option<anyhow::Error>) -> bool {
        let Some(approval) = self.shell_approval.take() else {
            return false;
        };

        match error {
            Some(error) => approval.fail(error),
            None => approval.respond(false),
        }

        true
    }

    pub fn reject_capability_approval(&mut self, error: Option<anyhow::Error>) -> bool {
        let Some(approval) = self.capability_approval.take() else {
            return false;
        };

        match error {
            Some(error) => approval.fail(error),
            None => approval.respond(false),
        }

        true
    }

    pub fn reject_agent_selection(&mut self, error: Option<anyhow::Error>) -> bool {
        let Some(selection) = self.agent_selection.take() else {
            return false;
        };

        match error {
            Some(error) => selection.fail(error),
            None => selection.respond(None),
        }

        self.agent_selection_cursor = 0;
        true
    }

    fn resolve_shell_approval(&mut self, approved: bool, quit_after: bool) {
        let Some(approval) = self.shell_approval.take() else {
            return;
        };

        approval.respond(approved);
        self.status_line = None;
        self.should_quit = quit_after;
    }

    fn resolve_capability_approval(&mut self, approved: bool, quit_after: bool) {
        let Some(approval) = self.capability_approval.take() else {
            return;
        };

        approval.respond(approved);
        self.status_line = None;
        self.should_quit = quit_after;
    }

    fn resolve_agent_selection(&mut self, selection: Option<String>, quit_after: bool) {
        let Some(agent_selection) = self.agent_selection.take() else {
            return;
        };

        agent_selection.respond(selection);
        self.agent_selection_cursor = 0;
        self.status_line = None;
        self.should_quit = quit_after;
    }

    pub fn has_live_updates(&self) -> bool {
        self.current_workflow_run
            .as_ref()
            .is_some_and(|run| run.status == WorkflowStatus::Running)
            || self
                .workflow_runs
                .iter()
                .any(|run| run.status == WorkflowStatus::Running)
            || self
                .tasks
                .iter()
                .any(|task| task.status == TaskStatus::Running)
            || self
                .log_view
                .as_ref()
                .is_some_and(|log_view| log_view.status == TaskStatus::Running)
    }
}

fn sort_tasks(tasks: &mut [Task], workflow: Option<&Workflow>) {
    tasks.sort_by(|a, b| cmp_tasks_by_workflow_order(a, b, workflow));
}

fn hash_workflow_runs(hasher: &mut DefaultHasher, workflow_runs: &[WorkflowRun]) {
    workflow_runs.len().hash(hasher);
    for run in workflow_runs {
        hash_workflow_run(hasher, run);
    }
}

fn hash_optional_workflow_run(hasher: &mut DefaultHasher, workflow_run: Option<&WorkflowRun>) {
    workflow_run.is_some().hash(hasher);
    if let Some(run) = workflow_run {
        hash_workflow_run(hasher, run);
    }
}

fn hash_workflow_run(hasher: &mut DefaultHasher, run: &WorkflowRun) {
    run.id.hash(hasher);
    run.name.hash(hasher);
    run.target_path.hash(hasher);
    run.bundle_path.hash(hasher);
    hash_workflow_status(hasher, run.status);
    run.started_at.timestamp_millis().hash(hasher);
    run.ended_at
        .map(|time| time.timestamp_millis())
        .hash(hasher);
}

fn hash_tasks(hasher: &mut DefaultHasher, tasks: &[Task]) {
    tasks.len().hash(hasher);
    for task in tasks {
        task.id.hash(hasher);
        task.node_id.hash(hasher);
        task.is_master.hash(hasher);
        task.master_task_id.hash(hasher);
        hash_task_status(hasher, task.status);
        task.started_at
            .map(|time| time.timestamp_millis())
            .hash(hasher);
        task.ended_at
            .map(|time| time.timestamp_millis())
            .hash(hasher);
        task.error.hash(hasher);
        matrix_sort_key(task).hash(hasher);
        task.logs.len().hash(hasher);
        task.logs.last().hash(hasher);
    }
}

fn hash_status_line(hasher: &mut DefaultHasher, status_line: Option<&StatusLine>) {
    status_line.is_some().hash(hasher);
    if let Some(status) = status_line {
        status.message.hash(hasher);
    }
}

fn hash_log_view(hasher: &mut DefaultHasher, log_view: Option<&LogView>) {
    log_view.is_some().hash(hasher);
    if let Some(log_view) = log_view {
        log_view.task_id.hash(hasher);
        log_view.node_id.hash(hasher);
        hash_task_status(hasher, log_view.status);
        log_view.error.hash(hasher);
        log_view.lines.len().hash(hasher);
        log_view.lines.last().hash(hasher);
    }
}

fn hash_task_status(hasher: &mut DefaultHasher, status: TaskStatus) {
    task_status_discriminant(status).hash(hasher);
}

fn hash_workflow_status(hasher: &mut DefaultHasher, status: WorkflowStatus) {
    workflow_status_discriminant(status).hash(hasher);
}

fn task_status_discriminant(status: TaskStatus) -> u8 {
    match status {
        TaskStatus::Pending => 0,
        TaskStatus::Running => 1,
        TaskStatus::Completed => 2,
        TaskStatus::Failed => 3,
        TaskStatus::AwaitingTrigger => 4,
        TaskStatus::Blocked => 5,
        TaskStatus::WontDo => 6,
    }
}

fn workflow_status_discriminant(status: WorkflowStatus) -> u8 {
    match status {
        WorkflowStatus::Pending => 0,
        WorkflowStatus::Running => 1,
        WorkflowStatus::Completed => 2,
        WorkflowStatus::Failed => 3,
        WorkflowStatus::AwaitingTrigger => 4,
        WorkflowStatus::Canceled => 5,
    }
}

/// Order tasks like `workflow.yaml`: follow `workflow.nodes`, then matrix shards within a node.
fn cmp_tasks_by_workflow_order(a: &Task, b: &Task, workflow: Option<&Workflow>) -> Ordering {
    let pos = |task: &Task| {
        workflow.and_then(|w| w.nodes.iter().position(|n| n.id == task.node_id))
    };
    match (pos(a), pos(b)) {
        (Some(ia), Some(ib)) => ia
            .cmp(&ib)
            .then_with(|| matrix_sort_key(a).cmp(&matrix_sort_key(b)))
            .then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a
            .node_id
            .cmp(&b.node_id)
            .then_with(|| matrix_sort_key(a).cmp(&matrix_sort_key(b)))
            .then_with(|| a.id.cmp(&b.id)),
    }
}

fn matrix_sort_key(task: &Task) -> String {
    task.matrix_values
        .as_ref()
        .and_then(|matrix_values| serde_json::to_value(matrix_values).ok())
        .and_then(|value| serde_json_canonicalizer::to_vec(&value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}
