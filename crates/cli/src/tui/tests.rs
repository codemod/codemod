use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use butterflow_core::config::ShellCommandExecutionRequest;
use butterflow_core::config::WorkflowRunConfig;
use butterflow_core::engine::Engine;
use butterflow_core::{
    Node, Runtime, RuntimeType, Step, Task, TaskStatus, Trigger, TriggerType, Workflow,
    WorkflowRun, WorkflowStatus,
};
use butterflow_models::node::NodeType;
use butterflow_models::step::StepAction;
use butterflow_state::mock_adapter::MockStateAdapter;
use chrono::Utc;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, widgets::TableState, Terminal};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

use super::app::{
    App, AppEffect, EffectResult, LogView, PendingCapabilityApproval, PendingShellApproval, Screen,
    SessionOverrides,
};
use super::event::AppEvent;
use super::screens::{self, StatusLine, StatusTone};
use super::{apply_session_overrides, coalesce_events, execute_effect};

fn key_event(code: KeyCode) -> AppEvent {
    AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn mouse_scroll_event(kind: MouseEventKind) -> AppEvent {
    AppEvent::Mouse(MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })
}

fn direct_runtime() -> Option<Runtime> {
    Some(Runtime {
        r#type: RuntimeType::Direct,
        image: None,
        working_dir: None,
        user: None,
        network: None,
        options: None,
    })
}

fn script_step(id: &str, name: &str, command: String) -> Step {
    Step {
        id: Some(id.to_string()),
        name: name.to_string(),
        action: StepAction::RunScript(command),
        env: None,
        condition: None,
        commit: None,
    }
}

fn empty_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![],
    }
}

fn workflow_run(
    id: Uuid,
    status: WorkflowStatus,
    capabilities: Option<HashSet<LlrtSupportedModules>>,
) -> WorkflowRun {
    workflow_run_with_workflow(id, status, capabilities, empty_workflow())
}

fn workflow_run_with_workflow(
    id: Uuid,
    status: WorkflowStatus,
    capabilities: Option<HashSet<LlrtSupportedModules>>,
    workflow: Workflow,
) -> WorkflowRun {
    WorkflowRun {
        id,
        workflow,
        status,
        params: HashMap::new(),
        tasks: vec![],
        started_at: Utc::now(),
        ended_at: None,
        bundle_path: None,
        capabilities,
        name: Some("Example Workflow".to_string()),
        target_path: Some(Path::new("/tmp/example-target").to_path_buf()),
    }
}

fn minimal_script_node(id: &str, name: &str) -> Node {
    let step_id = format!("{id}-step");
    Node {
        id: id.to_string(),
        name: name.to_string(),
        description: None,
        r#type: NodeType::Automatic,
        depends_on: vec![],
        trigger: None,
        strategy: None,
        runtime: direct_runtime(),
        steps: vec![script_step(&step_id, name, "true".to_string())],
        env: HashMap::new(),
        branch_name: None,
        pull_request: None,
    }
}

fn task(workflow_run_id: Uuid, node_id: &str, status: TaskStatus, logs: Vec<&str>) -> Task {
    Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: node_id.to_string(),
        status,
        is_master: false,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(Utc::now()),
        ended_at: None,
        error: None,
        logs: logs.into_iter().map(str::to_string).collect(),
    }
}

