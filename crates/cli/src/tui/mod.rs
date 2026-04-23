pub mod app;
pub mod event;
#[cfg(test)]
mod render_latency_bench;
mod screens;

use std::io;

use anyhow::Result;
use arboard::Clipboard;
use butterflow_core::engine::Engine;
use butterflow_core::execution::ProgressCallback;
use butterflow_core::workflow_runtime::{
    publish_event, WorkflowCommand, WorkflowEvent, WorkflowSession,
};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::{broadcast, mpsc};
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

#[derive(Default)]
struct TuiPerfCounters {
    enabled: bool,
    draws: u64,
    workflow_events: u64,
    workflow_lag_reconciles: u64,
    ui_events: u64,
    terminal_events: u64,
    terminal_key_events: u64,
    terminal_resize_events: u64,
    deadline_wakeups: u64,
}

impl TuiPerfCounters {
    fn from_env() -> Self {
        Self {
            enabled: std::env::var_os("CODEMOD_TUI_PERF_METRICS").is_some(),
            ..Self::default()
        }
    }

    fn inc_draws(&mut self) {
        if self.enabled {
            self.draws += 1;
        }
    }

    fn inc_workflow_events(&mut self) {
        if self.enabled {
            self.workflow_events += 1;
        }
    }

    fn inc_workflow_lag_reconciles(&mut self) {
        if self.enabled {
            self.workflow_lag_reconciles += 1;
        }
    }

    fn inc_ui_events(&mut self) {
        if self.enabled {
            self.ui_events += 1;
        }
    }

    fn inc_terminal_key_event(&mut self) {
        if self.enabled {
            self.terminal_events += 1;
            self.terminal_key_events += 1;
        }
    }

    fn inc_terminal_resize_event(&mut self) {
        if self.enabled {
            self.terminal_events += 1;
            self.terminal_resize_events += 1;
        }
    }

    fn inc_terminal_other_event(&mut self) {
        if self.enabled {
            self.terminal_events += 1;
        }
    }

    fn inc_deadline_wakeups(&mut self) {
        if self.enabled {
            self.deadline_wakeups += 1;
        }
    }

    fn summary(&self) -> Option<String> {
        self.enabled.then(|| {
            format!(
                "TUI perf counters: draws={} workflow_events={} workflow_lag_reconciles={} ui_events={} terminal_events={} terminal_key_events={} terminal_resize_events={} deadline_wakeups={}",
                self.draws,
                self.workflow_events,
                self.workflow_lag_reconciles,
                self.ui_events,
                self.terminal_events,
                self.terminal_key_events,
                self.terminal_resize_events,
                self.deadline_wakeups,
            )
        })
    }
}

fn tui_perf_auto_exit_deadline() -> Option<std::time::Instant> {
    let seconds = std::env::var("CODEMOD_TUI_PERF_AUTO_EXIT_SECS")
        .ok()?
        .parse::<u64>()
        .ok()?;
    Some(std::time::Instant::now() + std::time::Duration::from_secs(seconds))
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
    let guard = TerminalGuard::enter()?;
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
    let mut perf_counters = TuiPerfCounters::from_env();

    if let Some(run_id) = run_id {
        attach_run(&mut engine, run_id, &mut state, &mut runtime).await?;
    }

    let result = run_tui_loop(
        &mut engine,
        &mut terminal,
        &mut state,
        limit,
        &mut runtime,
        &mut perf_counters,
    )
    .await;
    drop(terminal);
    drop(guard);
    if let Some(summary) = perf_counters.summary() {
        eprintln!("{summary}");
    }
    result
}

pub async fn run_workflow_tui_with_session(
    mut engine: Engine,
    session: WorkflowSession,
    limit: usize,
) -> Result<()> {
    let guard = TerminalGuard::enter()?;
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
    let mut perf_counters = TuiPerfCounters::from_env();
    let run_id = session.handle().workflow_run_id();
    engine.set_progress_callback(std::sync::Arc::new(Some(create_tui_progress_callback(
        run_id,
    ))));
    bind_session(&session, &mut state, &mut runtime).await?;
    runtime.session = Some(session);

    let result = run_tui_loop(
        &mut engine,
        &mut terminal,
        &mut state,
        limit,
        &mut runtime,
        &mut perf_counters,
    )
    .await;
    drop(terminal);
    drop(guard);
    if let Some(summary) = perf_counters.summary() {
        eprintln!("{summary}");
    }
    result
}

