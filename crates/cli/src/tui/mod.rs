pub mod app;
pub mod event;
mod screens;

use std::io;
use std::time::Duration;

use anyhow::Result;
use butterflow_core::engine::Engine;
use butterflow_core::workflow_runtime::{WorkflowCommand, WorkflowSession, WorkflowSnapshot};
use crossterm::event::{poll, read, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::tui::app::{ApprovalPrompt, Screen, StatusBanner, TuiState};
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
    terminal_height.saturating_mul(3).saturating_div(5).saturating_sub(2)
}

pub async fn run_workflow_tui(mut engine: Engine, run_id: Option<Uuid>, limit: usize) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    engine.set_quiet(true);
    engine.set_progress_callback(std::sync::Arc::new(None));
    engine.workflow_run_config_mut().capture_stdout_in_quiet_mode = false;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut state = TuiState::default();
    state.set_runs(engine.list_workflow_runs(limit).await.unwrap_or_default());

    let mut session: Option<WorkflowSession> = None;
    let mut receiver = None;
    let mut snapshot_receiver: Option<mpsc::UnboundedReceiver<WorkflowSnapshot>> = None;
    let mut snapshot_task: Option<tokio::task::JoinHandle<()>> = None;
    let (ui_event_tx, mut ui_event_rx) = mpsc::unbounded_channel::<AppEvent>();

    if let Some(run_id) = run_id {
        attach_run(
            &engine,
            run_id,
            &mut state,
            &mut session,
            &mut receiver,
            &mut snapshot_receiver,
            &mut snapshot_task,
        )
        .await?;
    }

    loop {
        if let Some(rx) = receiver.as_mut() {
            while let Ok(event) = rx.try_recv() {
                state.reduce(AppEvent::Workflow(event));
            }
        }

        if let Some(rx) = snapshot_receiver.as_mut() {
            while let Ok(snapshot) = rx.try_recv() {
                state.reduce(AppEvent::Snapshot(snapshot));
            }
        }

        while let Ok(event) = ui_event_rx.try_recv() {
            state.reduce(event);
        }

        terminal.draw(|frame| screens::render(frame, &state))?;

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
                    if let (Some(session), Some(command)) = (
                        session.as_ref(),
                        state.approval_accept_command(),
                    ) {
                        state.banner = Some(StatusBanner {
                            message: "Sending approval response...".to_string(),
                            is_error: false,
                        });
                        spawn_command(
                            session.handle(),
                            command,
                            ui_event_tx.clone(),
                            "Approval accepted",
                            "Approval failed",
                        );
                    }
                    state.clear_approval();
                }
                KeyCode::Char('n') => {
                    if let (Some(session), Some(command)) = (
                        session.as_ref(),
                        state.approval_reject_command(),
                    ) {
                        state.banner = Some(StatusBanner {
                            message: "Sending approval response...".to_string(),
                            is_error: false,
                        });
                        spawn_command(
                            session.handle(),
                            command,
                            ui_event_tx.clone(),
                            "Approval rejected",
                            "Approval failed",
                        );
                    }
                    state.clear_approval();
                }
                KeyCode::Up | KeyCode::Char('k') => state.move_up(),
                KeyCode::Down | KeyCode::Char('j') => state.move_down(),
                KeyCode::Enter => {
                    if matches!(state.approval, Some(ApprovalPrompt::AgentSelection { .. })) {
                        if let (Some(session), Some(command)) = (
                            session.as_ref(),
                            state.approval_accept_command(),
                        ) {
                            state.banner = Some(StatusBanner {
                                message: "Sending agent selection...".to_string(),
                                is_error: false,
                            });
                            spawn_command(
                                session.handle(),
                                command,
                                ui_event_tx.clone(),
                                "Agent selected",
                                "Agent selection failed",
                            );
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
                        attach_run(
                            &engine,
                            run_id,
                            &mut state,
                            &mut session,
                            &mut receiver,
                            &mut snapshot_receiver,
                            &mut snapshot_task,
                        )
                        .await?;
                    }
                }
                _ => {}
            },
            Screen::RunDetail => match key.code {
                KeyCode::Char('q') => {
                    if state.show_log_modal {
                        state.close_log_modal();
                        continue;
                    }
                    if let Some(task) = snapshot_task.take() {
                        task.abort();
                    }
                    receiver = None;
                    snapshot_receiver = None;
                    session = None;
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
                    if let (Some(session), Some(command)) = (
                        session.as_ref(),
                        state.selected_task_trigger_command(),
                    ) {
                        let WorkflowCommand::TriggerTask { task_id } = command else {
                            unreachable!("selected task trigger command must be TriggerTask");
                        };
                        session.handle().dispatch_trigger_task(task_id);
                        state.banner = Some(StatusBanner {
                            message: "Trigger request accepted".to_string(),
                            is_error: false,
                        });
                    }
                }
                KeyCode::Char('G') => {
                    if state.show_log_modal {
                        let viewport_height = log_modal_viewport_height(terminal.size()?.height);
                        state.scroll_logs_to_bottom(viewport_height);
                    }
                }
                KeyCode::Char('a') => {
                    if let Some(session) = session.as_ref() {
                        let task_ids = state.visible_awaiting_task_ids();
                        let requested = task_ids.len();
                        session.handle().dispatch_trigger_tasks(task_ids);
                        state.banner = Some(StatusBanner {
                            message: if requested > 0 {
                                format!("Trigger-all request accepted for {requested} task(s)")
                            } else {
                                "No awaiting tasks to trigger".to_string()
                            },
                            is_error: false,
                        });
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(session) = session.as_ref() {
                        session.handle().dispatch_cancel_workflow();
                        state.banner = Some(StatusBanner {
                            message: "Cancel request accepted".to_string(),
                            is_error: false,
                        });
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
    ui_event_tx: mpsc::UnboundedSender<AppEvent>,
    success_message: &'static str,
    failure_prefix: &'static str,
) {
    tokio::spawn(async move {
        let banner = match handle.send(command).await {
            Ok(()) => StatusBanner {
                message: success_message.to_string(),
                is_error: false,
            },
            Err(error) => StatusBanner {
                message: format!("{failure_prefix}: {error}"),
                is_error: true,
            },
        };
        let _ = ui_event_tx.send(AppEvent::Banner(banner));
    });
}

async fn attach_run(
    engine: &Engine,
    run_id: Uuid,
    state: &mut TuiState,
    session_slot: &mut Option<WorkflowSession>,
    receiver_slot: &mut Option<tokio::sync::broadcast::Receiver<butterflow_core::workflow_runtime::WorkflowEvent>>,
    snapshot_receiver_slot: &mut Option<mpsc::UnboundedReceiver<WorkflowSnapshot>>,
    snapshot_task_slot: &mut Option<tokio::task::JoinHandle<()>>,
) -> Result<()> {
    if let Some(task) = snapshot_task_slot.take() {
        task.abort();
    }
    let session = WorkflowSession::attach(engine.clone(), run_id);
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
    *session_slot = Some(session);
    *receiver_slot = Some(receiver);
    *snapshot_receiver_slot = Some(snapshot_rx);
    *snapshot_task_slot = Some(snapshot_task);
    Ok(())
}