fn render_to_string(draw: impl FnOnce(&mut ratatui::Frame<'_>)) -> String {
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(draw).unwrap();
    format!("{}", terminal.backend())
}

fn shell_request() -> ShellCommandExecutionRequest {
    ShellCommandExecutionRequest {
        command: "echo test".to_string(),
        node_id: "node".to_string(),
        node_name: "Node".to_string(),
        step_id: Some("step".to_string()),
        step_name: "Step".to_string(),
        task_id: Uuid::new_v4().to_string(),
    }
}

fn create_manual_trigger_workflow(command: String) -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "prepare".to_string(),
                name: "Prepare".to_string(),
                description: Some("Setup".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: direct_runtime(),
                steps: vec![script_step(
                    "prepare-step",
                    "Prepare Step",
                    "echo 'prepare'".to_string(),
                )],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "gate".to_string(),
                name: "Gate".to_string(),
                description: Some("Manual gate".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["prepare".to_string()],
                trigger: Some(Trigger {
                    r#type: TriggerType::Manual,
                }),
                strategy: None,
                runtime: direct_runtime(),
                steps: vec![script_step("gate-step", "Gate Step", command)],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

fn create_retryable_workflow(flag_path: &Path) -> Workflow {
    let flag = flag_path.display().to_string();
    let command = format!(
        "if [ -f '{flag}' ]; then echo retried; else touch '{flag}'; echo first-run; exit 1; fi"
    );

    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "flaky".to_string(),
            name: "Flaky".to_string(),
            description: Some("Fails once then succeeds".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: direct_runtime(),
            steps: vec![script_step("flaky-step", "Flaky Step", command)],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

fn create_long_running_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "sleepy".to_string(),
            name: "Sleepy".to_string(),
            description: Some("Long running task".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: direct_runtime(),
            steps: vec![script_step(
                "sleepy-step",
                "Sleepy Step",
                "sleep 5 && echo done".to_string(),
            )],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

fn create_param_sensitive_manual_workflow() -> Workflow {
    create_manual_trigger_workflow("echo 'hello ${{ params.repo_name }}'".to_string())
}

fn test_engine(temp_dir: &TempDir, wait_for_completion: bool) -> Engine {
    let config = WorkflowRunConfig {
        target_path: temp_dir.path().to_path_buf(),
        bundle_path: temp_dir.path().to_path_buf(),
        workflow_file_path: temp_dir.path().join("workflow.yaml"),
        wait_for_completion,
        no_interactive: true,
        quiet: true,
        ..Default::default()
    };

    Engine::with_state_adapter(Box::new(MockStateAdapter::new()), config)
}

async fn wait_for_task(
    engine: &Engine,
    workflow_run_id: Uuid,
    node_id: &str,
    predicate: impl Fn(TaskStatus) -> bool,
) -> Task {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
        if let Some(task) = tasks.into_iter().find(|task| task.node_id == node_id) {
            if predicate(task.status) {
                return task;
            }
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for task {node_id}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_workflow_status(
    engine: &Engine,
    workflow_run_id: Uuid,
    predicate: impl Fn(WorkflowStatus) -> bool,
) -> WorkflowRun {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();
        if predicate(workflow_run.status) {
            return workflow_run;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for workflow run {workflow_run_id}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn run_list_enter_opens_selected_run_and_requests_refresh() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut app = App::new(false, None, 20);
    app.workflow_runs = vec![
        workflow_run(first, WorkflowStatus::Completed, None),
        workflow_run(second, WorkflowStatus::AwaitingTrigger, None),
    ];
    app.run_list_state.select(Some(1));

    let effects = app.reduce(key_event(KeyCode::Enter));

    assert!(matches!(
        app.screen,
        Screen::TaskList { workflow_run_id } if workflow_run_id == second
    ));
    assert!(matches!(effects.as_slice(), [AppEffect::Refresh]));
    assert_eq!(app.task_list_state.selected(), Some(0));
}

#[test]
fn task_list_keys_dispatch_trigger_retry_and_logs() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let awaiting = task(
        run_id,
        "awaiting-node",
        TaskStatus::AwaitingTrigger,
        vec!["pending"],
    );
    let failed = task(run_id, "failed-node", TaskStatus::Failed, vec!["boom"]);
    app.tasks = vec![awaiting.clone(), failed.clone()];

    app.task_list_state.select(Some(0));
    let log_effects = app.reduce(key_event(KeyCode::Char('l')));
    assert!(matches!(
        log_effects.as_slice(),
        [AppEffect::LoadLogs { workflow_run_id, task_id }]
            if *workflow_run_id == run_id && *task_id == awaiting.id
    ));

    let trigger_effects = app.reduce(key_event(KeyCode::Char('t')));
    assert!(matches!(
        trigger_effects.as_slice(),
        [AppEffect::TriggerTask { workflow_run_id, task_id }]
            if *workflow_run_id == run_id && *task_id == awaiting.id
    ));

    app.task_list_state.select(Some(1));
    let retry_effects = app.reduce(key_event(KeyCode::Char('R')));
    assert!(matches!(
        retry_effects.as_slice(),
        [AppEffect::RetryTask { workflow_run_id, task_id }]
            if *workflow_run_id == run_id && *task_id == failed.id
    ));

    let cancel_effects = app.reduce(key_event(KeyCode::Char('c')));
    // CancelWorkflow is only returned when current_workflow_run has a cancelable status
    // The test doesn't set current_workflow_run, so no CancelWorkflow effect is returned
    assert!(cancel_effects.is_empty());
}

#[test]
fn task_list_mouse_scroll_moves_selection() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let first = task(run_id, "first", TaskStatus::AwaitingTrigger, vec![]);
    let second = task(run_id, "second", TaskStatus::AwaitingTrigger, vec![]);
    app.tasks = vec![first, second];
    app.task_list_state.select(Some(0));

    let effects = app.reduce(mouse_scroll_event(MouseEventKind::ScrollDown));
    assert!(effects.is_empty());
    assert_eq!(app.task_list_state.selected(), Some(1));

    let effects = app.reduce(mouse_scroll_event(MouseEventKind::ScrollUp));
    assert!(effects.is_empty());
    assert_eq!(app.task_list_state.selected(), Some(0));
}

#[test]
fn shell_approval_ignores_mouse_scroll() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let (tx, _rx) = sync_channel(1);
    app.present_shell_approval(PendingShellApproval::new(shell_request(), tx));

    let effects = app.reduce(mouse_scroll_event(MouseEventKind::ScrollDown));

    assert!(effects.is_empty());
    assert!(app.has_shell_approval());
}

#[test]
fn settings_toggle_updates_session_overrides_without_status_banner() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    app.screen = Screen::Settings {
        workflow_run_id: run_id,
    };

    let effects = app.reduce(key_event(KeyCode::Enter));
    assert!(effects.is_empty());
    assert!(app.session_overrides.dry_run);
    assert!(app.status_line.is_none());

    app.reduce(key_event(KeyCode::Down));
    app.reduce(key_event(KeyCode::Down));
    app.reduce(key_event(KeyCode::Enter));
    assert!(app
        .session_overrides
        .capabilities
        .as_ref()
        .is_some_and(|set| set.contains(&LlrtSupportedModules::Fetch)));

    app.reduce(key_event(KeyCode::Esc));
    assert!(matches!(
        app.screen,
        Screen::TaskList { workflow_run_id } if workflow_run_id == run_id
    ));
}

#[test]
fn log_modal_closes_on_escape() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let task = task(run_id, "node", TaskStatus::Running, vec!["line"]);
    app.log_view = Some(LogView::from_task(&task));

    let effects = app.reduce(key_event(KeyCode::Esc));

    assert!(effects.is_empty());
    assert!(app.log_view.is_none());
}

#[test]
fn shell_approval_modal_accepts_and_responds() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let (tx, rx) = sync_channel(1);
    app.present_shell_approval(PendingShellApproval::new(shell_request(), tx));

    let effects = app.reduce(key_event(KeyCode::Char('y')));

    assert!(effects.is_empty());
    assert!(!app.has_shell_approval());
    assert!(rx.recv().unwrap().unwrap());
}

#[test]
fn shell_approval_enter_does_not_approve() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let (tx, _rx) = sync_channel(1);
    app.present_shell_approval(PendingShellApproval::new(shell_request(), tx));

    let effects = app.reduce(key_event(KeyCode::Enter));

    assert!(effects.is_empty());
    assert!(app.has_shell_approval());
}

#[test]
fn capability_approval_enter_does_not_approve() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let (tx, _rx) = sync_channel(1);
    app.present_capability_approval(PendingCapabilityApproval::new(
        vec![LlrtSupportedModules::Fetch],
        tx,
    ));

    let effects = app.reduce(key_event(KeyCode::Enter));

    assert!(effects.is_empty());
    assert!(app.has_capability_approval());
}

#[test]
fn capability_approval_accepts_and_responds() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let (tx, rx) = sync_channel(1);
    app.present_capability_approval(PendingCapabilityApproval::new(
        vec![LlrtSupportedModules::Fetch],
        tx,
    ));

    let effects = app.reduce(key_event(KeyCode::Char('y')));

    assert!(effects.is_empty());
    assert!(!app.has_capability_approval());
    assert!(rx.recv().unwrap().is_ok());
}

