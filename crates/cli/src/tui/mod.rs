pub mod app;
pub mod event;
mod screens;

use std::io;
use std::time::Duration;

use anyhow::Result;
use arboard::Clipboard;
use butterflow_core::engine::Engine;
use butterflow_core::execution::ProgressCallback;
use butterflow_core::workflow_runtime::{
    publish_event, WorkflowCommand, WorkflowEvent, WorkflowSession, WorkflowSnapshot,
};
use crossterm::event::{poll, read, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::tui::app::{ApprovalPrompt, Screen, TuiState};
use crate::tui::event::AppEvent;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        io::stdout().execute(EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn log_modal_viewport_height(terminal_height: u16) -> u16 {
    terminal_height
        .saturating_mul(3)
        .saturating_div(5)
        .saturating_sub(2)
}

fn task_list_viewport_height(terminal_height: u16) -> usize {
    terminal_height.saturating_sub(5) as usize
}

fn is_copy_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('c') | KeyCode::Char('C'))
        && (modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::SUPER))
}

fn copy_text_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}

pub(crate) fn create_tui_progress_callback(workflow_run_id: Uuid) -> ProgressCallback {
    ProgressCallback {
        callback: std::sync::Arc::new(Box::new(move |task_id, path, status, count, index| {
            let Ok(task_id) = Uuid::parse_str(task_id) else {
                return;
            };

            let current_file = match status {
                "processing" | "update" | "next" if !path.is_empty() => Some(path.to_string()),
                _ => None,
            };

            let processed_files = match status {
                "increment" | "finish" => *index,
                "start" | "counting" => 0,
                _ => *index,
            };

            publish_event(
                workflow_run_id,
                WorkflowEvent::TaskProgressUpdated {
                    workflow_run_id,
                    task_id,
                    processed_files,
                    total_files: count.cloned(),
                    current_file,
                    at: chrono::Utc::now(),
                },
            );
        })),
    }
}

pub async fn run_workflow_tui(
    mut engine: Engine,
    run_id: Option<Uuid>,
    limit: usize,
) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    engine.set_quiet(true);
    engine
        .workflow_run_config_mut()
        .capture_stdout_in_quiet_mode = false;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut state = TuiState::default();
    state.set_runs(engine.list_workflow_runs(limit).await.unwrap_or_default());

    let (_ui_event_tx, ui_event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut runtime = TuiRuntime::new(ui_event_rx);

    if let Some(run_id) = run_id {
        attach_run(&mut engine, run_id, &mut state, &mut runtime).await?;
    }

    run_tui_loop(&mut engine, &mut terminal, &mut state, limit, &mut runtime).await
}

pub async fn run_workflow_tui_with_session(
    mut engine: Engine,
    session: WorkflowSession,
    limit: usize,
) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    engine.set_quiet(true);
    engine
        .workflow_run_config_mut()
        .capture_stdout_in_quiet_mode = false;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut state = TuiState::default();
    state.set_runs(engine.list_workflow_runs(limit).await.unwrap_or_default());

    let (_ui_event_tx, ui_event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut runtime = TuiRuntime::new(ui_event_rx);
    let run_id = session.handle().workflow_run_id();
    engine.set_progress_callback(std::sync::Arc::new(Some(create_tui_progress_callback(
        run_id,
    ))));
    bind_session(&session, &mut state, &mut runtime).await?;
    runtime.session = Some(session);

    run_tui_loop(&mut engine, &mut terminal, &mut state, limit, &mut runtime).await
}

