pub mod app;
pub mod event;
pub mod screens;

use std::collections::{HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use butterflow_core::ai_handoff::AgentOption;
use butterflow_core::config::{
    AgentSelectionCallback, CapabilitiesSecurityCallback, ShellCommandApprovalCallback,
    ShellCommandExecutionRequest,
};
use butterflow_core::engine::Engine;
use butterflow_core::execution::CodemodExecutionConfig;
use codemod_llrt_capabilities::module_builder::UNSAFE_MODULES;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;
use uuid::Uuid;

use app::{
    AgentSelectionItem, App, AppEffect, EffectResult, LogView, PendingAgentSelection,
    PendingCapabilityApproval, PendingShellApproval,
};
use event::{AppEvent, EventHandler};
use screens::{render_screen_background, StatusLine, StatusTone};

type TuiTerminal = Terminal<CrosstermBackend<File>>;

/// Run the TUI starting at the run list.
pub async fn run_tui(mut engine: Engine, limit: usize) -> Result<()> {
    let runtime = configure_engine_for_tui(&mut engine);

    let app = App::new(
        engine.is_dry_run(),
        engine.get_capabilities().clone(),
        limit,
    );
    run_tui_loop(app, engine, runtime).await
}

/// Run the TUI starting at the task list for a specific workflow run.
pub async fn run_tui_for_run(mut engine: Engine, workflow_run_id: Uuid) -> Result<()> {
    let runtime = configure_engine_for_tui(&mut engine);

    run_tui_for_run_with_runtime(engine, workflow_run_id, runtime).await
}

pub(crate) async fn run_tui_for_run_with_runtime(
    engine: Engine,
    workflow_run_id: Uuid,
    runtime: TuiRuntime,
) -> Result<()> {
    let app = App::new_for_run(
        engine.is_dry_run(),
        engine.get_capabilities().clone(),
        workflow_run_id,
    );
    run_tui_loop(app, engine, runtime).await
}

pub(crate) fn configure_engine_for_tui(engine: &mut Engine) -> TuiRuntime {
    engine.set_quiet(true);
    engine.set_progress_callback(Arc::new(None));
    let pre_approved_capabilities = engine.get_capabilities().clone().unwrap_or_default();
    let config = engine.workflow_run_config_mut();

    let (shell_tx, approval_rx) = mpsc::unbounded_channel();
    let approval_callback: ShellCommandApprovalCallback = Arc::new(move |request| {
        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
        shell_tx
            .send(ShellApprovalMessage {
                request: request.clone(),
                response_tx,
            })
            .map_err(|_| anyhow::anyhow!("TUI approval channel closed"))?;

        response_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("TUI approval response channel closed"))?
    });
    config.shell_command_approval_callback = Some(approval_callback);

    let (capability_tx, capability_approval_rx) = mpsc::unbounded_channel();
    let checked_capabilities = Arc::new(Mutex::new(pre_approved_capabilities));
    let unsafe_capabilities: HashSet<LlrtSupportedModules> =
        UNSAFE_MODULES.iter().copied().collect();
    let capability_callback: CapabilitiesSecurityCallback =
        Arc::new(move |request: &CodemodExecutionConfig| {
            let requested = request
                .capabilities
                .as_ref()
                .map(|set| {
                    let checked = checked_capabilities.lock().unwrap();
                    set.iter()
                        .filter(|module| {
                            unsafe_capabilities.contains(module) && !checked.contains(module)
                        })
                        .copied()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            if requested.is_empty() {
                return Ok(());
            }

            let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
            capability_tx
                .send(CapabilityApprovalMessage {
                    modules: requested.clone(),
                    response_tx,
                })
                .map_err(|_| anyhow::anyhow!("TUI capability approval channel closed"))?;

            response_rx.recv().map_err(|_| {
                anyhow::anyhow!("TUI capability approval response channel closed")
            })??;

            let mut checked = checked_capabilities.lock().unwrap();
            checked.extend(requested);
            Ok(())
        });
    config.capabilities_security_callback = Some(capability_callback);

    let (agent_selection_tx, agent_selection_rx) = mpsc::unbounded_channel();
    let agent_selection_callback: AgentSelectionCallback =
        Arc::new(move |agents: &[AgentOption]| {
            let mut options: Vec<AgentSelectionItem> = agents
                .iter()
                .filter(|agent| agent.is_available())
                .map(AgentSelectionItem::from_agent_option)
                .collect();
            options.push(AgentSelectionItem {
                canonical: app::USE_BUILT_IN_AGENT.to_string(),
                label: "Use built-in AI".to_string(),
                is_available: true,
            });

            let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
            if agent_selection_tx
                .send(AgentSelectionMessage {
                    options,
                    response_tx,
                })
                .is_err()
            {
                return None;
            }

            match response_rx.recv() {
                Ok(Ok(selection)) => selection,
                Ok(Err(_)) | Err(_) => None,
            }
        });
    config.agent_selection_callback = Some(agent_selection_callback);

    TuiRuntime {
        approval_rx,
        capability_approval_rx,
        agent_selection_rx,
    }
}

async fn run_tui_loop(mut app: App, mut engine: Engine, mut runtime: TuiRuntime) -> Result<()> {
    let mut session = TuiSession::enter()?;
    let mut terminal = session
        .terminal
        .take()
        .expect("TUI session should always own a terminal");
    terminal.clear()?;

    let mut events = EventHandler::new(std::time::Duration::from_millis(500));
    let result = run_event_loop(
        &mut app,
        &mut engine,
        &mut terminal,
        &mut events,
        &mut runtime,
    )
    .await;

    session.terminal = Some(terminal);
    let restore_result = session.restore();

    result?;
    restore_result?;
    Ok(())
}

struct ShellApprovalMessage {
    request: ShellCommandExecutionRequest,
    response_tx: std::sync::mpsc::SyncSender<Result<bool>>,
}

struct CapabilityApprovalMessage {
    modules: Vec<LlrtSupportedModules>,
    response_tx: std::sync::mpsc::SyncSender<Result<()>>,
}

struct AgentSelectionMessage {
    options: Vec<AgentSelectionItem>,
    response_tx: std::sync::mpsc::SyncSender<Result<Option<String>>>,
}

pub(crate) struct TuiRuntime {
    approval_rx: mpsc::UnboundedReceiver<ShellApprovalMessage>,
    capability_approval_rx: mpsc::UnboundedReceiver<CapabilityApprovalMessage>,
    agent_selection_rx: mpsc::UnboundedReceiver<AgentSelectionMessage>,
}

async fn run_event_loop(
    app: &mut App,
    engine: &mut Engine,
    terminal: &mut TuiTerminal,
    events: &mut EventHandler,
    runtime: &mut TuiRuntime,
) -> Result<()> {
    let mut pending_effects: VecDeque<AppEffect> = app.initial_effects().into();
    let mut approval_queue: VecDeque<PendingShellApproval> = VecDeque::new();
    let mut capability_approval_queue: VecDeque<PendingCapabilityApproval> = VecDeque::new();
    let mut agent_selection_queue: VecDeque<PendingAgentSelection> = VecDeque::new();
    let mut needs_redraw = true;

    loop {
        drain_shell_approvals(runtime, &mut approval_queue);
        drain_capability_approvals(runtime, &mut capability_approval_queue);
        drain_agent_selections(runtime, &mut agent_selection_queue);
        if !app.has_shell_approval() && !app.has_capability_approval() && !app.has_agent_selection()
        {
            if let Some(next_approval) = approval_queue.pop_front() {
                needs_redraw |= app.present_shell_approval(next_approval);
            } else if let Some(next_approval) = capability_approval_queue.pop_front() {
                needs_redraw |= app.present_capability_approval(next_approval);
            } else if let Some(next_selection) = agent_selection_queue.pop_front() {
                needs_redraw |= app.present_agent_selection(next_selection);
            }
        }

        while let Some(effect) = pending_effects.pop_front() {
            let should_refresh = effect.clone().should_refresh_after();
            let effect_result = execute_effect(app, engine, effect).await;
            needs_redraw |= app.apply_effect_result(effect_result);

            if should_refresh {
                pending_effects.push_back(AppEffect::Refresh);
            }
        }

        if app.should_quit {
            app.reject_shell_approval(Some(anyhow::anyhow!(
                "TUI closed while shell command approval was pending"
            )));
            while let Some(pending) = approval_queue.pop_front() {
                pending.fail(anyhow::anyhow!(
                    "TUI closed while shell command approval was pending"
                ));
            }
            app.reject_capability_approval(Some(anyhow::anyhow!(
                "TUI closed while capability approval was pending"
            )));
            while let Some(pending) = capability_approval_queue.pop_front() {
                pending.fail(anyhow::anyhow!(
                    "TUI closed while capability approval was pending"
                ));
            }
            app.reject_agent_selection(Some(anyhow::anyhow!(
                "TUI closed while agent selection was pending"
            )));
            while let Some(pending) = agent_selection_queue.pop_front() {
                pending.fail(anyhow::anyhow!(
                    "TUI closed while agent selection was pending"
                ));
            }
            break;
        }

        if needs_redraw {
            terminal.draw(|frame| {
                let area = frame.area();
                render_screen_background(frame, area);
                if let Some(request) = app.shell_approval_request() {
                    screens::render_shell_approval_modal(frame, area, request);
                    return;
                }
                if let Some(modules) = app.capability_approval_modules() {
                    screens::render_capability_approval_modal(frame, area, modules);
                    return;
                }
                if let Some(options) = app.agent_selection_options() {
                    screens::render_agent_selection_modal(
                        frame,
                        area,
                        options,
                        app.agent_selection_cursor,
                    );
                    return;
                }

                match &app.screen {
                    app::Screen::RunList => screens::run_list::render(
                        frame,
                        area,
                        &app.workflow_runs,
                        &mut app.run_list_state,
                        app.status_line.as_ref(),
                    ),
                    app::Screen::TaskList { .. } => screens::task_list::render(
                        frame,
                        area,
                        app.current_workflow_run.as_ref(),
                        &app.tasks,
                        &mut app.task_list_state,
                        app.status_line.as_ref(),
                        app.log_view.as_ref(),
                        app.log_scroll,
                        app.log_follow,
                    ),
                    app::Screen::Settings { .. } => screens::settings::render(
                        frame,
                        area,
                        app.current_workflow_run.as_ref(),
                        app.settings_cursor,
                        app.session_overrides.dry_run,
                        &app.session_overrides.capabilities,
                        app.status_line.as_ref(),
                    ),
                }
            })?;
            needs_redraw = false;
        }

        let first_event = events.next().await?;
        let mut event_batch = vec![first_event];
        event_batch.extend(events.drain_pending(255));

        for event in coalesce_events(event_batch) {
            needs_redraw |= matches!(
                event,
                AppEvent::Key(_)
                    | AppEvent::Mouse(_)
                    | AppEvent::Scroll(_)
                    | AppEvent::Resize(_, _)
            ) || matches!(event, AppEvent::Tick) && app.has_live_updates();
            pending_effects.extend(app.reduce(event));
        }
    }

    Ok(())
}

fn coalesce_events(events: Vec<AppEvent>) -> Vec<AppEvent> {
    let mut coalesced = Vec::new();
    let mut pending_scroll = 0i32;
    let mut pending_tick = false;
    let mut pending_resize: Option<(u16, u16)> = None;

    for event in events {
        match event {
            AppEvent::Mouse(mouse) => match mouse.kind {
                crossterm::event::MouseEventKind::ScrollDown => {
                    pending_scroll = pending_scroll.saturating_add(1);
                }
                crossterm::event::MouseEventKind::ScrollUp => {
                    pending_scroll = pending_scroll.saturating_sub(1);
                }
                _ => {
                    flush_coalesced_events(
                        &mut coalesced,
                        &mut pending_scroll,
                        &mut pending_tick,
                        &mut pending_resize,
                    );
                    coalesced.push(AppEvent::Mouse(mouse));
                }
            },
            AppEvent::Tick => {
                pending_tick = true;
            }
            AppEvent::Resize(width, height) => {
                pending_resize = Some((width, height));
            }
            other => {
                flush_coalesced_events(
                    &mut coalesced,
                    &mut pending_scroll,
                    &mut pending_tick,
                    &mut pending_resize,
                );
                coalesced.push(other);
            }
        }
    }

    flush_coalesced_events(
        &mut coalesced,
        &mut pending_scroll,
        &mut pending_tick,
        &mut pending_resize,
    );
    coalesced
}

fn flush_coalesced_events(
    coalesced: &mut Vec<AppEvent>,
    pending_scroll: &mut i32,
    pending_tick: &mut bool,
    pending_resize: &mut Option<(u16, u16)>,
) {
    if let Some((width, height)) = pending_resize.take() {
        coalesced.push(AppEvent::Resize(width, height));
    }
    if *pending_scroll != 0 {
        coalesced.push(AppEvent::Scroll(*pending_scroll));
        *pending_scroll = 0;
    }
    if *pending_tick {
        coalesced.push(AppEvent::Tick);
        *pending_tick = false;
    }
}

struct TuiSession {
    terminal: Option<TuiTerminal>,
    control: File,
    restored: bool,
}

impl TuiSession {
    fn enter() -> Result<Self> {
        let control = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let backend_tty = control.try_clone()?;
        enable_raw_mode()?;
        let mut control_for_setup = control.try_clone()?;
        execute!(
            control_for_setup,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            EnableFocusChange
        )?;

        let terminal = Terminal::new(CrosstermBackend::new(backend_tty))?;
        Ok(Self {
            terminal: Some(terminal),
            control,
            restored: false,
        })
    }

    fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        disable_raw_mode()?;
        execute!(
            self.control,
            DisableFocusChange,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
        if let Some(terminal) = self.terminal.as_mut() {
            terminal.show_cursor()?;
        }
        self.restored = true;
        Ok(())
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        if self.restored {
            return;
        }

        let _ = disable_raw_mode();
        let _ = execute!(
            self.control,
            DisableFocusChange,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        if let Some(terminal) = self.terminal.as_mut() {
            let _ = terminal.show_cursor();
        }
        self.restored = true;
    }
}

fn drain_shell_approvals(runtime: &mut TuiRuntime, queue: &mut VecDeque<PendingShellApproval>) {
    while let Ok(message) = runtime.approval_rx.try_recv() {
        queue.push_back(PendingShellApproval::new(
            message.request,
            message.response_tx,
        ));
    }
}

fn drain_capability_approvals(
    runtime: &mut TuiRuntime,
    queue: &mut VecDeque<PendingCapabilityApproval>,
) {
    while let Ok(message) = runtime.capability_approval_rx.try_recv() {
        queue.push_back(PendingCapabilityApproval::new(
            message.modules,
            message.response_tx,
        ));
    }
}

fn drain_agent_selections(runtime: &mut TuiRuntime, queue: &mut VecDeque<PendingAgentSelection>) {
    while let Ok(message) = runtime.agent_selection_rx.try_recv() {
        queue.push_back(PendingAgentSelection::new(
            message.options,
            message.response_tx,
        ));
    }
}

async fn execute_effect(app: &App, engine: &mut Engine, effect: AppEffect) -> EffectResult {
    match effect {
        AppEffect::Refresh => {
            let (workflow_runs, current_workflow_run, tasks) = match app.current_workflow_run_id() {
                Some(workflow_run_id) => {
                    let workflow_run = match engine.get_workflow_run(workflow_run_id).await {
                        Ok(workflow_run) => Some(workflow_run),
                        Err(error) => {
                            return EffectResult::Status(StatusLine {
                                tone: StatusTone::Error,
                                message: format!("Failed to load workflow run: {error}"),
                            });
                        }
                    };
                    let tasks = match engine.get_tasks(workflow_run_id).await {
                        Ok(tasks) => tasks,
                        Err(error) => {
                            return EffectResult::Status(StatusLine {
                                tone: StatusTone::Error,
                                message: format!("Failed to load tasks: {error}"),
                            });
                        }
                    };
                    (Vec::new(), workflow_run, tasks)
                }
                None => match engine.list_workflow_runs(app.run_list_limit).await {
                    Ok(workflow_runs) => (workflow_runs, None, Vec::new()),
                    Err(error) => {
                        return EffectResult::Status(StatusLine {
                            tone: StatusTone::Error,
                            message: format!("Failed to load workflow runs: {error}"),
                        });
                    }
                },
            };

            EffectResult::Refreshed {
                workflow_runs,
                current_workflow_run,
                tasks,
            }
        }
        AppEffect::LoadLogs {
            workflow_run_id,
            task_id,
        } => {
            let task = engine
                .get_tasks(workflow_run_id)
                .await
                .ok()
                .and_then(|tasks| tasks.into_iter().find(|task| task.id == task_id));
            EffectResult::LogsLoaded(task.as_ref().map(LogView::from_task))
        }
        AppEffect::TriggerTask {
            workflow_run_id,
            task_id,
        } => {
            apply_session_overrides(engine, &app.session_overrides);
            match engine.resume_workflow(workflow_run_id, vec![task_id]).await {
                Ok(()) => EffectResult::Noop,
                Err(error) => EffectResult::Status(StatusLine {
                    tone: StatusTone::Error,
                    message: format!("Failed to trigger task {task_id}: {error}"),
                }),
            }
        }
        AppEffect::TriggerAll { workflow_run_id } => {
            apply_session_overrides(engine, &app.session_overrides);
            match engine.trigger_all(workflow_run_id).await {
                Ok(true) | Ok(false) => EffectResult::Noop,
                Err(error) => EffectResult::Status(StatusLine {
                    tone: StatusTone::Error,
                    message: format!("Failed to trigger all tasks: {error}"),
                }),
            }
        }
        AppEffect::RetryTask {
            workflow_run_id,
            task_id,
        } => {
            apply_session_overrides(engine, &app.session_overrides);
            match engine.resume_workflow(workflow_run_id, vec![task_id]).await {
                Ok(()) => EffectResult::Noop,
                Err(error) => EffectResult::Status(StatusLine {
                    tone: StatusTone::Error,
                    message: format!("Failed to retry task {task_id}: {error}"),
                }),
            }
        }
        AppEffect::CancelWorkflow { workflow_run_id } => {
            match engine.cancel_workflow(workflow_run_id).await {
                Ok(()) => EffectResult::Noop,
                Err(error) => EffectResult::Status(StatusLine {
                    tone: StatusTone::Error,
                    message: format!("Failed to cancel workflow: {error}"),
                }),
            }
        }
    }
}

fn apply_session_overrides(engine: &mut Engine, overrides: &app::SessionOverrides) {
    engine.set_quiet(true);
    engine.set_progress_callback(std::sync::Arc::new(None));
    engine.set_dry_run(overrides.dry_run);
    engine.set_capabilities(overrides.capabilities.clone());
}

#[cfg(test)]
mod tests;