struct TuiRuntime {
    session: Option<WorkflowSession>,
    receiver: Option<tokio::sync::broadcast::Receiver<WorkflowEvent>>,
    ui_event_rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl TuiRuntime {
    fn new(ui_event_rx: mpsc::UnboundedReceiver<AppEvent>) -> Self {
        Self {
            session: None,
            receiver: None,
            ui_event_rx,
        }
    }
}

struct WorkflowReceiverDrain {
    applied_events: u64,
    needs_snapshot_reconcile: bool,
}

enum TuiLoopWake {
    WorkflowEvent(WorkflowEvent),
    WorkflowLagged,
    UiEvent(AppEvent),
    TerminalEvent(Event),
}

fn should_redraw(
    needs_redraw: bool,
    state_changed: bool,
    redraw_deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    let deadline_due = redraw_deadline.is_some_and(|deadline| deadline <= now);
    needs_redraw || state_changed || deadline_due
}

fn reduce_workflow_receiver(
    state: &mut TuiState,
    receiver: &mut broadcast::Receiver<WorkflowEvent>,
) -> WorkflowReceiverDrain {
    let mut applied_events = 0;

    loop {
        match receiver.try_recv() {
            Ok(event) => {
                state.reduce(AppEvent::Workflow(event));
                applied_events += 1;
            }
            Err(broadcast::error::TryRecvError::Empty)
            | Err(broadcast::error::TryRecvError::Closed) => {
                return WorkflowReceiverDrain {
                    applied_events,
                    needs_snapshot_reconcile: false,
                };
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                return WorkflowReceiverDrain {
                    applied_events,
                    needs_snapshot_reconcile: true,
                };
            }
        }
    }
}

async fn wait_for_next_wake(
    runtime: &mut TuiRuntime,
    terminal_events: &mut EventStream,
) -> Result<Option<TuiLoopWake>> {
    tokio::select! {
        maybe_event = terminal_events.next() => {
            Ok(maybe_event.transpose()?.map(TuiLoopWake::TerminalEvent))
        }
        recv_result = async {
            runtime
                .receiver
                .as_mut()
                .expect("workflow receiver should be present when awaiting workflow events")
                .recv()
                .await
        }, if runtime.receiver.is_some() => {
            match recv_result {
                Ok(event) => Ok(Some(TuiLoopWake::WorkflowEvent(event))),
                Err(broadcast::error::RecvError::Lagged(_)) => Ok(Some(TuiLoopWake::WorkflowLagged)),
                Err(broadcast::error::RecvError::Closed) => Ok(None),
            }
        }
        maybe_event = runtime.ui_event_rx.recv() => {
            Ok(maybe_event.map(TuiLoopWake::UiEvent))
        }
    }
}

async fn run_tui_loop(
    engine: &mut Engine,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
    limit: usize,
    runtime: &mut TuiRuntime,
    perf_counters: &mut TuiPerfCounters,
) -> Result<()> {
    let mut needs_redraw = true;
    let mut terminal_events = EventStream::new();
    let auto_exit_deadline = tui_perf_auto_exit_deadline();

    loop {
        if auto_exit_deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            break;
        }

        let mut state_changed = false;

        let workflow_drain = if let Some(rx) = runtime.receiver.as_mut() {
            reduce_workflow_receiver(state, rx)
        } else {
            WorkflowReceiverDrain {
                applied_events: 0,
                needs_snapshot_reconcile: false,
            }
        };
        state_changed |= workflow_drain.applied_events > 0;
        for _ in 0..workflow_drain.applied_events {
            perf_counters.inc_workflow_events();
        }

        if workflow_drain.needs_snapshot_reconcile {
            if let Some(session) = runtime.session.as_ref() {
                state.reduce(AppEvent::Snapshot(session.handle().load_snapshot().await?));
                state_changed = true;
                perf_counters.inc_workflow_lag_reconciles();
            }
        }

        while let Ok(event) = runtime.ui_event_rx.try_recv() {
            state.reduce(event);
            state_changed = true;
            perf_counters.inc_ui_events();
        }

        state_changed |= state.clear_expired_log_modal_notice();

        if matches!(state.screen, Screen::RunDetail) && (state_changed || needs_redraw) {
            let viewport_height = task_list_viewport_height(terminal.size()?.height);
            state_changed |= state.sync_task_list_scroll(viewport_height);
        }

        let redraw_deadline = state.next_redraw_deadline();
        let now = std::time::Instant::now();

        if should_redraw(needs_redraw, state_changed, redraw_deadline, now) {
            terminal.draw(|frame| screens::render(frame, state))?;
            needs_redraw = false;
            perf_counters.inc_draws();
        }

        let wait_deadline = match (redraw_deadline, auto_exit_deadline) {
            (Some(redraw_deadline), Some(auto_exit_deadline)) => {
                Some(redraw_deadline.min(auto_exit_deadline))
            }
            (Some(redraw_deadline), None) => Some(redraw_deadline),
            (None, Some(auto_exit_deadline)) => Some(auto_exit_deadline),
            (None, None) => None,
        };

        let wake = if let Some(deadline) = wait_deadline {
            match tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                wait_for_next_wake(runtime, &mut terminal_events),
            )
            .await
            {
                Ok(wake) => wake?,
                Err(_) => {
                    needs_redraw = true;
                    perf_counters.inc_deadline_wakeups();
                    continue;
                }
            }
        } else {
            wait_for_next_wake(runtime, &mut terminal_events).await?
        };