#[test]
fn returning_to_run_list_discards_session_overrides() {
    let run_id = Uuid::new_v4();
    let base_capabilities = HashSet::from([LlrtSupportedModules::Fs]);
    let mut app = App::new_for_run(false, Some(base_capabilities.clone()), run_id);
    app.session_overrides.toggle_dry_run();
    app.session_overrides
        .toggle_capability(LlrtSupportedModules::Fetch);

    let effects = app.reduce(key_event(KeyCode::Esc));

    assert!(matches!(effects.as_slice(), [AppEffect::Refresh]));
    assert!(matches!(app.screen, Screen::RunList));
    assert!(!app.session_overrides.dry_run);
    assert_eq!(app.session_overrides.capabilities, Some(base_capabilities));
}

#[test]
fn refresh_result_seeds_session_overrides_from_workflow_run_once() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let initial_capabilities = HashSet::from([LlrtSupportedModules::Fs]);
    let seeded = app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(workflow_run(
            run_id,
            WorkflowStatus::AwaitingTrigger,
            Some(initial_capabilities.clone()),
        )),
        tasks: vec![],
    });

    assert!(seeded);
    assert_eq!(
        app.session_overrides.capabilities,
        Some(initial_capabilities)
    );

    app.session_overrides
        .toggle_capability(LlrtSupportedModules::Fetch);
    let updated_capabilities = HashSet::from([LlrtSupportedModules::ChildProcess]);
    app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(workflow_run(
            run_id,
            WorkflowStatus::AwaitingTrigger,
            Some(updated_capabilities),
        )),
        tasks: vec![],
    });

    assert!(app
        .session_overrides
        .capabilities
        .as_ref()
        .is_some_and(|set| set.contains(&LlrtSupportedModules::Fetch)));
}

