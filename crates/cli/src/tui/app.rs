use butterflow_core::workflow_runtime::{WorkflowCommand, WorkflowEvent, WorkflowSnapshot};
use butterflow_models::{step::StepAction, Task, TaskStatus, WorkflowRun};
use chrono::Utc;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::tui::event::AppEvent;

#[derive(Clone, Debug)]
pub enum Screen {
    Runs,
    RunDetail,
}

#[derive(Clone, Debug)]
pub enum ApprovalPrompt {
    Shell {
        request_id: Uuid,
        command: String,
    },
    Capabilities {
        request_id: Uuid,
        modules: Vec<String>,
    },
    AgentSelection {
        request_id: Uuid,
        options: Vec<(String, bool)>,
        selected: usize,
    },
    Selection {
        request_id: Uuid,
        title: String,
        options: Vec<(String, String)>,
        selected: usize,
    },
}

#[derive(Clone, Debug)]
pub struct LogModalNotice {
    pub message: String,
    pub expires_at: Instant,
}

#[derive(Clone, Debug)]
pub struct TuiState {
    pub screen: Screen,
    pub runs: Vec<WorkflowRun>,
    pub selected_run: usize,
    pub current_run: Option<WorkflowRun>,
    pub tasks: Vec<Task>,
    pub task_progress: HashMap<Uuid, TaskProgressView>,
    pub selected_task: usize,
    pub task_list_scroll: usize,
    pub approval: Option<ApprovalPrompt>,
    pub show_log_modal: bool,
    pub log_modal_scroll: u16,
    pub log_modal_notice: Option<LogModalNotice>,
}

#[derive(Clone, Debug, Default)]
pub struct TaskProgressView {
    pub processed_files: u64,
    pub total_files: Option<u64>,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            screen: Screen::Runs,
            runs: Vec::new(),
            selected_run: 0,
            current_run: None,
            tasks: Vec::new(),
            task_progress: HashMap::new(),
            selected_task: 0,
            task_list_scroll: 0,
            approval: None,
            show_log_modal: false,
            log_modal_scroll: 0,
            log_modal_notice: None,
        }
    }
}

impl TuiState {
    const LOG_MODAL_NOTICE_TTL: Duration = Duration::from_secs(2);

    fn is_terminal_task_status(status: TaskStatus) -> bool {
        matches!(
            status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::WontDo
        )
    }

    fn is_install_skill_task(&self, task: &Task) -> bool {
        self.current_run
            .as_ref()
            .and_then(|run| {
                run.workflow
                    .nodes
                    .iter()
                    .find(|node| node.id == task.node_id)
            })
            .map(|node| {
                node.steps
                    .iter()
                    .any(|step| matches!(step.action, StepAction::InstallSkill(_)))
            })
            .unwrap_or_else(|| task.node_id == "install-skill")
    }

    fn is_ignorable_pending_install_skill(&self, task: &Task) -> bool {
        self.is_install_skill_task(task) && task.status == TaskStatus::AwaitingTrigger
    }

    fn is_individually_triggerable_task(&self, task: &Task) -> bool {
        task.status == TaskStatus::AwaitingTrigger && self.task_dependencies_satisfied(task)
    }

    fn is_bulk_triggerable_task(&self, task: &Task) -> bool {
        self.is_individually_triggerable_task(task) && !self.is_install_skill_task(task)
    }

    fn task_dependencies_satisfied(&self, task: &Task) -> bool {
        let Some(run) = self.current_run.as_ref() else {
            return true;
        };
        let Some(node) = run
            .workflow
            .nodes
            .iter()
            .find(|node| node.id == task.node_id)
        else {
            return true;
        };

        node.depends_on.iter().all(|dependency_node_id| {
            let mut dependency_tasks = self
                .tasks
                .iter()
                .filter(|candidate| candidate.node_id == *dependency_node_id);

            dependency_tasks.clone().next().is_none()
                || dependency_tasks.all(|dependency_task| {
                    matches!(
                        dependency_task.status,
                        TaskStatus::Completed | TaskStatus::WontDo
                    )
                })
        })
    }

    pub fn visible_tasks(&self) -> Vec<&Task> {
        self.tasks.iter().filter(|task| !task.is_master).collect()
    }

    pub fn is_effectively_complete(&self) -> bool {
        let Some(run) = self.current_run.as_ref() else {
            return false;
        };

        if self.tasks.is_empty() {
            return matches!(run.status, butterflow_models::WorkflowStatus::Completed);
        }

        self.tasks.iter().all(|task| {
            Self::is_terminal_task_status(task.status)
                || self.is_ignorable_pending_install_skill(task)
        })
    }

    pub fn display_run_status(&self) -> String {
        let Some(run) = self.current_run.as_ref() else {
            return "Unknown".to_string();
        };

        if self.is_effectively_complete()
            && matches!(
                run.status,
                butterflow_models::WorkflowStatus::AwaitingTrigger
            )
        {
            "Completed (install-skill pending)".to_string()
        } else {
            Self::workflow_status_text(run.status)
        }
    }

    pub fn display_workflow_name(&self) -> String {
        let Some(run) = self.current_run.as_ref() else {
            return "Workflow".to_string();
        };

        Self::workflow_run_display_name(run)
    }

    pub fn display_target_path(&self) -> Option<String> {
        self.current_run
            .as_ref()
            .and_then(|run| run.target_path.as_ref())
            .map(|path| path.display().to_string())
    }

    pub fn workflow_status_text(status: butterflow_models::WorkflowStatus) -> String {
        match status {
            butterflow_models::WorkflowStatus::Pending => "Pending".to_string(),
            butterflow_models::WorkflowStatus::Running => "Running".to_string(),
            butterflow_models::WorkflowStatus::Completed => "Completed".to_string(),
            butterflow_models::WorkflowStatus::Failed => "Failed".to_string(),
            butterflow_models::WorkflowStatus::AwaitingTrigger => "Awaiting trigger".to_string(),
            butterflow_models::WorkflowStatus::Canceled => "Canceled".to_string(),
        }
    }

    pub fn workflow_run_display_name(run: &WorkflowRun) -> String {
        if let Some(name) = run.name.as_deref() {
            let trimmed = name.trim();
            let lower = trimmed.to_ascii_lowercase();
            if !trimmed.is_empty() && lower != "workflow.yaml" && lower != "workflow.yml" {
                return trimmed.to_string();
            }
        }

        run.bundle_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .or_else(|| run.name.clone())
            .unwrap_or_else(|| "Workflow".to_string())
    }