        let Some(wake) = wake else {
            continue;
        };

        match wake {
            TuiLoopWake::WorkflowEvent(event) => {
                state.reduce(AppEvent::Workflow(event));
                needs_redraw = true;
                perf_counters.inc_workflow_events();
                continue;
            }
            TuiLoopWake::WorkflowLagged => {
                if let Some(session) = runtime.session.as_ref() {
                    state.reduce(AppEvent::Snapshot(session.handle().load_snapshot().await?));
                    needs_redraw = true;
                    perf_counters.inc_workflow_lag_reconciles();
                }
                continue;
            }
            TuiLoopWake::UiEvent(event) => {
                state.reduce(event);
                needs_redraw = true;
                perf_counters.inc_ui_events();
                continue;
            }
            TuiLoopWake::TerminalEvent(Event::Resize(_, _)) => {
                perf_counters.inc_terminal_resize_event();
                needs_redraw = true;
                continue;
            }
            TuiLoopWake::TerminalEvent(Event::Key(key)) => {
                perf_counters.inc_terminal_key_event();
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                needs_redraw = true;

                if matches!(state.screen, Screen::RunDetail) && state.approval.is_some() {
                    match key.code {
                        KeyCode::Char('y') => {
                            if let (Some(session), Some(command)) =
                                (runtime.session.as_ref(), state.approval_accept_command())
                            {
                                spawn_command(session.handle(), command);
                            }
                            state.clear_approval();
                        }
                        KeyCode::Char('n')
                            if !matches!(
                                state.approval,
                                Some(ApprovalPrompt::WorktreeConsent { .. })
                                    | Some(ApprovalPrompt::PullRequestConsent { .. })
                                    | Some(ApprovalPrompt::ManualPullRequestConsent { .. })
                                    | Some(ApprovalPrompt::AgentSelection { .. })
                                    | Some(ApprovalPrompt::Selection { .. })
                            ) =>
                        {
                            if let (Some(session), Some(command)) =
                                (runtime.session.as_ref(), state.approval_reject_command())
                            {
                                spawn_command(session.handle(), command);
                            }
                            state.clear_approval();
                        }
                        KeyCode::Esc => {
                            if let (Some(session), Some(command)) =
                                (runtime.session.as_ref(), state.approval_reject_command())
                            {
                                spawn_command(session.handle(), command);
                            }
                            state.clear_approval();
                        }
                        KeyCode::Enter => {
                            if matches!(
                                state.approval,
                                Some(ApprovalPrompt::WorktreeConsent { .. })
                                    | Some(ApprovalPrompt::PullRequestConsent { .. })
                                    | Some(ApprovalPrompt::ManualPullRequestConsent { .. })
                                    | Some(ApprovalPrompt::AgentSelection { .. })
                                    | Some(ApprovalPrompt::Selection { .. })
                            ) {
                                if let (Some(session), Some(command)) =
                                    (runtime.session.as_ref(), state.approval_accept_command())
                                {
                                    spawn_command(session.handle(), command);
                                }
                                state.clear_approval();
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => state.move_up(),
                        KeyCode::Down | KeyCode::Char('j') => state.move_down(),
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
                            state.set_runs(
                                engine.list_workflow_runs(limit).await.unwrap_or_default(),
                            );
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
                                    Ok(()) => {
                                        state.set_log_modal_notice("Copied full log to clipboard")
                                    }
                                    Err(error) => state.set_log_modal_notice(format!(
                                        "Clipboard copy failed: {error}"
                                    )),
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
                                let viewport_height =
                                    log_modal_viewport_height(terminal.size()?.height);
                                state.scroll_logs_down(viewport_height, 1);
                            } else {
                                state.move_down();
                            }
                        }
                        KeyCode::Enter => {
                            if state.show_log_modal {
                                state.close_log_modal();
                            } else {
                                let viewport_height =
                                    log_modal_viewport_height(terminal.size()?.height);
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
                            } else {
                                state.begin_selected_task_trigger_confirmation();
                            }
                        }
                        KeyCode::Char('G') => {
                            if state.show_log_modal {
                                let viewport_height =
                                    log_modal_viewport_height(terminal.size()?.height);
                                state.scroll_logs_to_bottom(viewport_height);
                            }
                        }
                        KeyCode::Char('T') => {
                            state.begin_trigger_all_confirmation();
                        }
                        KeyCode::Char('p') => {
                            state.begin_create_pr_confirmation();
                        }
                        KeyCode::Char('c') => {
                            if let Some(session) = runtime.session.as_ref() {
                                spawn_command(session.handle(), WorkflowCommand::CancelWorkflow);
                            }
                        }
                        _ => {}
                    },
                }
                continue;
            }
            TuiLoopWake::TerminalEvent(_) => {
                perf_counters.inc_terminal_other_event();
                continue;
            }
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
    if runtime
        .session
        .as_ref()
        .is_some_and(|session| session.handle().workflow_run_id() == run_id)
    {
        let handle = runtime
            .session
            .as_ref()
            .expect("checked session presence")
            .handle();
        let snapshot = handle.load_snapshot().await?;
        let receiver = handle.subscribe();
        state.enter_run(snapshot);
        runtime.receiver = Some(receiver);
        return Ok(());
    }

    if let Some(session) = runtime.session.as_ref() {
        let handle = session.handle();
        for command in state.drain_approval_reject_commands() {
            let _ = handle.send(command).await;
        }
        runtime.receiver = None;
        runtime.session = None;
        state.clear_approvals();
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
    state.enter_run(snapshot);
    runtime.receiver = Some(receiver);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{reduce_workflow_receiver, should_redraw, TuiPerfCounters};
    use crate::tui::app::{Screen, TuiState};
    use butterflow_core::workflow_runtime::WorkflowEvent;
    use butterflow_models::{workflow::Workflow, WorkflowRun, WorkflowStatus};
    use chrono::Utc;
    use std::time::{Duration, Instant};
    use tokio::sync::broadcast;
    use uuid::Uuid;

    struct DrawCycle {
        needs_redraw: bool,
        state_changed: bool,
        redraw_deadline: Option<Instant>,
        now: Instant,
    }

    fn count_redraws(cycles: &[DrawCycle]) -> usize {
        cycles
            .iter()
            .filter(|cycle| {
                should_redraw(
                    cycle.needs_redraw,
                    cycle.state_changed,
                    cycle.redraw_deadline,
                    cycle.now,
                )
            })
            .count()
    }

    fn sample_workflow_run(run_id: Uuid) -> WorkflowRun {
        WorkflowRun {
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
        }
    }

    #[test]
    fn reduce_workflow_receiver_applies_available_events_without_requesting_snapshot() {
        let run_id = Uuid::new_v4();
        let (tx, mut rx) = broadcast::channel(4);
        let mut state = TuiState::default();
        tx.send(WorkflowEvent::WorkflowStarted {
            workflow_run: sample_workflow_run(run_id),
            at: Utc::now(),
        })
        .unwrap();

        let drain = reduce_workflow_receiver(&mut state, &mut rx);

        assert_eq!(drain.applied_events, 1);
        assert!(!drain.needs_snapshot_reconcile);
        assert_eq!(state.runs.len(), 1);
        assert_eq!(state.runs[0].id, run_id);
    }

    #[test]
    fn reduce_workflow_receiver_requests_snapshot_after_receiver_lag() {
        let run_id = Uuid::new_v4();
        let (tx, mut rx) = broadcast::channel(1);
        tx.send(WorkflowEvent::WorkflowStatusChanged {
            workflow_run_id: run_id,
            status: WorkflowStatus::Running,
            at: Utc::now(),
        })
        .unwrap();
        tx.send(WorkflowEvent::WorkflowStatusChanged {
            workflow_run_id: run_id,
            status: WorkflowStatus::Completed,
            at: Utc::now(),
        })
        .unwrap();

        let drain = reduce_workflow_receiver(&mut TuiState::default(), &mut rx);

        assert_eq!(drain.applied_events, 0);
        assert!(drain.needs_snapshot_reconcile);
    }

    #[test]
    fn should_redraw_is_false_for_static_idle_ui_without_changes() {
        let state = TuiState::default();
        let now = Instant::now();

        assert!(!should_redraw(
            false,
            false,
            state.next_redraw_deadline(),
            now
        ));
    }

    #[test]
    fn should_redraw_is_true_when_state_changes() {
        assert!(should_redraw(false, true, None, Instant::now()));
    }

    #[test]
    fn should_redraw_is_true_when_deadline_is_due() {
        let deadline = Instant::now() - Duration::from_millis(1);

        assert!(should_redraw(false, false, Some(deadline), Instant::now()));
    }

    #[test]
    fn should_redraw_is_false_when_running_ui_deadline_is_not_due_yet() {
        let state = TuiState {
            screen: Screen::Runs,
            runs: vec![sample_workflow_run(Uuid::new_v4())],
            ..TuiState::default()
        };
        let now = Instant::now();
        let deadline = state
            .next_redraw_deadline()
            .expect("running screen should have a redraw deadline");

        assert!(!should_redraw(false, false, Some(deadline), now));
    }

    #[test]
    fn draw_counter_counts_only_initial_draw_for_static_idle_screen() {
        let now = Instant::now();
        let draws = count_redraws(&[
            DrawCycle {
                needs_redraw: true,
                state_changed: false,
                redraw_deadline: None,
                now,
            },
            DrawCycle {
                needs_redraw: false,
                state_changed: false,
                redraw_deadline: None,
                now,
            },
            DrawCycle {
                needs_redraw: false,
                state_changed: false,
                redraw_deadline: None,
                now,
            },
        ]);

        assert_eq!(draws, 1);
    }

    #[test]
    fn draw_counter_counts_one_redraw_for_burst_of_drained_workflow_events() {
        let run_id = Uuid::new_v4();
        let (tx, mut rx) = broadcast::channel(4);
        let mut state = TuiState::default();
        tx.send(WorkflowEvent::WorkflowStarted {
            workflow_run: sample_workflow_run(run_id),
            at: Utc::now(),
        })
        .unwrap();
        tx.send(WorkflowEvent::WorkflowStatusChanged {
            workflow_run_id: run_id,
            status: WorkflowStatus::Completed,
            at: Utc::now(),
        })
        .unwrap();

        let drain = reduce_workflow_receiver(&mut state, &mut rx);
        assert_eq!(drain.applied_events, 2);

        let now = Instant::now();
        let draws = count_redraws(&[
            DrawCycle {
                needs_redraw: true,
                state_changed: false,
                redraw_deadline: None,
                now,
            },
            DrawCycle {
                needs_redraw: false,
                state_changed: drain.applied_events > 0,
                redraw_deadline: None,
                now,
            },
            DrawCycle {
                needs_redraw: false,
                state_changed: false,
                redraw_deadline: None,
                now,
            },
        ]);

        assert_eq!(draws, 2);
        assert_eq!(state.runs[0].status, WorkflowStatus::Completed);
    }

    #[test]
    fn draw_counter_counts_deadline_tick_for_running_screen() {
        let state = TuiState {
            screen: Screen::Runs,
            runs: vec![sample_workflow_run(Uuid::new_v4())],
            ..TuiState::default()
        };
        let now = Instant::now();
        let deadline = state
            .next_redraw_deadline()
            .expect("running screen should have a redraw deadline");

        let draws = count_redraws(&[
            DrawCycle {
                needs_redraw: true,
                state_changed: false,
                redraw_deadline: None,
                now,
            },
            DrawCycle {
                needs_redraw: false,
                state_changed: false,
                redraw_deadline: Some(deadline),
                now,
            },
            DrawCycle {
                needs_redraw: false,
                state_changed: false,
                redraw_deadline: Some(deadline),
                now: deadline + Duration::from_millis(1),
            },
        ]);

        assert_eq!(draws, 2);
    }

    #[test]
    fn perf_counter_summary_is_disabled_by_default() {
        assert!(TuiPerfCounters::default().summary().is_none());
    }

    #[test]
    fn perf_counter_summary_renders_enabled_counts() {
        let mut counters = TuiPerfCounters {
            enabled: true,
            ..TuiPerfCounters::default()
        };
        counters.inc_draws();
        counters.inc_workflow_events();
        counters.inc_workflow_lag_reconciles();
        counters.inc_ui_events();
        counters.inc_terminal_key_event();
        counters.inc_deadline_wakeups();

        let summary = counters.summary().expect("enabled counters should render");
        assert!(summary.contains("draws=1"));
        assert!(summary.contains("workflow_events=1"));
        assert!(summary.contains("workflow_lag_reconciles=1"));
        assert!(summary.contains("ui_events=1"));
        assert!(summary.contains("terminal_key_events=1"));
        assert!(summary.contains("deadline_wakeups=1"));
    }
}