struct TuiRuntime {
    session: Option<WorkflowSession>,
    receiver: Option<tokio::sync::broadcast::Receiver<WorkflowEvent>>,
    snapshot_receiver: Option<mpsc::UnboundedReceiver<WorkflowSnapshot>>,
    snapshot_task: Option<tokio::task::JoinHandle<()>>,
    ui_event_rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl TuiRuntime {
    fn new(ui_event_rx: mpsc::UnboundedReceiver<AppEvent>) -> Self {
        Self {
            session: None,
            receiver: None,
            snapshot_receiver: None,
            snapshot_task: None,
            ui_event_rx,
        }
    }
}

async fn run_tui_loop(
    engine: &mut Engine,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
    limit: usize,
    runtime: &mut TuiRuntime,
) -> Result<()> {
    loop {
        if let Some(rx) = runtime.receiver.as_mut() {
            while let Ok(event) = rx.try_recv() {
                state.reduce(AppEvent::Workflow(event));
            }
        }

        if let Some(rx) = runtime.snapshot_receiver.as_mut() {
            while let Ok(snapshot) = rx.try_recv() {
                state.reduce(AppEvent::Snapshot(snapshot));
            }
        }

        while let Ok(event) = runtime.ui_event_rx.try_recv() {
            state.reduce(event);
        }

        state.clear_expired_log_modal_notice();

        if matches!(state.screen, Screen::RunDetail) {
            let viewport_height = task_list_viewport_height(terminal.size()?.height);
            state.sync_task_list_scroll(viewport_height);
        }

        terminal.draw(|frame| screens::render(frame, state))?;

        if !poll(Duration::from_millis(100))? {
            continue;
        }

        let Event::Key(key) = read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if state.approval.is_some() {
            match key.code {
                KeyCode::Char('y') => {
                    if let (Some(session), Some(command)) =
                        (runtime.session.as_ref(), state.approval_accept_command())
                    {
                        spawn_command(session.handle(), command);
                    }
                    state.clear_approval();
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    if let (Some(session), Some(command)) =
                        (runtime.session.as_ref(), state.approval_reject_command())
                    {
                        spawn_command(session.handle(), command);
                    }
                    state.clear_approval();
                }
                KeyCode::Up | KeyCode::Char('k') => state.move_up(),
                KeyCode::Down | KeyCode::Char('j') => state.move_down(),
                KeyCode::Enter => {
                    if matches!(state.approval, Some(ApprovalPrompt::AgentSelection { .. })) {
                        if let (Some(session), Some(command)) =
                            (runtime.session.as_ref(), state.approval_accept_command())
                        {
                            spawn_command(session.handle(), command);
                        }
                        state.clear_approval();
                    }
                }
                _ => {}
            }
            continue;
        }

        match state.screen {
            Screen::Runs => match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Up | KeyCode::Char('k') => state.move_up(),
                KeyCode::Down | KeyCode::Char('j') => state.move_down(),
                KeyCode::Char('r') => {
                    state.set_runs(engine.list_workflow_runs(limit).await.unwrap_or_default());
                }
                KeyCode::Enter => {
                    if let Some(run_id) = state.selected_run_id() {
                        attach_run(engine, run_id, state, runtime).await?;
                    }
                }
                _ => {}
            },
            Screen::RunDetail => match key.code {
                KeyCode::Char('c') | KeyCode::Char('C')
                    if is_copy_shortcut(key.code, key.modifiers) =>
                {
                    if state.show_log_modal {
                        match copy_text_to_clipboard(&state.selected_task_log_text()) {
                            Ok(()) => state.set_log_modal_notice("Copied full log to clipboard"),
                            Err(error) => state
                                .set_log_modal_notice(format!("Clipboard copy failed: {error}")),
                        }
                    } else if let Some(session) = runtime.session.as_ref() {
                        spawn_command(session.handle(), WorkflowCommand::CancelWorkflow);
                    }
                }
                KeyCode::Char('q') => {
                    if state.show_log_modal {
                        state.close_log_modal();
                    } else {
                        break;
                    }
                }
                KeyCode::Esc => {
                    if state.show_log_modal {
                        state.close_log_modal();
                        continue;
                    }
                    if let Some(task) = runtime.snapshot_task.take() {
                        task.abort();
                    }
                    runtime.receiver = None;
                    runtime.snapshot_receiver = None;
                    runtime.session = None;
                    state.leave_run();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if state.show_log_modal {
                        state.scroll_logs_up(1);
                    } else {
                        state.move_up();
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if state.show_log_modal {
                        let viewport_height = log_modal_viewport_height(terminal.size()?.height);
                        state.scroll_logs_down(viewport_height, 1);
                    } else {
                        state.move_down();
                    }
                }
                KeyCode::Enter => {
                    if state.show_log_modal {
                        state.close_log_modal();
                    } else {
                        let viewport_height = log_modal_viewport_height(terminal.size()?.height);
                        state.open_log_modal(viewport_height);
                    }
                }
                KeyCode::Char('g') => {
                    if state.show_log_modal {
                        state.scroll_logs_to_top();
                        continue;
                    }
                }
                KeyCode::Char('t') => {
                    if let (Some(session), Some(command)) = (
                        runtime.session.as_ref(),
                        state.selected_task_trigger_command(),
                    ) {
                        spawn_command(session.handle(), command);
                    }
                }
                KeyCode::Char('G') => {
                    if state.show_log_modal {
                        let viewport_height = log_modal_viewport_height(terminal.size()?.height);
                        state.scroll_logs_to_bottom(viewport_height);
                    }
                }
                KeyCode::Char('T') => {
                    if let Some(session) = runtime.session.as_ref() {
                        let task_ids = state.visible_awaiting_task_ids();
                        spawn_command(session.handle(), WorkflowCommand::TriggerTasks { task_ids });
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(session) = runtime.session.as_ref() {
                        spawn_command(session.handle(), WorkflowCommand::CancelWorkflow);
                    }
                }
                _ => {}
            },
        }
    }

    Ok(())
}

fn spawn_command(
    handle: butterflow_core::workflow_runtime::WorkflowSessionHandle,
    command: WorkflowCommand,
) {
    tokio::spawn(async move {
        let _ = handle.send(command).await;
    });
}

async fn attach_run(
    engine: &mut Engine,
    run_id: Uuid,
    state: &mut TuiState,
    runtime: &mut TuiRuntime,
) -> Result<()> {
    if let Some(task) = runtime.snapshot_task.take() {
        task.abort();
    }
    let session = WorkflowSession::attach(engine.clone(), run_id);
    engine.set_progress_callback(std::sync::Arc::new(Some(create_tui_progress_callback(
        run_id,
    ))));
    bind_session(&session, state, runtime).await?;
    runtime.session = Some(session);
    Ok(())
}

async fn bind_session(
    session: &WorkflowSession,
    state: &mut TuiState,
    runtime: &mut TuiRuntime,
) -> Result<()> {
    let snapshot = session.handle().load_snapshot().await?;
    let receiver = session.handle().subscribe();
    let session_handle = session.handle();
    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel();
    let snapshot_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match session_handle.load_snapshot().await {
                Ok(snapshot) => {
                    if snapshot_tx.send(snapshot).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    log::debug!("snapshot reconcile failed: {error}");
                }
            }
        }
    });
    state.enter_run(snapshot);
    runtime.receiver = Some(receiver);
    runtime.snapshot_receiver = Some(snapshot_rx);
    runtime.snapshot_task = Some(snapshot_task);
    Ok(())
}