    pub fn workflow_elapsed_text(run: &WorkflowRun) -> String {
        let ended_at = run.ended_at.unwrap_or_else(Utc::now);
        let duration = ended_at.signed_duration_since(run.started_at);
        let total_seconds = duration.num_seconds().max(0);
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;

        if hours > 0 {
            format!("{hours:02}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes:02}:{seconds:02}")
        }
    }

    fn clamp_selected_task(&mut self) {
        let visible_len = self.visible_tasks().len();
        if self.selected_task >= visible_len {
            self.selected_task = visible_len.saturating_sub(1);
        }
        if visible_len == 0 {
            self.task_list_scroll = 0;
        } else if self.task_list_scroll >= visible_len {
            self.task_list_scroll = visible_len.saturating_sub(1);
        }
    }

    pub fn sync_task_list_scroll(&mut self, viewport_height: usize) {
        let visible_len = self.visible_tasks().len();
        if visible_len == 0 || viewport_height == 0 {
            self.task_list_scroll = 0;
            return;
        }

        let max_scroll = visible_len.saturating_sub(viewport_height);
        if self.selected_task < self.task_list_scroll {
            self.task_list_scroll = self.selected_task;
        } else if self.selected_task >= self.task_list_scroll.saturating_add(viewport_height) {
            self.task_list_scroll = self
                .selected_task
                .saturating_add(1)
                .saturating_sub(viewport_height);
        }
        self.task_list_scroll = self.task_list_scroll.min(max_scroll);
    }

    pub fn visible_task_window(&self, viewport_height: usize) -> Vec<&Task> {
        let tasks = self.visible_tasks();
        if viewport_height == 0 {
            return Vec::new();
        }
        let start = self.task_list_scroll.min(tasks.len());
        let end = start.saturating_add(viewport_height).min(tasks.len());
        tasks[start..end].to_vec()
    }

    pub fn set_runs(&mut self, runs: Vec<WorkflowRun>) {
        self.runs = runs;
        if self.selected_run >= self.runs.len() {
            self.selected_run = self.runs.len().saturating_sub(1);
        }
    }

    pub fn enter_run(&mut self, snapshot: WorkflowSnapshot) {
        if let Some(existing) = self
            .runs
            .iter_mut()
            .find(|run| run.id == snapshot.workflow_run.id)
        {
            *existing = snapshot.workflow_run.clone();
        } else {
            self.runs.insert(0, snapshot.workflow_run.clone());
        }
        self.screen = Screen::RunDetail;
        self.current_run = Some(snapshot.workflow_run);
        self.tasks = snapshot.tasks;
        self.task_progress.clear();
        self.selected_task = 0;
        self.task_list_scroll = 0;
        self.approval = None;
        self.show_log_modal = false;
        self.log_modal_scroll = 0;
    }

    pub fn reconcile_snapshot(&mut self, snapshot: WorkflowSnapshot) {
        let selected_task_id = self.selected_task().map(|task| task.id);
        if let Some(existing) = self
            .runs
            .iter_mut()
            .find(|run| run.id == snapshot.workflow_run.id)
        {
            *existing = snapshot.workflow_run.clone();
        } else {
            self.runs.insert(0, snapshot.workflow_run.clone());
        }
        self.current_run = Some(snapshot.workflow_run);
        self.tasks = snapshot.tasks;
        self.task_progress
            .retain(|task_id, _| self.tasks.iter().any(|task| task.id == *task_id));
        if let Some(selected_task_id) = selected_task_id {
            if let Some(index) = self
                .visible_tasks()
                .iter()
                .position(|task| task.id == selected_task_id)
            {
                self.selected_task = index;
                return;
            }
        }
        self.clamp_selected_task();
    }

    pub fn leave_run(&mut self) {
        self.screen = Screen::Runs;
        self.current_run = None;
        self.tasks.clear();
        self.task_progress.clear();
        self.selected_task = 0;
        self.task_list_scroll = 0;
        self.approval = None;
        self.show_log_modal = false;
        self.log_modal_scroll = 0;
    }

    pub fn selected_run_id(&self) -> Option<Uuid> {
        self.runs.get(self.selected_run).map(|run| run.id)
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.visible_tasks().get(self.selected_task).copied()
    }

    pub fn task_display_name(&self, task: &Task) -> String {
        let base_name = self
            .current_run
            .as_ref()
            .and_then(|run| {
                run.workflow
                    .nodes
                    .iter()
                    .find(|node| node.id == task.node_id)
            })
            .map(|node| node.name.clone())
            .unwrap_or_else(|| task.node_id.clone());

        if let Some(shard_label) = self.task_matrix_label(task) {
            format!("{base_name} · {shard_label}")
        } else {
            base_name
        }
    }