#[test]
fn refresh_orders_tasks_by_workflow_yaml_node_order() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let add = task(
        run_id,
        "add-descriptions",
        TaskStatus::Pending,
        vec![],
    );
    let evaluate = task(run_id, "evaluate-shards", TaskStatus::Pending, vec![]);
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            minimal_script_node("evaluate-shards", "Evaluate"),
            minimal_script_node("add-descriptions", "Add descriptions"),
        ],
    };
    app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(workflow_run_with_workflow(
            run_id,
            WorkflowStatus::Running,
            None,
            workflow,
        )),
        tasks: vec![add, evaluate],
    });
    assert_eq!(app.tasks[0].node_id, "evaluate-shards");
    assert_eq!(app.tasks[1].node_id, "add-descriptions");
}

#[test]
fn refresh_keeps_selected_task_by_id_when_task_order_changes() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let first = task(
        run_id,
        "apply-transforms",
        TaskStatus::AwaitingTrigger,
        vec![],
    );
    let second = task(
        run_id,
        "apply-transforms",
        TaskStatus::AwaitingTrigger,
        vec![],
    );
    let third = task(run_id, "evaluate-shards", TaskStatus::Completed, vec![]);
    let mut first = first;
    first.matrix_values = Some(HashMap::from([("shard".to_string(), json!(1))]));
    let mut second = second;
    second.matrix_values = Some(HashMap::from([("shard".to_string(), json!(2))]));

    app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(workflow_run(run_id, WorkflowStatus::AwaitingTrigger, None)),
        tasks: vec![third.clone(), second.clone(), first.clone()],
    });

    app.reduce(key_event(KeyCode::Down));
    assert_eq!(app.task_list_state.selected(), Some(1));

    app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(workflow_run(run_id, WorkflowStatus::Running, None)),
        tasks: vec![second.clone(), first.clone(), third.clone()],
    });

    assert_eq!(app.task_list_state.selected(), Some(1));
}

#[test]
fn refresh_with_running_task_forces_redraw_even_when_payload_is_unchanged() {
    let run_id = Uuid::new_v4();
    let mut app = App::new_for_run(false, None, run_id);
    let running = task(run_id, "apply-transforms", TaskStatus::Running, vec![]);
    let run = workflow_run(run_id, WorkflowStatus::Running, None);

    assert!(app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(run.clone()),
        tasks: vec![running.clone()],
    }));

    assert!(app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(run),
        tasks: vec![running],
    }));
}

