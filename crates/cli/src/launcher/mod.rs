//! The interactive launcher: a terminal UI shown when a workflow runs in an
//! interactive terminal. A setup screen chooses the target, params, dry run,
//! capabilities, coding agent and decision model; a run screen shows the
//! steps live, their logs and diffs, and answers approvals in place.
//!
//! Only `workflow_exec` and the run commands call in here, and only when
//! [`enabled`] says so: `--no-interactive`, `--format jsonl`, a pipe, `CI`
//! or `CODEMOD_LAUNCHER=0` keep the plain output.

mod render;
mod setup;
mod view;

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use butterflow_core::structured_log::OutputFormat;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_workflow::events::Event;
use codemod_workflow::interaction::{
    AgentChoice, Approval, Ask, Decision, Interaction, NonInteractive,
};
use codemod_workflow::{load_workflow, RunReport, RunStatus, StartRequest, Summary};
use console::style;
use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::user_settings::{self, DecisionModel, SettingsFile, UserSettings};
use crate::workflow_exec::{ExecOutcome, ExecRequest, ExecSettings, RunFlags};

pub use setup::SetupChoices;
use setup::{SetupAction, SetupInput, SetupState};
use view::{Action, RunView, StepState};

/// Whether runs with these flags show the launcher.
pub fn enabled(flags: &RunFlags) -> bool {
    let opted_out = std::env::var("CODEMOD_LAUNCHER")
        .map(|value| matches!(value.trim(), "0" | "false" | "off" | "no"))
        .unwrap_or(false);
    flags.format == OutputFormat::Text
        && flags.control.is_none()
        && !opted_out
        && std::env::var_os("CI").is_none()
        && crate::utils::interactive::is_interactive(flags.no_interactive)
        && std::io::stdout().is_terminal()
}

// ------------------------------------------------------------ terminal ---

/// The terminal in raw mode on the alternate screen, restored when dropped.
struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Screen {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        if let Err(error) = crossterm::execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        terminal.hide_cursor()?;
        terminal.clear()?;
        Ok(Self { terminal })
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

// --------------------------------------------------------------- setup ---

/// What a run command knows before the setup screen.
pub struct Plan<'a> {
    pub title: String,
    pub workflow: &'a Path,
    pub target: PathBuf,
    pub params: HashMap<String, Value>,
    pub dry_run: bool,
    /// The run must be a dry run (a pro codemod).
    pub dry_run_locked: bool,
    /// Capabilities the package's manifest asks for.
    pub requested: HashSet<LlrtSupportedModules>,
    pub flags: &'a RunFlags,
}

impl SetupChoices {
    /// Put the capability, agent and model choices into the flags.
    pub fn apply_to(&self, flags: &mut RunFlags) {
        flags.allow_fs = self.granted.contains(&LlrtSupportedModules::Fs);
        flags.allow_fetch = self.granted.contains(&LlrtSupportedModules::Fetch);
        flags.allow_child_process = self.granted.contains(&LlrtSupportedModules::ChildProcess);
        flags.agent = self.agent.clone();
        flags.model = Some(self.decision_model.clone());
    }
}

