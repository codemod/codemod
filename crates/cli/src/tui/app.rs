use butterflow_core::workflow_runtime::{WorkflowCommand, WorkflowEvent, WorkflowSnapshot};
use butterflow_models::{Task, TaskStatus, WorkflowRun};
use uuid::Uuid;

use crate::tui::event::AppEvent;

#[derive(Clone, Debug)]
pub enum Screen {
    Runs,
    RunDetail,
}

#[derive(Clone, Debug)]
pub enum ApprovalPrompt {
    Shell { request_id: Uuid, command: String },
    Capabilities { request_id: Uuid, modules: Vec<String> },
    AgentSelection {
        request_id: Uuid,
        options: Vec<(String, bool)>,
        selected: usize,
    },
}

#[derive(Clone, Debug)]
pub struct StatusBanner {
    pub message: String,
    pub is_error: bool,
}

#[derive(Clone, Debug)]
pub struct TuiState {
    pub screen: Screen,
    pub runs: Vec<WorkflowRun>,
    pub selected_run: usize,
    pub current_run: Option<WorkflowRun>,
    pub tasks: Vec<Task>,
    pub selected_task: usize,
    pub approval: Option<ApprovalPrompt>,
    pub banner: Option<StatusBanner>,
    pub show_log_modal: bool,
    pub log_modal_scroll: u16,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            screen: Screen::Runs,
            runs: Vec::new(),
            selected_run: 0,
            current_run: None,
            tasks: Vec::new(),
            selected_task: 0,
            approval: None,
            banner: None,
            show_log_modal: false,
            log_modal_scroll: 0,
        }
    }
}

impl TuiState {
    fn is_terminal_task_status(status: TaskStatus) -> bool {
        matches!(
            status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::WontDo
        )
    }

    fn is_ignorable_pending_install_skill(task: &Task) -> bool {
        task.node_id == "install-skill" && task.status == TaskStatus::AwaitingTrigger
    }