    fn task_matrix_label(&self, task: &Task) -> Option<String> {
        let matrix_values = task.matrix_values.as_ref()?;

        for preferred_key in ["name", "shardId", "task", "shard"] {
            if let Some(value) = matrix_values
                .get(preferred_key)
                .and_then(|value| value.as_str())
            {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }

        let mut scalar_pairs = matrix_values
            .iter()
            .filter(|(key, _)| !key.starts_with("_meta"))
            .filter_map(|(key, value)| {
                let rendered = match value {
                    serde_json::Value::String(value) => Some(value.clone()),
                    serde_json::Value::Number(value) => Some(value.to_string()),
                    serde_json::Value::Bool(value) => Some(value.to_string()),
                    _ => None,
                }?;
                Some((key.clone(), rendered))
            })
            .collect::<Vec<_>>();

        scalar_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        if scalar_pairs.is_empty() {
            return None;
        }

        Some(
            scalar_pairs
                .into_iter()
                .take(2)
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }

    pub fn task_elapsed_text(&self, task: &Task) -> String {
        let Some(started_at) = task.started_at else {
            return "-".to_string();
        };

        let ended_at = task.ended_at.unwrap_or_else(Utc::now);
        let duration = ended_at.signed_duration_since(started_at);
        let total_seconds = duration.num_seconds().max(0);
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;

        if hours > 0 {
            format!("{hours:02}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes:02}:{seconds:02}")
        }
    }

    pub fn task_progress_counts(&self, task: &Task) -> Option<(usize, usize)> {
        if let Some(progress) = self.task_progress.get(&task.id) {
            if let Some(total) = progress.total_files {
                let processed = if task.status == TaskStatus::Completed {
                    total
                } else {
                    progress.processed_files.min(total)
                };
                return Some((processed as usize, total as usize));
            }
        }

        let total = task.logs.iter().find_map(|line| {
            let prefix = "Starting js-ast-grep file loop (";
            let (_, rest) = line.split_once(prefix)?;
            let marker = "target files: ";
            let (_, count_text) = rest.split_once(marker)?;
            let count_text = count_text.trim_end_matches(')').trim();
            if count_text == "unknown" {
                None
            } else {
                count_text.parse::<usize>().ok()
            }
        })?;

        let processed = task
            .logs
            .iter()
            .filter(|line| line.starts_with("Processing file: "))
            .count();
        let processed = if task.status == TaskStatus::Completed {
            total
        } else {
            processed.min(total)
        };
        Some((processed, total))
    }

    pub fn task_progress_bar(&self, task: &Task, width: usize) -> Option<String> {
        if width < 3 {
            return None;
        }

        let (processed, total) = self.task_progress_counts(task)?;
        if total == 0 {
            return None;
        }

        let inner_width = width.saturating_sub(2);
        let mut bar = String::with_capacity(width);
        bar.push('[');
        if task.status == TaskStatus::Completed {
            for _ in 0..inner_width {
                bar.push('=');
            }
        } else {
            let mut filled = processed.saturating_mul(inner_width) / total;
            if filled >= inner_width {
                filled = inner_width.saturating_sub(1);
            }

            for index in 0..inner_width {
                if index < filled {
                    bar.push('=');
                } else if index == filled {
                    bar.push('>');
                } else {
                    bar.push(' ');
                }
            }
        }
        bar.push(']');
        Some(bar)
    }

    pub fn selected_task_log_text(&self) -> String {
        self.selected_task()
            .map(|task| {
                let mut lines = task.logs.clone();

                if task.status == TaskStatus::Failed {
                    if let Some(error) = task.error.as_deref() {
                        let rendered_error = format!("Error: {error}");
                        if !lines.iter().any(|line| line == &rendered_error) {
                            lines.push(rendered_error);
                        }
                    }
                }

                if lines.is_empty() {
                    "No logs yet".to_string()
                } else {
                    lines.join("\n")
                }
            })
            .unwrap_or_else(|| "No task selected".to_string())
    }

    pub fn open_log_modal(&mut self, viewport_height: u16) {
        if self.selected_task().is_none() {
            return;
        }
        self.show_log_modal = true;
        self.log_modal_notice = None;
        self.scroll_logs_to_bottom(viewport_height);
    }

    pub fn close_log_modal(&mut self) {
        self.show_log_modal = false;
        self.log_modal_scroll = 0;
        self.log_modal_notice = None;
    }

    pub fn set_log_modal_notice(&mut self, notice: impl Into<String>) {
        self.set_log_modal_notice_for(notice, Self::LOG_MODAL_NOTICE_TTL);
    }

    fn set_log_modal_notice_for(&mut self, notice: impl Into<String>, ttl: Duration) {
        self.log_modal_notice = Some(LogModalNotice {
            message: notice.into(),
            expires_at: Instant::now() + ttl,
        });
    }

    pub fn clear_expired_log_modal_notice(&mut self) {
        if self
            .log_modal_notice
            .as_ref()
            .is_some_and(|notice| Instant::now() >= notice.expires_at)
        {
            self.log_modal_notice = None;
        }
    }

    pub fn log_modal_notice_text(&self) -> Option<&str> {
        self.log_modal_notice
            .as_ref()
            .map(|notice| notice.message.as_str())
    }

    pub fn log_modal_max_scroll(&self, viewport_height: u16) -> u16 {
        let line_count = self.selected_task_log_text().lines().count();
        line_count.saturating_sub(viewport_height as usize) as u16
    }

    pub fn scroll_logs_up(&mut self, amount: u16) {
        self.log_modal_scroll = self.log_modal_scroll.saturating_sub(amount);
    }

    pub fn scroll_logs_down(&mut self, viewport_height: u16, amount: u16) {
        self.log_modal_scroll = self
            .log_modal_scroll
            .saturating_add(amount)
            .min(self.log_modal_max_scroll(viewport_height));
    }
    pub fn scroll_logs_to_top(&mut self) {
        self.log_modal_scroll = 0;
    }

    pub fn scroll_logs_to_bottom(&mut self, viewport_height: u16) {
        self.log_modal_scroll = self.log_modal_max_scroll(viewport_height);
    }

    pub fn move_up(&mut self) {
        match self.screen {
            Screen::Runs => {
                self.selected_run = self.selected_run.saturating_sub(1);
            }
            Screen::RunDetail => match &mut self.approval {
                Some(ApprovalPrompt::AgentSelection { selected, .. })
                | Some(ApprovalPrompt::Selection { selected, .. }) => {
                    *selected = selected.saturating_sub(1);
                }
                _ => {
                    self.selected_task = self.selected_task.saturating_sub(1);
                }
            },
        }
    }

    pub fn move_down(&mut self) {
        match self.screen {
            Screen::Runs => {
                if !self.runs.is_empty() {
                    self.selected_run = (self.selected_run + 1).min(self.runs.len() - 1);
                }
            }
            Screen::RunDetail => match &mut self.approval {
                Some(ApprovalPrompt::AgentSelection {
                    selected, options, ..
                }) => {
                    if !options.is_empty() {
                        *selected = (*selected + 1).min(options.len() - 1);
                    }
                }
                Some(ApprovalPrompt::Selection {
                    selected, options, ..
                }) => {
                    if !options.is_empty() {
                        *selected = (*selected + 1).min(options.len() - 1);
                    }
                }
                _ => {
                    let visible_len = self.visible_tasks().len();
                    if visible_len > 0 {
                        self.selected_task = (self.selected_task + 1).min(visible_len - 1);
                    }
                }
            },
        }
    }

    pub fn reduce(&mut self, event: AppEvent) {
        match event {
            AppEvent::Workflow(workflow_event) => self.reduce_workflow_event(workflow_event),
            AppEvent::Snapshot(snapshot) => self.reconcile_snapshot(snapshot),
        }
    }

    fn reduce_workflow_event(&mut self, event: WorkflowEvent) {
        match event {
            WorkflowEvent::WorkflowStarted { workflow_run, .. } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == workflow_run.id) {
                    *run = workflow_run.clone();
                } else {
                    self.runs.insert(0, workflow_run.clone());
                }
                if self.current_run.as_ref().map(|run| run.id) == Some(workflow_run.id) {
                    self.current_run = Some(workflow_run);
                }
            }
            WorkflowEvent::WorkflowStatusChanged {
                workflow_run_id,
                status,
                ..
            } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == workflow_run_id) {
                    run.status = status;
                }
                if let Some(run) = self.current_run.as_mut() {
                    if run.id == workflow_run_id {
                        run.status = status;
                    }
                }
            }
            WorkflowEvent::TaskCreated { task, .. } => {
                if let Some(existing) = self
                    .tasks
                    .iter_mut()
                    .find(|existing| existing.id == task.id)
                {
                    *existing = task;
                } else {
                    self.tasks.push(task);
                }
                self.clamp_selected_task();
            }
            WorkflowEvent::TaskUpdated { task, .. } => {
                if let Some(existing) = self
                    .tasks
                    .iter_mut()
                    .find(|existing| existing.id == task.id)
                {
                    *existing = task;
                } else {
                    self.tasks.push(task);
                }
                self.clamp_selected_task();
            }
            WorkflowEvent::TaskLogAppended { task_id, line, .. } => {
                if let Some(task) = self.tasks.iter_mut().find(|task| task.id == task_id) {
                    task.logs.push(line);
                }
            }
            WorkflowEvent::TaskProgressUpdated {
                task_id,
                processed_files,
                total_files,
                current_file: _,
                ..
            } => {
                self.task_progress.insert(
                    task_id,
                    TaskProgressView {
                        processed_files,
                        total_files,
                    },
                );
            }
            WorkflowEvent::ShellApprovalRequested {
                request_id,
                request,
                ..
            } => {
                self.approval = Some(ApprovalPrompt::Shell {
                    request_id,
                    command: request.command,
                });
            }
            WorkflowEvent::CapabilitiesApprovalRequested {
                request_id,
                modules,
                ..
            } => {
                self.approval = Some(ApprovalPrompt::Capabilities {
                    request_id,
                    modules: modules
                        .into_iter()
                        .map(|module| format!("{module:?}"))
                        .collect(),
                });
            }
            WorkflowEvent::AgentSelectionRequested {
                request_id,
                options,
                ..
            } => {
                self.approval = Some(ApprovalPrompt::AgentSelection {
                    request_id,
                    options: options
                        .into_iter()
                        .map(|option| {
                            (
                                format!(
                                    "{}{}",
                                    option.label,
                                    if option.is_available {
                                        ""
                                    } else {
                                        " (not installed)"
                                    }
                                ),
                                option.is_available,
                            )
                        })
                        .collect(),
                    selected: 0,
                });
            }
            WorkflowEvent::SelectionRequested {
                request_id, prompt, ..
            } => {
                self.approval = Some(ApprovalPrompt::Selection {
                    request_id,
                    title: prompt.title,
                    options: prompt
                        .options
                        .into_iter()
                        .map(|option| (option.value, option.label))
                        .collect(),
                    selected: prompt.default_index,
                });
            }
        }
    }

    pub fn approval_accept_command(&self) -> Option<WorkflowCommand> {
        match self.approval.as_ref()? {
            ApprovalPrompt::Shell { request_id, .. } => {
                Some(WorkflowCommand::RespondShellApproval {
                    request_id: *request_id,
                    approved: true,
                })
            }
            ApprovalPrompt::Capabilities { request_id, .. } => {
                Some(WorkflowCommand::RespondCapabilitiesApproval {
                    request_id: *request_id,
                    approved: true,
                })
            }
            ApprovalPrompt::AgentSelection {
                request_id,
                options,
                selected,
            } => options.get(*selected).map(|(label, available)| {
                WorkflowCommand::RespondAgentSelection {
                    request_id: *request_id,
                    selection: if *available {
                        Some(
                            label
                                .split(" (")
                                .next()
                                .unwrap_or(label)
                                .to_ascii_lowercase()
                                .replace(' ', "-"),
                        )
                    } else {
                        None
                    },
                }
            }),
            ApprovalPrompt::Selection {
                request_id,
                options,
                selected,
                ..
            } => options
                .get(*selected)
                .map(|(value, _)| WorkflowCommand::RespondSelection {
                    request_id: *request_id,
                    selection: Some(value.clone()),
                }),
        }
    }

    pub fn approval_reject_command(&self) -> Option<WorkflowCommand> {
        match self.approval.as_ref()? {
            ApprovalPrompt::Shell { request_id, .. } => {
                Some(WorkflowCommand::RespondShellApproval {
                    request_id: *request_id,
                    approved: false,
                })
            }
            ApprovalPrompt::Capabilities { request_id, .. } => {
                Some(WorkflowCommand::RespondCapabilitiesApproval {
                    request_id: *request_id,
                    approved: false,
                })
            }
            ApprovalPrompt::AgentSelection { request_id, .. } => {
                Some(WorkflowCommand::RespondAgentSelection {
                    request_id: *request_id,
                    selection: None,
                })
            }
            ApprovalPrompt::Selection { request_id, .. } => {
                Some(WorkflowCommand::RespondSelection {
                    request_id: *request_id,
                    selection: None,
                })
            }
        }
    }

    pub fn clear_approval(&mut self) {
        self.approval = None;
    }

    pub fn selected_task_trigger_command(&self) -> Option<WorkflowCommand> {
        let task = self.selected_task()?;
        if self.is_individually_triggerable_task(task) {
            Some(WorkflowCommand::TriggerTask { task_id: task.id })
        } else {
            None
        }
    }

    pub fn visible_awaiting_task_ids(&self) -> Vec<Uuid> {
        self.visible_tasks()
            .into_iter()
            .filter(|task| self.is_bulk_triggerable_task(task))
            .map(|task| task.id)
            .collect()
    }

    pub fn task_help_text(&self) -> String {
        let mut parts = vec!["Enter logs".to_string()];
        if self.selected_task_trigger_command().is_some() {
            parts.push("t trigger".to_string());
        }
        if !self.visible_awaiting_task_ids().is_empty() {
            parts.push("T trigger-all".to_string());
        }
        parts.push("c cancel".to_string());
        parts.push("esc back".to_string());
        parts.push("q quit".to_string());
        parts.join("  ")
    }

    pub fn selected_task_completion_detail(&self) -> Option<String> {
        let task = self.selected_task()?;
        if task.status != TaskStatus::Completed {
            return None;
        }

        let pr_url = task.logs.iter().rev().find_map(|line| {
            line.strip_prefix("Pull request created: ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        });

        let branch_name = task.logs.iter().rev().find_map(|line| {
            line.strip_prefix("Preparing git worktree for branch ")
                .and_then(|value| value.split(" in ").next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    line.strip_prefix("Creating git worktree for branch ")
                        .and_then(|value| value.split(" in ").next())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                })
        });

        match (pr_url, branch_name) {
            (Some(pr_url), Some(branch_name)) => {
                Some(format!("Branch: {branch_name}  PR: {pr_url}"))
            }
            (Some(pr_url), None) => Some(format!("PR: {pr_url}")),
            (None, _) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use butterflow_core::workflow_runtime::{WorkflowCommand, WorkflowEvent, WorkflowSnapshot};
    use butterflow_models::{
        node::NodeType,
        step::{Step, StepAction, UseInstallSkill},
        Task, TaskStatus, Workflow, WorkflowRun, WorkflowStatus,
    };
    use chrono::Utc;
    use std::time::Duration;
    use uuid::Uuid;

    use super::{AppEvent, TaskProgressView, TuiState};

    #[test]
    fn reducer_updates_run_status_from_runtime_event() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.runs.push(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status: WorkflowStatus::Pending,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        state.reduce(AppEvent::Workflow(WorkflowEvent::WorkflowStatusChanged {
            workflow_run_id: run_id,
            status: WorkflowStatus::Running,
            at: Utc::now(),
        }));
        assert_eq!(state.runs[0].status, WorkflowStatus::Running);
    }

    #[test]
    fn reducer_updates_task_log_from_runtime_event() {
        let run_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: task_id,
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Pending,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });
        state.reduce(AppEvent::Workflow(WorkflowEvent::TaskLogAppended {
            workflow_run_id: run_id,
            task_id,
            line: "hello".to_string(),
            at: Utc::now(),
        }));
        assert_eq!(state.tasks[0].logs, vec!["hello".to_string()]);
    }

    #[test]
    fn reducer_updates_task_progress_from_runtime_event() {
        let run_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: task_id,
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Running,
            started_at: Some(Utc::now()),
            ended_at: None,
            logs: vec![
                "Starting js-ast-grep file loop (explicit-files, target files: 10)".to_string(),
            ],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        state.reduce(AppEvent::Workflow(WorkflowEvent::TaskProgressUpdated {
            workflow_run_id: run_id,
            task_id,
            processed_files: 6,
            total_files: Some(10),
            current_file: Some("src/example.ts".to_string()),
            at: Utc::now(),
        }));

        assert_eq!(state.task_progress_counts(&state.tasks[0]), Some((6, 10)));
    }

    #[test]
    fn visible_tasks_hide_master_tasks() {
        let run_id = Uuid::new_v4();
        let master_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: master_id,
            workflow_run_id: run_id,
            node_id: "matrix-node".to_string(),
            status: TaskStatus::AwaitingTrigger,
            is_master: true,
            master_task_id: None,
            matrix_values: None,
            started_at: None,
            ended_at: None,
            error: None,
            logs: vec![],
        });
        state.tasks.push(Task {
            id: child_id,
            workflow_run_id: run_id,
            node_id: "matrix-node".to_string(),
            status: TaskStatus::AwaitingTrigger,
            is_master: false,
            master_task_id: Some(master_id),
            matrix_values: Some(Default::default()),
            started_at: None,
            ended_at: None,
            error: None,
            logs: vec![],
        });

        let visible = state.visible_tasks();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, child_id);
    }

    #[test]
    fn display_run_status_treats_install_skill_as_effectively_complete() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status: WorkflowStatus::AwaitingTrigger,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        state.tasks = vec![
            Task {
                id: Uuid::new_v4(),
                workflow_run_id: run_id,
                node_id: "apply-transforms".to_string(),
                status: TaskStatus::Completed,
                started_at: None,
                ended_at: None,
                logs: vec![],
                master_task_id: None,
                matrix_values: None,
                is_master: false,
                error: None,
            },
            Task {
                id: Uuid::new_v4(),
                workflow_run_id: run_id,
                node_id: "install-skill".to_string(),
                status: TaskStatus::AwaitingTrigger,
                started_at: None,
                ended_at: None,
                logs: vec![],
                master_task_id: None,
                matrix_values: None,
                is_master: false,
                error: None,
            },
        ];

        assert!(state.is_effectively_complete());
        assert_eq!(
            state.display_run_status(),
            "Completed (install-skill pending)"
        );
    }

    #[test]
    fn display_run_status_does_not_ignore_running_install_skill() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
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
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        state.tasks = vec![Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "install-skill".to_string(),
            status: TaskStatus::Running,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        }];

        assert!(!state.is_effectively_complete());
        assert_eq!(state.display_run_status(), "Running");
    }

    #[test]
    fn display_run_status_does_not_ignore_failed_install_skill() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status: WorkflowStatus::Failed,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        state.tasks = vec![Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "install-skill".to_string(),
            status: TaskStatus::Failed,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: Some("boom".to_string()),
        }];

        assert!(state.is_effectively_complete());
        assert_eq!(state.display_run_status(), "Failed");
    }

    #[test]
    fn display_run_status_spaces_awaiting_trigger() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status: WorkflowStatus::AwaitingTrigger,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });

        assert_eq!(state.display_run_status(), "Awaiting trigger");
    }

    #[test]
    fn display_workflow_name_prefers_bundle_dir_when_run_name_is_workflow_yaml() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
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
            bundle_path: Some(std::path::PathBuf::from(
                "/Users/sahilmobaidin/Desktop/myprojects/useful-codemods/codemods/debarrel",
            )),
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: Some("workflow.yaml".to_string()),
            target_path: None,
        });

        assert_eq!(state.display_workflow_name(), "debarrel");
    }

    #[test]
    fn workflow_elapsed_text_formats_running_run() {
        let now = Utc::now();
        let run = WorkflowRun {
            id: Uuid::new_v4(),
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
            started_at: now - chrono::Duration::seconds(65),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        };

        assert!(matches!(
            TuiState::workflow_elapsed_text(&run).as_str(),
            "01:05" | "01:06"
        ));
    }

    #[test]
    fn workflow_elapsed_text_formats_completed_run() {
        let started_at = Utc::now() - chrono::Duration::seconds(125);
        let run = WorkflowRun {
            id: Uuid::new_v4(),
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![],
            },
            status: WorkflowStatus::Completed,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at,
            ended_at: Some(started_at + chrono::Duration::seconds(125)),
            capabilities: None,
            name: None,
            target_path: None,
        };

        assert_eq!(TuiState::workflow_elapsed_text(&run), "02:05");
    }

    #[test]
    fn task_progress_counts_parse_target_files_and_processed_files() {
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: Uuid::new_v4(),
            node_id: "apply-transforms".to_string(),
            status: TaskStatus::Running,
            started_at: Some(Utc::now()),
            ended_at: None,
            logs: vec![
                "Starting js-ast-grep file loop (explicit-files, target files: 5)".to_string(),
                "Processing file: src/a.ts".to_string(),
                "Processing file: src/b.ts".to_string(),
            ],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        };

        assert_eq!(
            TuiState::default().task_progress_counts(&task),
            Some((2, 5))
        );
    }

    #[test]
    fn task_progress_bar_fills_completed_task_to_total() {
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: Uuid::new_v4(),
            node_id: "apply-transforms".to_string(),
            status: TaskStatus::Completed,
            started_at: Some(Utc::now()),
            ended_at: Some(Utc::now()),
            logs: vec![
                "Starting js-ast-grep file loop (explicit-files, target files: 4)".to_string(),
                "Processing file: src/a.ts".to_string(),
            ],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        };

        assert_eq!(
            TuiState::default().task_progress_bar(&task, 4).as_deref(),
            Some("[==]")
        );
    }

    #[test]
    fn task_progress_bar_does_not_render_full_for_running_task() {
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: Uuid::new_v4(),
            node_id: "apply-transforms".to_string(),
            status: TaskStatus::Running,
            started_at: Some(Utc::now()),
            ended_at: None,
            logs: vec![
                "Starting js-ast-grep file loop (explicit-files, target files: 4)".to_string(),
            ],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        };
        let mut state = TuiState::default();
        state.task_progress.insert(
            task.id,
            TaskProgressView {
                processed_files: 4,
                total_files: Some(4),
            },
        );

        assert_eq!(state.task_progress_bar(&task, 6).as_deref(), Some("[===>]"));
    }

    #[test]
    fn enter_run_updates_existing_run_in_runs_list() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.runs.push(WorkflowRun {
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
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: Some("workflow.yaml".to_string()),
            target_path: None,
        });

        state.enter_run(WorkflowSnapshot {
            workflow_run: WorkflowRun {
                id: run_id,
                workflow: Workflow {
                    version: "1".to_string(),
                    state: None,
                    params: None,
                    templates: vec![],
                    nodes: vec![],
                },
                status: WorkflowStatus::AwaitingTrigger,
                params: Default::default(),
                bundle_path: Some(std::path::PathBuf::from("/tmp/debarrel")),
                tasks: vec![],
                started_at: Utc::now(),
                ended_at: None,
                capabilities: None,
                name: Some("workflow.yaml".to_string()),
                target_path: Some(std::path::PathBuf::from("/tmp/repo")),
            },
            tasks: vec![],
        });

        assert_eq!(state.runs.len(), 1);
        assert_eq!(state.runs[0].status, WorkflowStatus::AwaitingTrigger);
        assert_eq!(state.display_workflow_name(), "debarrel");
    }

    #[test]
    fn reconcile_snapshot_updates_existing_run_in_runs_list() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        let initial_run = WorkflowRun {
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
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: Some("workflow.yaml".to_string()),
            target_path: None,
        };
        state.runs.push(initial_run.clone());
        state.current_run = Some(initial_run);

        state.reconcile_snapshot(WorkflowSnapshot {
            workflow_run: WorkflowRun {
                id: run_id,
                workflow: Workflow {
                    version: "1".to_string(),
                    state: None,
                    params: None,
                    templates: vec![],
                    nodes: vec![],
                },
                status: WorkflowStatus::AwaitingTrigger,
                params: Default::default(),
                bundle_path: None,
                tasks: vec![],
                started_at: Utc::now(),
                ended_at: None,
                capabilities: None,
                name: Some("workflow.yaml".to_string()),
                target_path: None,
            },
            tasks: vec![],
        });

        assert_eq!(state.runs[0].status, WorkflowStatus::AwaitingTrigger);
        assert_eq!(
            state.current_run.as_ref().map(|run| run.status),
            Some(WorkflowStatus::AwaitingTrigger)
        );
    }

    #[test]
    fn open_log_modal_scrolls_to_bottom() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Running,
            started_at: None,
            ended_at: None,
            logs: (0..10).map(|index| format!("line {index}")).collect(),
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        state.open_log_modal(4);

        assert!(state.show_log_modal);
        assert_eq!(state.log_modal_scroll, 6);
    }

    #[test]
    fn log_modal_scroll_clamps() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Running,
            started_at: None,
            ended_at: None,
            logs: (0..6).map(|index| format!("line {index}")).collect(),
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        state.open_log_modal(3);
        state.scroll_logs_up(10);
        assert_eq!(state.log_modal_scroll, 0);

        state.scroll_logs_down(3, 10);
        assert_eq!(state.log_modal_scroll, 3);

        state.scroll_logs_to_top();
        assert_eq!(state.log_modal_scroll, 0);

        state.scroll_logs_to_bottom(3);
        assert_eq!(state.log_modal_scroll, 3);
    }

    #[test]
    fn log_modal_notice_expires() {
        let mut state = TuiState::default();
        state.set_log_modal_notice_for("Copied full log to clipboard", Duration::ZERO);

        state.clear_expired_log_modal_notice();

        assert!(state.log_modal_notice_text().is_none());
    }

    #[test]
    fn selected_task_log_text_includes_failed_task_error() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "install-skill".to_string(),
            status: TaskStatus::Failed,
            started_at: None,
            ended_at: None,
            logs: vec![
                "Task execution starting".to_string(),
                "Step started: Install debarrel skill".to_string(),
            ],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: Some("Failed to execute install-skill step".to_string()),
        });

        assert_eq!(
            state.selected_task_log_text(),
            "Task execution starting\nStep started: Install debarrel skill\nError: Failed to execute install-skill step"
        );
    }

    #[test]
    fn task_list_scroll_tracks_selection_window() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState {
            tasks: (0..6)
                .map(|index| Task {
                    id: Uuid::new_v4(),
                    workflow_run_id: run_id,
                    node_id: format!("node-{index}"),
                    status: TaskStatus::Running,
                    started_at: None,
                    ended_at: None,
                    logs: vec![],
                    master_task_id: None,
                    matrix_values: None,
                    is_master: false,
                    error: None,
                })
                .collect(),
            ..TuiState::default()
        };

        state.sync_task_list_scroll(3);
        assert_eq!(state.task_list_scroll, 0);

        state.selected_task = 3;
        state.sync_task_list_scroll(3);
        assert_eq!(state.task_list_scroll, 1);

        state.selected_task = 5;
        state.sync_task_list_scroll(3);
        assert_eq!(state.task_list_scroll, 3);

        state.selected_task = 1;
        state.sync_task_list_scroll(3);
        assert_eq!(state.task_list_scroll, 1);
    }

    #[test]
    fn task_help_text_hides_trigger_for_completed_task() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Completed,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        assert_eq!(
            state.task_help_text(),
            "Enter logs  c cancel  esc back  q quit"
        );
    }

    #[test]
    fn task_help_text_shows_trigger_for_awaiting_task() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::AwaitingTrigger,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        assert_eq!(
            state.task_help_text(),
            "Enter logs  t trigger  T trigger-all  c cancel  esc back  q quit"
        );
    }

    #[test]
    fn task_help_text_shows_individual_trigger_only_for_install_skill() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "install-skill".to_string(),
            status: TaskStatus::AwaitingTrigger,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        assert_eq!(
            state.task_help_text(),
            "Enter logs  t trigger  c cancel  esc back  q quit"
        );
    }

    #[test]
    fn selected_task_trigger_command_requires_dependencies() {
        let run_id = Uuid::new_v4();
        let dependency_task_id = Uuid::new_v4();
        let blocked_task_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![
                    butterflow_models::Node {
                        id: "install-skill".to_string(),
                        name: "Install Skill".to_string(),
                        description: None,
                        r#type: NodeType::Manual,
                        depends_on: vec![],
                        trigger: None,
                        strategy: None,
                        runtime: None,
                        steps: vec![],
                        env: Default::default(),
                        branch_name: None,
                        pull_request: None,
                    },
                    butterflow_models::Node {
                        id: "apply-transforms".to_string(),
                        name: "Apply transforms".to_string(),
                        description: None,
                        r#type: NodeType::Manual,
                        depends_on: vec!["install-skill".to_string()],
                        trigger: None,
                        strategy: None,
                        runtime: None,
                        steps: vec![],
                        env: Default::default(),
                        branch_name: None,
                        pull_request: None,
                    },
                ],
            },
            status: WorkflowStatus::AwaitingTrigger,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        state.tasks = vec![
            Task {
                id: dependency_task_id,
                workflow_run_id: run_id,
                node_id: "install-skill".to_string(),
                status: TaskStatus::AwaitingTrigger,
                started_at: None,
                ended_at: None,
                logs: vec![],
                master_task_id: None,
                matrix_values: None,
                is_master: false,
                error: None,
            },
            Task {
                id: blocked_task_id,
                workflow_run_id: run_id,
                node_id: "apply-transforms".to_string(),
                status: TaskStatus::AwaitingTrigger,
                started_at: None,
                ended_at: None,
                logs: vec![],
                master_task_id: None,
                matrix_values: None,
                is_master: false,
                error: None,
            },
        ];
        state.selected_task = 1;

        assert_eq!(
            state.selected_task().map(|task| task.id),
            Some(blocked_task_id)
        );
        assert!(state.selected_task_trigger_command().is_none());
        assert!(state.visible_awaiting_task_ids().is_empty());
    }

    #[test]
    fn install_skill_is_individually_triggerable_but_excluded_from_trigger_all() {
        let run_id = Uuid::new_v4();
        let install_skill_task_id = Uuid::new_v4();
        let normal_task_id = Uuid::new_v4();
        let state = TuiState {
            current_run: Some(WorkflowRun {
                id: run_id,
                workflow: Workflow {
                    version: "1".to_string(),
                    state: None,
                    params: None,
                    templates: vec![],
                    nodes: vec![
                        butterflow_models::Node {
                            id: "node2".to_string(),
                            name: "Install Skill".to_string(),
                            description: None,
                            r#type: NodeType::Manual,
                            depends_on: vec![],
                            trigger: None,
                            strategy: None,
                            runtime: None,
                            steps: vec![Step {
                                id: Some("install-skill-step".to_string()),
                                name: "Install debarrel skill".to_string(),
                                action: StepAction::InstallSkill(UseInstallSkill {
                                    package: "debarrel".to_string(),
                                    path: None,
                                    harness: None,
                                    scope: None,
                                    force: None,
                                }),
                                env: None,
                                condition: None,
                                commit: None,
                            }],
                            env: Default::default(),
                            branch_name: None,
                            pull_request: None,
                        },
                        butterflow_models::Node {
                            id: "apply-transforms".to_string(),
                            name: "Apply transforms".to_string(),
                            description: None,
                            r#type: NodeType::Manual,
                            depends_on: vec![],
                            trigger: None,
                            strategy: None,
                            runtime: None,
                            steps: vec![],
                            env: Default::default(),
                            branch_name: None,
                            pull_request: None,
                        },
                    ],
                },
                status: WorkflowStatus::AwaitingTrigger,
                params: Default::default(),
                bundle_path: None,
                tasks: vec![],
                started_at: Utc::now(),
                ended_at: None,
                capabilities: None,
                name: None,
                target_path: None,
            }),
            tasks: vec![
                Task {
                    id: install_skill_task_id,
                    workflow_run_id: run_id,
                    node_id: "node2".to_string(),
                    status: TaskStatus::AwaitingTrigger,
                    started_at: None,
                    ended_at: None,
                    logs: vec![],
                    master_task_id: None,
                    matrix_values: None,
                    is_master: false,
                    error: None,
                },
                Task {
                    id: normal_task_id,
                    workflow_run_id: run_id,
                    node_id: "apply-transforms".to_string(),
                    status: TaskStatus::AwaitingTrigger,
                    started_at: None,
                    ended_at: None,
                    logs: vec![],
                    master_task_id: None,
                    matrix_values: None,
                    is_master: false,
                    error: None,
                },
            ],
            ..TuiState::default()
        };

        match state.selected_task_trigger_command() {
            Some(WorkflowCommand::TriggerTask { task_id }) => {
                assert_eq!(task_id, install_skill_task_id);
            }
            other => panic!("expected install-skill trigger command, got {other:?}"),
        }
        assert_eq!(state.visible_awaiting_task_ids(), vec![normal_task_id]);
    }

    #[test]
    fn task_display_name_prefers_workflow_node_name() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![butterflow_models::Node {
                    id: "node".to_string(),
                    name: "Apply migration".to_string(),
                    description: None,
                    r#type: NodeType::Automatic,
                    depends_on: vec![],
                    trigger: None,
                    strategy: None,
                    runtime: None,
                    steps: vec![],
                    env: Default::default(),
                    branch_name: None,
                    pull_request: None,
                }],
            },
            status: WorkflowStatus::Running,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Pending,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        };

        assert_eq!(state.task_display_name(&task), "Apply migration");
    }

    #[test]
    fn task_display_name_appends_matrix_name_label() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![butterflow_models::Node {
                    id: "node".to_string(),
                    name: "Debarrel".to_string(),
                    description: None,
                    r#type: NodeType::Automatic,
                    depends_on: vec![],
                    trigger: None,
                    strategy: None,
                    runtime: None,
                    steps: vec![],
                    env: Default::default(),
                    branch_name: None,
                    pull_request: None,
                }],
            },
            status: WorkflowStatus::Running,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Pending,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: Some(std::collections::HashMap::from([(
                "name".to_string(),
                serde_json::json!("unowned-10"),
            )])),
            is_master: false,
            error: None,
        };

        assert_eq!(state.task_display_name(&task), "Debarrel · unowned-10");
    }

    #[test]
    fn task_display_name_falls_back_to_matrix_scalar_summary() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.current_run = Some(WorkflowRun {
            id: run_id,
            workflow: Workflow {
                version: "1".to_string(),
                state: None,
                params: None,
                templates: vec![],
                nodes: vec![butterflow_models::Node {
                    id: "node".to_string(),
                    name: "Run codemod".to_string(),
                    description: None,
                    r#type: NodeType::Automatic,
                    depends_on: vec![],
                    trigger: None,
                    strategy: None,
                    runtime: None,
                    steps: vec![],
                    env: Default::default(),
                    branch_name: None,
                    pull_request: None,
                }],
            },
            status: WorkflowStatus::Running,
            params: Default::default(),
            bundle_path: None,
            tasks: vec![],
            started_at: Utc::now(),
            ended_at: None,
            capabilities: None,
            name: None,
            target_path: None,
        });
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Pending,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: Some(std::collections::HashMap::from([
                ("team".to_string(), serde_json::json!("frontend")),
                ("kind".to_string(), serde_json::json!("ts")),
                ("_meta_shard".to_string(), serde_json::json!(3)),
            ])),
            is_master: false,
            error: None,
        };

        assert_eq!(
            state.task_display_name(&task),
            "Run codemod · kind=ts, team=frontend"
        );
    }

    #[test]
    fn task_elapsed_text_is_dash_when_task_has_not_started() {
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: Uuid::new_v4(),
            node_id: "node".to_string(),
            status: TaskStatus::Pending,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        };

        assert_eq!(TuiState::default().task_elapsed_text(&task), "-");
    }

    #[test]
    fn selected_task_completion_detail_shows_branch_and_pr_when_pr_exists() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Completed,
            started_at: Some(Utc::now()),
            ended_at: Some(Utc::now()),
            logs: vec![
                "Preparing git worktree for branch codemod-1234 in /tmp/repo".to_string(),
                "Pull request created: https://github.com/example/repo/pull/42".to_string(),
            ],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        assert_eq!(
            state.selected_task_completion_detail().as_deref(),
            Some("Branch: codemod-1234  PR: https://github.com/example/repo/pull/42")
        );
    }

    #[test]
    fn selected_task_completion_detail_hides_branch_when_no_pr_exists() {
        let run_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id: run_id,
            node_id: "node".to_string(),
            status: TaskStatus::Completed,
            started_at: Some(Utc::now()),
            ended_at: Some(Utc::now()),
            logs: vec!["Preparing git worktree for branch codemod-1234 in /tmp/repo".to_string()],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });

        assert_eq!(state.selected_task_completion_detail(), None);
    }

    #[test]
    fn selection_prompt_approval_accepts_selected_value() {
        let request_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.reduce(AppEvent::Workflow(WorkflowEvent::SelectionRequested {
            request_id,
            prompt: butterflow_core::config::SelectionPrompt {
                title: "Choose install scope".to_string(),
                options: vec![
                    butterflow_core::config::SelectionPromptOption {
                        value: "project".to_string(),
                        label: "project".to_string(),
                    },
                    butterflow_core::config::SelectionPromptOption {
                        value: "user".to_string(),
                        label: "user".to_string(),
                    },
                ],
                default_index: 1,
            },
            at: Utc::now(),
        }));

        assert!(matches!(
            state.approval_accept_command(),
            Some(WorkflowCommand::RespondSelection {
                request_id: actual_request_id,
                selection: Some(selection),
            }) if actual_request_id == request_id && selection == "user"
        ));
    }

    #[test]
    fn selection_prompt_reject_defers_selected_task() {
        let workflow_run_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();
        let mut state = TuiState::default();
        state.tasks.push(Task {
            id: Uuid::new_v4(),
            workflow_run_id,
            node_id: "install-skill".to_string(),
            status: TaskStatus::Running,
            started_at: Some(Utc::now()),
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        });
        state.reduce(AppEvent::Workflow(WorkflowEvent::SelectionRequested {
            request_id,
            prompt: butterflow_core::config::SelectionPrompt {
                title: "Choose harness".to_string(),
                options: vec![butterflow_core::config::SelectionPromptOption {
                    value: "claude".to_string(),
                    label: "Claude".to_string(),
                }],
                default_index: 0,
            },
            at: Utc::now(),
        }));

        assert!(matches!(
            state.approval_reject_command(),
            Some(WorkflowCommand::RespondSelection {
                request_id: actual_request_id,
                selection: None,
            }) if actual_request_id == request_id
        ));
    }

    #[test]
    fn task_elapsed_text_is_dash_without_started_at() {
        let state = TuiState::default();
        let task = Task {
            id: Uuid::new_v4(),
            workflow_run_id: Uuid::new_v4(),
            node_id: "install-skill".to_string(),
            status: TaskStatus::AwaitingTrigger,
            started_at: None,
            ended_at: None,
            logs: vec![],
            master_task_id: None,
            matrix_values: None,
            is_master: false,
            error: None,
        };

        assert_eq!(state.task_elapsed_text(&task), "-");
    }
}