/// Show the setup screen. `None` when the person quits; their agent and
/// model choices are saved when they start.
pub async fn setup(plan: Plan<'_>) -> Result<Option<SetupChoices>> {
    let saved = user_settings::load();
    let declared = if codemod_workflow::is_typescript(plan.workflow) {
        std::fs::read_to_string(plan.workflow)
            .map(|source| codemod_workflow::params::declared_params(&source))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let decision_model = saved.decision_model(plan.flags.model.as_ref());
    let known_models = user_settings::known_models(&decision_model.provider);
    let mut state = SetupState::new(SetupInput {
        title: plan.title,
        // Shown relative to where the command runs, when it is under it.
        workflow: std::env::current_dir()
            .ok()
            .and_then(|cwd| plan.workflow.strip_prefix(cwd).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| plan.workflow.to_path_buf()),
        target: plan.target,
        params: plan.params,
        declared,
        dry_run: plan.dry_run,
        dry_run_locked: plan.dry_run_locked,
        granted: plan.flags.granted(),
        requested: plan.requested,
        installed_agents: codemod_agents::installed()
            .into_iter()
            .map(|(agent, _)| (agent.id().to_string(), agent.label().to_string()))
            .collect(),
        agent: saved.agent(plan.flags.agent.as_deref()),
        decision_model: decision_model.clone(),
        known_models: known_models.clone(),
    });

    let provider = decision_model.provider.clone();
    let mut fetch = Some(tokio::spawn(async move {
        user_settings::fetch_models(&provider).await
    }));
    state.fetching_models = true;

    let mut screen = Screen::enter()?;
    let mut keys = EventStream::new();
    let mut key_error = None;
    let choices = loop {
        screen.terminal.draw(|frame| render::setup(frame, &state))?;
        tokio::select! {
            event = keys.next() => match event {
                Some(Ok(TermEvent::Key(key))) if key.kind != KeyEventKind::Release => {
                    match state.key(key) {
                        SetupAction::Start => break state.choices().ok(),
                        SetupAction::Quit => break None,
                        SetupAction::None => {}
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    key_error = Some(error.to_string());
                    break None;
                }
                None => break None,
            },
            fetched = async { fetch.as_mut().expect("guarded").await }, if fetch.is_some() => {
                fetch = None;
                state.fetching_models = false;
                if let Ok(Some(models)) = fetched {
                    if state.provider == decision_model.provider {
                        let current = state.model_name().to_string();
                        let mut all = models;
                        for known in &known_models {
                            if !all.contains(known) {
                                all.push(known.clone());
                            }
                        }
                        state.set_models(all, &current);
                    }
                }
            }
        }
    };
    drop(keys);
    drop(screen);
    if let Some(fetch) = fetch {
        fetch.abort();
    }

    if let Some(choices) = &choices {
        if let Some(file) = SettingsFile::default_location() {
            let settings = UserSettings {
                agent: choices.agent.clone(),
                decision_model: Some(choices.decision_model.clone()),
            };
            if settings != saved {
                if let Err(error) = file.save(&settings) {
                    eprintln!(
                        "{} could not save your agent and model choice: {error}",
                        style("warning:").yellow()
                    );
                }
            }
        }
    } else if let Some(error) = key_error {
        return Err(anyhow!("could not read the keyboard: {error}"));
    } else {
        eprintln!("Nothing ran.");
    }
    Ok(choices)
}

// ---------------------------------------------------------- the bridge ---

/// What the engine's thread tells the screen.
enum Message {
    Event(Event),
    Approval(Approval, Sender<Decision>),
    Agent(Vec<AgentChoice>, Sender<Option<String>>),
}

struct ChannelEvents(UnboundedSender<Message>);

impl crate::journal::Stamped for ChannelEvents {
    fn emit(&self, event: Event, _record: &Value) {
        let _ = self.0.send(Message::Event(event));
    }
}

/// Asks through the screen, waiting on the engine's thread for the answer.
struct ScreenInteraction {
    messages: UnboundedSender<Message>,
    approvals: crate::control::Approvals,
    /// Steps running at the same time must not ask at the same time.
    asking: Mutex<()>,
}

impl Interaction for ScreenInteraction {
    fn is_interactive(&self) -> bool {
        true
    }

    fn ask(&self, ask: &Ask, approval: &Approval) -> Decision {
        if self.approvals.gives(&ask.id, approval) {
            return Decision::Approved;
        }
        self.approve(approval)
    }

    fn approve(&self, approval: &Approval) -> Decision {
        if self.approvals.all && approval.given_by_approve_all() {
            return NonInteractive { approve_all: true }.approve(approval);
        }
        let _guard = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        let (reply, answer) = std::sync::mpsc::channel();
        if self
            .messages
            .send(Message::Approval(approval.clone(), reply))
            .is_err()
        {
            return view::unanswered(approval);
        }
        answer.recv().unwrap_or_else(|_| view::unanswered(approval))
    }

    fn choose_agent(&self, choices: &[AgentChoice]) -> Option<String> {
        if !choices.iter().any(|choice| choice.available) {
            return None;
        }
        let _guard = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        let (reply, answer) = std::sync::mpsc::channel();
        self.messages
            .send(Message::Agent(choices.to_vec(), reply))
            .ok()?;
        answer.recv().ok().flatten()
    }
}

// ----------------------------------------------------------------- run ---

enum Kind {
    Start {
        workflow: PathBuf,
        target: PathBuf,
        params: HashMap<String, Value>,
        name: Option<String>,
        dry_run: bool,
    },
    Resume(String),
}

/// Start a run on the run screen. A dry run can then be applied for real,
/// as a new run, from the same screen.
pub(crate) async fn run_start(request: ExecRequest) -> Result<ExecOutcome> {
    let title = request
        .name
        .clone()
        .unwrap_or_else(|| request.workflow.display().to_string());
    let dry_run_locked = request.settings.skip_shard_steps;
    let mut dry_run = request.dry_run;
    loop {
        let kind = Kind::Start {
            workflow: request.workflow.clone(),
            target: request.target.clone(),
            params: request.params.clone(),
            name: request.name.clone(),
            dry_run,
        };
        let (outcome, action) =
            run_on_screen(&title, kind, &request.settings, dry_run_locked).await?;
        if action != Action::Apply {
            return Ok(outcome);
        }
        // Applying is a run like any other: it asks about uncommitted
        // changes before its first write.
        dry_run = false;
    }
}

pub(crate) async fn run_resume(
    run_id: &str,
    title: String,
    settings: ExecSettings,
) -> Result<ExecOutcome> {
    let (outcome, _) =
        run_on_screen(&title, Kind::Resume(run_id.to_string()), &settings, true).await?;
    Ok(outcome)
}

fn agent_label(agent: &str) -> String {
    codemod_agents::find(agent)
        .map(|adapter| adapter.label().to_string())
        .unwrap_or_else(|| agent.to_string())
}

async fn run_on_screen(
    title: &str,
    kind: Kind,
    settings: &ExecSettings,
    dry_run_locked: bool,
) -> Result<(ExecOutcome, Action)> {
    let dry_run = matches!(kind, Kind::Start { dry_run: true, .. });
    let (sender, mut messages) = unbounded_channel();
    let interaction = Arc::new(ScreenInteraction {
        messages: sender.clone(),
        approvals: settings.approvals(),
        asking: Mutex::new(()),
    });
    let run_id = match &kind {
        Kind::Start { .. } => uuid::Uuid::new_v4().to_string(),
        Kind::Resume(run_id) => run_id.clone(),
    };
    // Dry runs are not saved, so they keep no journal.
    let events = Arc::new(crate::journal::Recorder::new(
        crate::workflow_exec::store(),
        (!dry_run).then(|| run_id.clone()),
        Arc::new(ChannelEvents(sender)),
    ));
    let (engine, summary) = crate::workflow_exec::engine_with(settings, events, interaction);
    let stop = engine.stop_handle();
    let allow_dirty = settings.allow_dirty;
    let force = settings.force;

    let workflow = match &kind {
        Kind::Start { workflow, .. } => {
            Some(load_workflow(workflow).map_err(|error| anyhow!("{error}"))?)
        }
        Kind::Resume(_) => None,
    };
    let (done_sender, mut done) = tokio::sync::oneshot::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let report = match kind {
            Kind::Start {
                target,
                params,
                name,
                dry_run,
                ..
            } => engine.start(StartRequest {
                run_id: Some(run_id),
                workflow: workflow.expect("loaded above"),
                target,
                params: Value::Object(params.into_iter().collect()),
                name,
                dry_run,
                test: None,
                check_uncommitted: !allow_dirty,
            }),
            Kind::Resume(run_id) => engine.resume_with(&run_id, force),
        };
        let _ = done_sender.send(report);
    });

    let mut view = RunView::new(title, dry_run);
    view.can_apply = dry_run && !dry_run_locked;
    view.agent = settings.agent.as_deref().map(agent_label);
    view.model = settings.decision_model.as_ref().map(DecisionModel::id);

    let mut screen = Screen::enter()?;
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));
    let signal = crate::workflow_exec::stop_requested();
    tokio::pin!(signal);
    let mut signaled = false;
    let mut report: Option<Result<RunReport, String>> = None;
    let mut messages_open = true;

    let action = loop {
        screen.terminal.draw(|frame| render::run(frame, &view))?;
        tokio::select! {
            message = messages.recv(), if messages_open => match message {
                Some(message) => {
                    receive(&mut view, &summary, message);
                    while let Ok(message) = messages.try_recv() {
                        receive(&mut view, &summary, message);
                    }
                }
                // The engine is gone; its end comes through `done`.
                None => messages_open = false,
            },
            result = &mut done, if report.is_none() => {
                // Everything the engine said before it ended comes first.
                while let Ok(message) = messages.try_recv() {
                    receive(&mut view, &summary, message);
                }
                let result = result
                    .map_err(|_| "the workflow engine stopped unexpectedly".to_string())
                    .and_then(|r| r.map_err(|error| error.to_string()));
                if let Err(error) = &result {
                    view.phase = view::Phase::Finished {
                        status: RunStatus::Failed,
                        error: Some(error.clone()),
                    };
                    view.can_apply = false;
                }
                if !dry_run {
                    view.add_recorded_diffs(&summary.diffs());
                }
                report = Some(result);
                if view.stopping || signaled {
                    break Action::Exit;
                }
            }
            event = keys.next() => match event {
                Some(Ok(TermEvent::Key(key))) if key.kind != KeyEventKind::Release => {
                    match view.key(key) {
                        Action::Stop => {
                            stop.stop();
                            view.abandon_questions();
                        }
                        Action::Exit if report.is_some() => break Action::Exit,
                        Action::Apply if report.is_some() => break Action::Apply,
                        _ => {}
                    }
                }
                Some(Ok(_)) => {}
                None | Some(Err(_)) => {
                    // The terminal went away: stop, and wait for the end.
                    stop.stop();
                    view.stopping = true;
                    view.abandon_questions();
                    if report.is_some() {
                        break Action::Exit;
                    }
                }
            },
            _ = &mut signal, if !signaled => {
                signaled = true;
                stop.stop();
                view.stopping = true;
                view.abandon_questions();
            }
            _ = tick.tick() => view.tick = view.tick.wrapping_add(1),
        }
    };
    drop(keys);
    drop(screen);

    // Wait for the engine if the screen closed first.
    let result = match report {
        Some(result) => result,
        None => done
            .await
            .map_err(|_| "the workflow engine stopped unexpectedly".to_string())
            .and_then(|r| r.map_err(|error| error.to_string())),
    };
    let report = result.map_err(|error| anyhow!(error))?;
    let outcome = ExecOutcome {
        report,
        summary,
        duration: started.elapsed(),
        dry_run,
        format: settings.format,
    };
    print_record(&view);
    crate::workflow_exec::print_ending(&outcome, settings.format, !dry_run);
    Ok((outcome, action))
}