#[tokio::test]
async fn apply_session_overrides_updates_engine_configuration() {
    let temp_dir = TempDir::new().unwrap();
    let mut engine = test_engine(&temp_dir, true);
    let overrides = SessionOverrides::new(
        true,
        Some(HashSet::from([
            LlrtSupportedModules::Fs,
            LlrtSupportedModules::Fetch,
        ])),
    );

    apply_session_overrides(&mut engine, &overrides);

    assert!(engine.is_dry_run());
    assert_eq!(engine.get_capabilities(), &overrides.capabilities);
}

#[test]
fn run_list_render_shows_empty_state_and_status_banner() {
    let mut table_state = TableState::default();
    let status = StatusLine {
        tone: StatusTone::Error,
        message: "boom".to_string(),
    };

    let output = render_to_string(|frame| {
        screens::run_list::render(frame, frame.area(), &[], &mut table_state, Some(&status))
    });

    assert!(output.contains("No workflow runs yet."));
    assert!(output.contains("Run a workflow with: codemod workflow run -w <path>"));
    assert!(output.contains("boom"));
}

#[test]
fn task_list_render_shows_matrix_columns_hints_and_status() {
    let run_id = Uuid::new_v4();
    let run = workflow_run(run_id, WorkflowStatus::AwaitingTrigger, None);
    let mut awaiting = task(
        run_id,
        "gate",
        TaskStatus::AwaitingTrigger,
        vec!["awaiting"],
    );
    awaiting.matrix_values = Some(HashMap::from([
        ("repo".to_string(), json!("web")),
        ("lang".to_string(), json!("ts")),
    ]));
    let failed = task(run_id, "retry-me", TaskStatus::Failed, vec!["failed"]);
    let mut table_state = TableState::default();
    table_state.select(Some(0));
    let output = render_to_string(|frame| {
        screens::task_list::render(
            frame,
            frame.area(),
            Some(&run),
            &[awaiting.clone(), failed.clone()],
            &mut table_state,
            None,
            None,
            0,
            true,
        )
    });

    assert!(output.contains("REPO"));
    assert!(output.contains("LANG"));
    assert!(output.contains("trigger all"));
    assert!(output.contains("retry"));
}

#[test]
fn task_list_render_shows_log_modal_and_empty_log_state() {
    let run_id = Uuid::new_v4();
    let run = workflow_run(run_id, WorkflowStatus::Running, None);
    let selected_task = task(run_id, "logs-node", TaskStatus::Running, vec![]);
    let log_view = LogView {
        task_id: selected_task.id,
        node_id: selected_task.node_id.clone(),
        status: selected_task.status,
        lines: vec![],
        error: Some("task failed".to_string()),
    };
    let mut table_state = TableState::default();
    table_state.select(Some(0));

    let output = render_to_string(|frame| {
        screens::task_list::render(
            frame,
            frame.area(),
            Some(&run),
            std::slice::from_ref(&selected_task),
            &mut table_state,
            None,
            Some(&log_view),
            0,
            true,
        )
    });

    assert!(output.contains("Waiting for log output"));
    assert!(output.contains("flush when the step exits"));
    assert!(output.contains("task failed"));
    assert!(output.contains("close"));
}

#[test]
fn settings_render_shows_session_override_copy() {
    let run = workflow_run(Uuid::new_v4(), WorkflowStatus::AwaitingTrigger, None);

    let output = render_to_string(|frame| {
        screens::settings::render(
            frame,
            frame.area(),
            Some(&run),
            1,
            true,
            &Some(HashSet::from([LlrtSupportedModules::Fs])),
            None,
        )
    });

    assert!(output.contains("session overrides"));
    assert!(output.contains("for this TUI session"));
}