    fn task_dependencies_satisfied(&self, task: &Task) -> bool {
        let Some(run) = self.current_run.as_ref() else {
            return true;
        };
        let Some(node) = run.workflow.nodes.iter().find(|node| node.id == task.node_id) else {
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
        self.tasks
            .iter()
            .filter(|task| !task.is_master)
            .collect()
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
                || Self::is_ignorable_pending_install_skill(task)
        })
    }

    pub fn display_run_status(&self) -> String {
        let Some(run) = self.current_run.as_ref() else {
            return "Unknown".to_string();
        };

        if self.is_effectively_complete()
            && matches!(run.status, butterflow_models::WorkflowStatus::AwaitingTrigger)
        {
            "Completed (install-skill pending)".to_string()
        } else {
            format!("{:?}", run.status)
        }
    }

    fn clamp_selected_task(&mut self) {
        let visible_len = self.visible_tasks().len();
        if self.selected_task >= visible_len {
            self.selected_task = visible_len.saturating_sub(1);
        }
    }

    pub fn set_runs(&mut self, runs: Vec<WorkflowRun>) {
        self.runs = runs;
        if self.selected_run >= self.runs.len() {
            self.selected_run = self.runs.len().saturating_sub(1);
        }
    }

    pub fn enter_run(&mut self, snapshot: WorkflowSnapshot) {
        self.screen = Screen::RunDetail;
        self.current_run = Some(snapshot.workflow_run);
        self.tasks = snapshot.tasks;
        self.selected_task = 0;
        self.approval = None;
        self.banner = None;
        self.show_log_modal = false;
        self.log_modal_scroll = 0;
    }

    pub fn reconcile_snapshot(&mut self, snapshot: WorkflowSnapshot) {
        let selected_task_id = self.selected_task().map(|task| task.id);
        self.current_run = Some(snapshot.workflow_run);
        self.tasks = snapshot.tasks;
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
        self.selected_task = 0;
        self.approval = None;
        self.banner = None;
        self.show_log_modal = false;
        self.log_modal_scroll = 0;
    }

    pub fn selected_run_id(&self) -> Option<Uuid> {
        self.runs.get(self.selected_run).map(|run| run.id)
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.visible_tasks().get(self.selected_task).copied()
    }

    pub fn selected_task_log_text(&self) -> String {
        self.selected_task()
            .map(|task| {
                if task.logs.is_empty() {
                    "No logs yet".to_string()
                } else {
                    task.logs.join("\n")
                }
            })
            .unwrap_or_else(|| "No task selected".to_string())
    }

    pub fn open_log_modal(&mut self, viewport_height: u16) {
        if self.selected_task().is_none() {
            return;
        }
        self.show_log_modal = true;
        self.scroll_logs_to_bottom(viewport_height);
    }

    pub fn close_log_modal(&mut self) {
        self.show_log_modal = false;
        self.log_modal_scroll = 0;
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
            Screen::RunDetail => {
                if let Some(ApprovalPrompt::AgentSelection { selected, .. }) = &mut self.approval {
                    *selected = selected.saturating_sub(1);
                } else {
                    self.selected_task = self.selected_task.saturating_sub(1);
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.screen {
            Screen::Runs => {
                if !self.runs.is_empty() {
                    self.selected_run = (self.selected_run + 1).min(self.runs.len() - 1);
                }
            }
            Screen::RunDetail => {
                if let Some(ApprovalPrompt::AgentSelection {
                    selected, options, ..
                }) = &mut self.approval
                {
                    if !options.is_empty() {
                        *selected = (*selected + 1).min(options.len() - 1);
                    }
                } else {
                    let visible_len = self.visible_tasks().len();
                    if visible_len > 0 {
                        self.selected_task = (self.selected_task + 1).min(visible_len - 1);
                    }
                }
            }
        }
    }

    pub fn reduce(&mut self, event: AppEvent) {
        match event {
            AppEvent::Workflow(workflow_event) => self.reduce_workflow_event(workflow_event),
            AppEvent::Snapshot(snapshot) => self.reconcile_snapshot(snapshot),
            AppEvent::Banner(banner) => self.banner = Some(banner),
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
                if let Some(existing) = self.tasks.iter_mut().find(|existing| existing.id == task.id)
                {
                    *existing = task;
                } else {
                    self.tasks.push(task);
                }
                self.clamp_selected_task();
            }
            WorkflowEvent::TaskUpdated { task, .. } => {
                if let Some(existing) = self.tasks.iter_mut().find(|existing| existing.id == task.id)
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
            WorkflowEvent::ShellApprovalRequested {
                request_id, request, ..
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
                    modules: modules.into_iter().map(|module| format!("{module:?}")).collect(),
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
                                    if option.is_available { "" } else { " (not installed)" }
                                ),
                                option.is_available,
                            )
                        })
                        .collect(),
                    selected: 0,
                });
            }
        }
    }

    pub fn approval_accept_command(&self) -> Option<WorkflowCommand> {
        match self.approval.as_ref()? {
            ApprovalPrompt::Shell { request_id, .. } => Some(WorkflowCommand::RespondShellApproval {
                request_id: *request_id,
                approved: true,
            }),
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
                            label.split(" (")
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
        }
    }

    pub fn approval_reject_command(&self) -> Option<WorkflowCommand> {
        match self.approval.as_ref()? {
            ApprovalPrompt::Shell { request_id, .. } => Some(WorkflowCommand::RespondShellApproval {
                request_id: *request_id,
                approved: false,
            }),
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
        }
    }

    pub fn clear_approval(&mut self) {
        self.approval = None;
    }

    pub fn selected_task_trigger_command(&self) -> Option<WorkflowCommand> {
        let task = self.selected_task()?;
        if task.status == TaskStatus::AwaitingTrigger && self.task_dependencies_satisfied(task) {
            Some(WorkflowCommand::TriggerTask { task_id: task.id })
        } else {
            None
        }
    }

    pub fn visible_awaiting_task_ids(&self) -> Vec<Uuid> {
        self.visible_tasks()
            .into_iter()
            .filter(|task| task.status == TaskStatus::AwaitingTrigger)
            .filter(|task| self.task_dependencies_satisfied(task))
            .map(|task| task.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use butterflow_core::workflow_runtime::WorkflowEvent;
    use butterflow_models::{Task, TaskStatus, Workflow, WorkflowRun, WorkflowStatus};
    use chrono::Utc;
    use uuid::Uuid;

    use super::{AppEvent, TuiState};

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
}