fn receive(view: &mut RunView, summary: &Summary, message: Message) {
    match message {
        Message::Event(event) => {
            let step_finished = matches!(event, Event::StepFinished { .. });
            view.apply(event);
            if step_finished && !view.dry_run {
                view.add_recorded_diffs(&summary.diffs());
            }
        }
        Message::Approval(approval, reply) => {
            if view.stopping {
                let _ = reply.send(view::unanswered(&approval));
            } else {
                view.ask_approval(approval, reply);
            }
        }
        Message::Agent(choices, reply) => {
            if view.stopping {
                let _ = reply.send(None);
            } else {
                view.ask_agent(choices, reply);
            }
        }
    }
}

/// What the run did, printed after the screen closes so it stays in the
/// terminal's scrollback.
fn print_record(view: &RunView) {
    let mut lines = vec![format!(
        "{} {}{}",
        style(if view.resumed { "Resumed run" } else { "Run" }).bold(),
        style(view.run_id.as_deref().unwrap_or("-")).dim(),
        if view.dry_run {
            style(" (dry run)").yellow().to_string()
        } else {
            String::new()
        }
    )];
    for step in &view.steps {
        let symbol = match step.state {
            StepState::Succeeded => style("✓").green(),
            StepState::Replayed => style("↺").dim(),
            StepState::Partial => style("◐").yellow(),
            StepState::Failed => style("✗").red(),
            StepState::Canceled => style("■").yellow(),
            StepState::AwaitingApproval => style("‖").yellow(),
            StepState::Waiting | StepState::Running => style("…").dim(),
        };
        let mut line = format!("{symbol} {}", step.label);
        if let Some(error) = step
            .error
            .as_ref()
            .filter(|_| matches!(step.state, StepState::Failed | StepState::Partial))
        {
            line.push_str(&format!(": {error}"));
        }
        lines.push(line);
    }
    if !view.diffs.is_empty() {
        let (additions, deletions) = view.diffs.iter().fold((0, 0), |(a, d), diff| {
            (a + diff.additions, d + diff.deletions)
        });
        lines.push(format!(
            "{} file{} {} ({}, {})",
            view.diffs.len(),
            if view.diffs.len() == 1 { "" } else { "s" },
            if view.dry_run {
                "would change"
            } else {
                "changed"
            },
            style(format!("+{additions}")).green(),
            style(format!("-{deletions}")).red()
        ));
    }
    if matches!(
        view.phase,
        view::Phase::Finished {
            status: RunStatus::Succeeded,
            ..
        }
    ) {
        lines.push(style("Done").green().bold().to_string());
    }
    eprintln!("{}", lines.join("\n"));
}