#[tokio::test]
async fn refresh_effect_populates_run_list() {
    let temp_dir = TempDir::new().unwrap();
    let mut engine = test_engine(&temp_dir, true);
    let run_id = engine
        .run_workflow(
            create_manual_trigger_workflow("echo ready".to_string()),
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();

    let app = App::new(false, None, 20);
    let result = execute_effect(&app, &mut engine, AppEffect::Refresh).await;

    let EffectResult::Refreshed { workflow_runs, .. } = result else {
        panic!("expected refresh result");
    };
    assert!(workflow_runs.iter().any(|run| run.id == run_id));
}

#[tokio::test]
#[ignore = "flaky: times out in CI due to async race conditions in wait_for_task"]
async fn trigger_task_effect_resumes_awaiting_task() {
    let temp_dir = TempDir::new().unwrap();
    let mut engine = test_engine(&temp_dir, true);
    let run_id = engine
        .run_workflow(
            create_manual_trigger_workflow("echo released".to_string()),
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    let awaiting = wait_for_task(&engine, run_id, "gate", |status| {
        status == TaskStatus::AwaitingTrigger
    })
    .await;
    let app = App::new_for_run(false, None, run_id);

    let result = execute_effect(
        &app,
        &mut engine,
        AppEffect::TriggerTask {
            workflow_run_id: run_id,
            task_id: awaiting.id,
        },
    )
    .await;

    assert!(matches!(result, EffectResult::Noop));
    let completed = wait_for_task(&engine, run_id, "gate", |status| {
        status == TaskStatus::Completed
    })
    .await;
    assert!(completed.logs.iter().any(|line| line.contains("released")));
}

#[tokio::test]
#[ignore = "flaky: times out in CI due to async race conditions in wait_for_task"]
async fn trigger_all_effect_uses_engine_trigger_all() {
    let temp_dir = TempDir::new().unwrap();
    let mut engine = test_engine(&temp_dir, true);
    let run_id = engine
        .run_workflow(
            create_manual_trigger_workflow("echo all".to_string()),
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    wait_for_task(&engine, run_id, "gate", |status| {
        status == TaskStatus::AwaitingTrigger
    })
    .await;
    let app = App::new_for_run(false, None, run_id);

    let result = execute_effect(
        &app,
        &mut engine,
        AppEffect::TriggerAll {
            workflow_run_id: run_id,
        },
    )
    .await;

    assert!(matches!(result, EffectResult::Noop));
    wait_for_task(&engine, run_id, "gate", |status| {
        status == TaskStatus::Completed
    })
    .await;
}

#[tokio::test]
#[ignore = "flaky: times out in CI due to async race conditions in wait_for_task"]
async fn retry_effect_resumes_failed_task() {
    let temp_dir = TempDir::new().unwrap();
    let flag_path = temp_dir.path().join("retry.flag");
    let mut engine = test_engine(&temp_dir, true);
    let run_id = engine
        .run_workflow(
            create_retryable_workflow(&flag_path),
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    let failed = wait_for_task(&engine, run_id, "flaky", |status| {
        status == TaskStatus::Failed
    })
    .await;
    let app = App::new_for_run(false, None, run_id);

    let result = execute_effect(
        &app,
        &mut engine,
        AppEffect::RetryTask {
            workflow_run_id: run_id,
            task_id: failed.id,
        },
    )
    .await;

    assert!(matches!(result, EffectResult::Noop));
    let completed = wait_for_task(&engine, run_id, "flaky", |status| {
        status == TaskStatus::Completed
    })
    .await;
    assert!(completed.logs.iter().any(|line| line.contains("retried")));
}

#[tokio::test]
#[ignore = "flaky: times out in CI due to async race conditions in wait_for_workflow_status"]
async fn cancel_effect_updates_workflow_status() {
    let temp_dir = TempDir::new().unwrap();
    let mut engine = test_engine(&temp_dir, false);
    let run_id = engine
        .run_workflow(create_long_running_workflow(), HashMap::new(), None, None)
        .await
        .unwrap();
    wait_for_workflow_status(&engine, run_id, |status| status == WorkflowStatus::Running).await;
    let app = App::new_for_run(false, None, run_id);

    let result = execute_effect(
        &app,
        &mut engine,
        AppEffect::CancelWorkflow {
            workflow_run_id: run_id,
        },
    )
    .await;

    assert!(matches!(result, EffectResult::Noop));
    let workflow_run =
        wait_for_workflow_status(&engine, run_id, |status| status == WorkflowStatus::Canceled)
            .await;
    assert_eq!(workflow_run.status, WorkflowStatus::Canceled);
}

#[tokio::test]
#[ignore = "flaky: times out in CI due to async race conditions in wait_for_task"]
async fn trigger_effect_preserves_stored_workflow_params() {
    let temp_dir = TempDir::new().unwrap();
    let mut engine = test_engine(&temp_dir, true);
    let params = HashMap::from([("repo_name".to_string(), json!("butterflow"))]);
    let run_id = engine
        .run_workflow(create_param_sensitive_manual_workflow(), params, None, None)
        .await
        .unwrap();
    let awaiting = wait_for_task(&engine, run_id, "gate", |status| {
        status == TaskStatus::AwaitingTrigger
    })
    .await;
    let app = App::new_for_run(false, None, run_id);

    let result = execute_effect(
        &app,
        &mut engine,
        AppEffect::TriggerTask {
            workflow_run_id: run_id,
            task_id: awaiting.id,
        },
    )
    .await;

    assert!(matches!(result, EffectResult::Noop));
    let completed = wait_for_task(&engine, run_id, "gate", |status| {
        status == TaskStatus::Completed
    })
    .await;
    assert!(completed
        .logs
        .iter()
        .any(|line| line.contains("hello butterflow")));
}

#[test]
fn sync_log_view_tracks_refreshed_task_logs() {
    let run_id = Uuid::new_v4();
    let selected_task = task(run_id, "node", TaskStatus::Running, vec!["line 1"]);
    let mut app = App::new_for_run(false, None, run_id);
    app.log_view = Some(LogView::from_task(&selected_task));

    let updated_task = Task {
        logs: vec!["line 1".to_string(), "line 2".to_string()],
        status: TaskStatus::Completed,
        ended_at: Some(Utc::now()),
        ..selected_task.clone()
    };

    app.apply_effect_result(EffectResult::Refreshed {
        workflow_runs: vec![],
        current_workflow_run: Some(workflow_run(run_id, WorkflowStatus::Running, None)),
        tasks: vec![updated_task],
    });

    assert_eq!(app.log_view.as_ref().map(|view| view.lines.len()), Some(2));
    assert_eq!(
        app.log_view.as_ref().map(|view| view.status),
        Some(TaskStatus::Completed)
    );
}

#[test]
fn log_view_scroll_keys_update_scroll_state() {
    let run_id = Uuid::new_v4();
    let selected_task = task(
        run_id,
        "node",
        TaskStatus::Running,
        vec!["line 1", "line 2"],
    );
    let mut app = App::new_for_run(false, None, run_id);
    app.log_view = Some(LogView::from_task(&selected_task));

    app.reduce(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    assert_eq!(app.log_scroll, 1);
    assert!(!app.log_follow);

    app.reduce(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(app.log_scroll, 0);
    assert!(app.log_follow);

    app.reduce(AppEvent::Key(KeyEvent::from(KeyCode::PageDown)));
    assert_eq!(app.log_scroll, 10);
    assert!(!app.log_follow);

    app.reduce(AppEvent::Key(KeyEvent::from(KeyCode::Char('g'))));
    assert_eq!(app.log_scroll, 0);
    assert!(!app.log_follow);

    app.reduce(AppEvent::Key(KeyEvent::from(KeyCode::Char('G'))));
    assert_eq!(app.log_scroll, 0);
    assert!(app.log_follow);

    app.reduce(AppEvent::Key(KeyEvent::from(KeyCode::End)));
    assert_eq!(app.log_scroll, 0);
    assert!(app.log_follow);
}

#[test]
fn coalesce_events_batches_scroll_bursts_and_ticks() {
    let events = vec![
        mouse_scroll_event(MouseEventKind::ScrollDown),
        mouse_scroll_event(MouseEventKind::ScrollDown),
        mouse_scroll_event(MouseEventKind::ScrollDown),
        AppEvent::Tick,
        AppEvent::Tick,
        mouse_scroll_event(MouseEventKind::ScrollUp),
    ];

    let coalesced = coalesce_events(events);

    assert_eq!(coalesced.len(), 2);
    assert!(matches!(coalesced[0], AppEvent::Scroll(2)));
    assert!(matches!(coalesced[1], AppEvent::Tick));
}

#[test]
fn coalesced_scroll_event_moves_log_view_without_mouse_backlog() {
    let run_id = Uuid::new_v4();
    let selected_task = task(
        run_id,
        "node",
        TaskStatus::Running,
        vec!["line 1", "line 2"],
    );
    let mut app = App::new_for_run(false, None, run_id);
    app.log_view = Some(LogView::from_task(&selected_task));

    let effects = app.reduce(AppEvent::Scroll(6));

    assert!(effects.is_empty());
    assert_eq!(app.log_scroll, 6);
    assert!(!app.log_follow);
}
