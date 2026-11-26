//! The run screen's state: engine events and the person's keys in, a view
//! model out. Nothing here touches the terminal, so it is tested directly.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use butterflow_core::diff::FileDiff;
use codemod_workflow::events::{Event, StepAt};
use codemod_workflow::interaction::{AgentChoice, Approval, Decision};
use codemod_workflow::{RunStatus, StepStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The most lines a log keeps; older ones are dropped.
const MAX_LOG_LINES: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Running,
    /// Finished in an earlier attempt of the run; its result was reused.
    Replayed,
    Succeeded,
    Partial,
    Failed,
    Canceled,
    AwaitingApproval,
    Waiting,
}

impl From<StepStatus> for StepState {
    fn from(status: StepStatus) -> Self {
        match status {
            StepStatus::Succeeded => Self::Succeeded,
            StepStatus::Partial => Self::Partial,
            StepStatus::Failed => Self::Failed,
            StepStatus::Canceled => Self::Canceled,
            StepStatus::AwaitingApproval => Self::AwaitingApproval,
            StepStatus::Rejected => Self::Failed,
            StepStatus::Waiting => Self::Waiting,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepView {
    pub position: u64,
    pub label: String,
    pub state: StepState,
    pub processed: usize,
    pub total: usize,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    /// The coding agent running it, for `agent(...)` steps.
    pub agent: Option<String>,
    /// The attempt that failed and how many there are, while retrying.
    pub retry: Option<(u32, u32)>,
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    pub path: String,
    pub diff: String,
    pub additions: usize,
    pub deletions: usize,
    /// The step that made it; `None` for a dry run's whole-run diff.
    pub step: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Logs,
    Diff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Running,
    Finished {
        status: RunStatus,
        error: Option<String>,
    },
}

pub enum Modal {
    Approval {
        approval: Approval,
        reply: Sender<Decision>,
    },
    Agent {
        choices: Vec<AgentChoice>,
        selected: usize,
        reply: Sender<Option<String>>,
    },
    /// Stop the run? A question it waits on stays open until the answer.
    ConfirmStop { pending: Option<Box<Modal>> },
}

/// What a key asks the screen's owner to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// Stop the run; it ends canceled and can be resumed.
    Stop,
    /// Leave the screen; the run has ended.
    Exit,
    /// Run the dry run's workflow again, for real.
    Apply,
}

pub struct RunView {
    pub title: String,
    pub run_id: Option<String>,
    pub dry_run: bool,
    pub resumed: bool,
    /// The coding agent and decision model the run was started with.
    pub agent: Option<String>,
    pub model: Option<String>,
    pub phase: Phase,
    pub steps: Vec<StepView>,
    /// Everything the run printed: its own output and every step's.
    pub output: Vec<String>,
    pub experimental_notices: Vec<String>,
    pub diffs: Vec<DiffView>,
    /// 0 is the whole run; `i + 1` is `steps[i]`.
    pub selected: usize,
    pub pane: Pane,
    /// Lines scrolled up from the bottom of the pane; 0 follows new lines.
    pub scroll: usize,
    pub modal: Option<Modal>,
    /// A finished dry run may be applied for real.
    pub can_apply: bool,
    pub stopping: bool,
    pub tick: usize,
    /// Streamed agent text not yet followed by a full line, per step.
    agent_text: HashMap<u64, String>,
}

impl RunView {
    pub fn new(title: impl Into<String>, dry_run: bool) -> Self {
        Self {
            title: title.into(),
            run_id: None,
            dry_run,
            resumed: false,
            agent: None,
            model: None,
            phase: Phase::Starting,
            steps: Vec::new(),
            output: Vec::new(),
            experimental_notices: Vec::new(),
            diffs: Vec::new(),
            selected: 0,
            pane: Pane::Logs,
            scroll: 0,
            modal: None,
            can_apply: dry_run,
            stopping: false,
            tick: 0,
            agent_text: HashMap::new(),
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.phase, Phase::Finished { .. })
    }

    pub fn selected_step(&self) -> Option<&StepView> {
        self.selected.checked_sub(1).and_then(|i| self.steps.get(i))
    }

    fn step_mut(&mut self, position: u64) -> Option<&mut StepView> {
        self.steps.iter_mut().find(|step| step.position == position)
    }

    fn start_step(&mut self, position: u64, label: String, state: StepState) {
        if let Some(step) = self.step_mut(position) {
            step.state = state;
            step.label = label;
            return;
        }
        self.steps.push(StepView {
            position,
            label,
            state,
            processed: 0,
            total: 0,
            error: None,
            duration_ms: None,
            agent: None,
            retry: None,
            logs: Vec::new(),
        });
    }

    fn log(&mut self, position: Option<u64>, text: String) {
        // One entry may hold several lines (`console.log` of a template).
        for line in text.trim_end_matches(['\n', '\r']).split('\n') {
            let line = line.trim_end_matches('\r').to_string();
            if let Some(step) = position.and_then(|p| self.step_mut(p)) {
                push_line(&mut step.logs, line.clone());
            }
            push_line(&mut self.output, line);
        }
    }

    fn flush_agent_text(&mut self, position: u64) {
        if let Some(text) = self.agent_text.remove(&position) {
            for line in text.trim_end().lines() {
                self.log(Some(position), line.to_string());
            }
        }
    }

    pub fn apply(&mut self, event: Event) {
        // A nested codemod's steps show as their parent step's.
        let Some(event) = event.top_level() else {
            return;
        };
        match event {
            Event::Experimental { api } => {
                let notice = crate::experimental_notice(&api);
                self.log(None, notice.clone());
                self.experimental_notices.push(notice);
            }
            Event::RunStarted {
                run_id, resumed, ..
            } => {
                self.run_id = Some(run_id);
                self.resumed = resumed;
                self.phase = Phase::Running;
            }
            Event::StepStarted { at, label, .. } => {
                self.start_step(at.position, label, StepState::Running)
            }
            Event::StepReplayed { at, label, .. } => {
                self.start_step(at.position, label, StepState::Replayed)
            }
            Event::StepProgress {
                at: StepAt { position, .. },
                processed,
                total,
            } => {
                if let Some(step) = self.step_mut(position) {
                    step.processed = processed;
                    step.total = total;
                }
            }
            Event::StepLog { at, line } => self.log(Some(at.position), line),
            Event::RunLog { line, .. } => self.log(None, line),
            Event::StepAgent {
                at: StepAt { position, .. },
                agent,
                activity,
            } => {
                if let Some(step) = self.step_mut(position) {
                    step.agent = Some(agent);
                }
                let line_open = self
                    .agent_text
                    .get(&position)
                    .is_some_and(|text| !text.is_empty() && !text.ends_with('\n'));
                use crate::agent_log_renderer::{render_agent_activity, RenderedAgentEvent};
                match render_agent_activity(&activity, line_open) {
                    Some(RenderedAgentEvent::Line(line)) => {
                        self.flush_agent_text(position);
                        for line in console::strip_ansi_codes(&line).lines() {
                            self.log(Some(position), line.to_string());
                        }
                    }
                    Some(RenderedAgentEvent::Fragment(fragment)) => {
                        let fragment = console::strip_ansi_codes(&fragment).to_string();
                        let text = self.agent_text.entry(position).or_default();
                        text.push_str(&fragment);
                        // Whole lines go to the log as they complete.
                        if let Some(end) = text.rfind('\n') {
                            let done: String = text.drain(..=end).collect();
                            for line in done.lines() {
                                self.log(Some(position), line.to_string());
                            }
                        }
                    }
                    None => {}
                }
            }
            Event::StepRetrying {
                at: StepAt { position, .. },
                label,
                attempt,
                attempts,
                error,
            } => {
                if let Some(step) = self.step_mut(position) {
                    step.retry = Some((attempt, attempts));
                }
                self.log(
                    Some(position),
                    format!("{label}: attempt {attempt} of {attempts} failed: {error}"),
                );
            }
            Event::StepFinished {
                at: StepAt { position, .. },
                label,
                status,
                error,
                duration_ms,
                ..
            } => {
                self.flush_agent_text(position);
                if self.step_mut(position).is_none() {
                    self.start_step(position, label, StepState::Running);
                }
                if let Some(step) = self.step_mut(position) {
                    step.state = status.into();
                    step.error = error;
                    step.duration_ms = Some(duration_ms);
                    step.retry = None;
                    if status == StepStatus::Succeeded && step.total > 0 {
                        step.processed = step.total;
                    }
                }
            }
            Event::RunDiff {
                path,
                diff,
                additions,
                deletions,
            } => self.diffs.push(DiffView {
                path,
                diff,
                additions,
                deletions,
                step: None,
            }),
            // A real run's diffs come from the run's summary; questions are
            // asked through the interaction.
            Event::StepDiff { .. }
            | Event::Question { .. }
            | Event::QuestionClosed { .. }
            | Event::ControlError { .. } => {}
            Event::RunFinished { status, error, .. } => {
                for position in self.agent_text.keys().copied().collect::<Vec<_>>() {
                    self.flush_agent_text(position);
                }
                // A run that waits for an approval has not finished, but
                // nothing more happens in this process.
                self.can_apply &= status == RunStatus::Succeeded;
                self.phase = Phase::Finished { status, error };
            }
        }
    }

    /// Take the diffs a real run recorded since the last call (`seen` of
    /// them were taken before).
    pub fn add_recorded_diffs(&mut self, diffs: &[FileDiff]) {
        let seen = self.diffs.len();
        for diff in diffs.iter().skip(seen) {
            self.diffs.push(DiffView {
                path: diff.path.clone(),
                diff: diff.diff_text.clone(),
                additions: diff.additions,
                deletions: diff.deletions,
                step: diff.step_id.as_deref().and_then(|id| id.parse().ok()),
            });
        }
    }

    /// The diffs to show for the selection: a step's own, or every one.
    pub fn visible_diffs(&self) -> Vec<&DiffView> {
        match self.selected_step() {
            Some(step) => self
                .diffs
                .iter()
                .filter(|diff| diff.step == Some(step.position))
                .collect(),
            None => self.diffs.iter().collect(),
        }
    }

    pub fn ask_approval(&mut self, approval: Approval, reply: Sender<Decision>) {
        self.modal = Some(Modal::Approval { approval, reply });
    }

    pub fn ask_agent(&mut self, choices: Vec<AgentChoice>, reply: Sender<Option<String>>) {
        self.modal = Some(Modal::Agent {
            choices: choices.into_iter().filter(|c| c.available).collect(),
            selected: 0,
            reply,
        });
    }

    /// Whether a question waits for the person.
    pub fn is_asking(&self) -> bool {
        matches!(
            self.modal,
            Some(
                Modal::Approval { .. }
                    | Modal::Agent { .. }
                    | Modal::ConfirmStop { pending: Some(_) }
            )
        )
    }

    /// Answer what waits, as nobody answering would: the run is stopping.
    pub fn abandon_questions(&mut self) {
        fn abandon(modal: Modal) {
            match modal {
                Modal::Approval { approval, reply } => {
                    let _ = reply.send(unanswered(&approval));
                }
                Modal::Agent { reply, .. } => {
                    let _ = reply.send(None);
                }
                Modal::ConfirmStop { pending } => {
                    if let Some(pending) = pending {
                        abandon(*pending);
                    }
                }
            }
        }
        if let Some(modal) = self.modal.take() {
            abandon(modal);
        }
    }

    /// Ask whether to stop, keeping `pending` to come back to.
    fn confirm_stop(&mut self, pending: Option<Modal>) {
        self.modal = Some(Modal::ConfirmStop {
            pending: pending.map(Box::new),
        });
    }

    pub fn key(&mut self, key: KeyEvent) -> Action {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c'));
        let quit = ctrl_c || matches!(key.code, KeyCode::Char('q'));
        match self.modal.take() {
            Some(Modal::Approval { approval, reply }) => {
                let decision = match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => Some(Decision::Approved),
                    KeyCode::Char('n') | KeyCode::Char('N') => Some(Decision::Denied),
                    KeyCode::Char('l') | KeyCode::Char('L')
                        if matches!(approval, Approval::Manual { .. }) =>
                    {
                        Some(Decision::Deferred)
                    }
                    KeyCode::Esc => Some(unanswered(&approval)),
                    _ if quit => {
                        self.confirm_stop(Some(Modal::Approval { approval, reply }));
                        return Action::None;
                    }
                    _ => None,
                };
                match decision {
                    Some(decision) => {
                        let _ = reply.send(decision);
                    }
                    None => {
                        // Keys that do not answer still move around.
                        self.navigate(key);
                        self.modal = Some(Modal::Approval { approval, reply });
                    }
                }
                Action::None
            }
            Some(Modal::Agent {
                choices,
                mut selected,
                reply,
            }) => {
                // The last row is the built-in agent.
                let rows = choices.len() + 1;
                if quit {
                    self.confirm_stop(Some(Modal::Agent {
                        choices,
                        selected,
                        reply,
                    }));
                    return Action::None;
                }
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(rows - 1),
                    KeyCode::Enter => {
                        let _ = reply.send(choices.get(selected).map(|c| c.name.clone()));
                        return Action::None;
                    }
                    KeyCode::Esc => {
                        let _ = reply.send(None);
                        return Action::None;
                    }
                    _ => {}
                }
                self.modal = Some(Modal::Agent {
                    choices,
                    selected,
                    reply,
                });
                Action::None
            }
            Some(Modal::ConfirmStop { pending }) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.modal = Some(Modal::ConfirmStop { pending });
                    self.abandon_questions();
                    self.stop()
                }
                _ if ctrl_c => {
                    self.modal = Some(Modal::ConfirmStop { pending });
                    self.abandon_questions();
                    self.stop()
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Back to the question, if one waits.
                    self.modal = pending.map(|pending| *pending);
                    Action::None
                }
                _ => {
                    self.modal = Some(Modal::ConfirmStop { pending });
                    Action::None
                }
            },
            None => {
                if ctrl_c || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    if self.is_finished() {
                        return Action::Exit;
                    }
                    if !self.stopping {
                        self.confirm_stop(None);
                    }
                    return Action::None;
                }
                match key.code {
                    KeyCode::Char('a') if self.is_finished() && self.can_apply => Action::Apply,
                    KeyCode::Enter if self.is_finished() => Action::Exit,
                    _ => {
                        self.navigate(key);
                        Action::None
                    }
                }
            }
        }
    }

    fn stop(&mut self) -> Action {
        if self.is_finished() {
            return Action::Exit;
        }
        self.stopping = true;
        Action::Stop
    }

    fn navigate(&mut self, key: KeyEvent) {
        let rows = self.steps.len() + 1;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                self.scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(rows - 1);
                self.scroll = 0;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected = 0;
                self.scroll = 0;
            }
            KeyCode::Tab | KeyCode::Char('d') => {
                self.pane = match self.pane {
                    Pane::Logs => Pane::Diff,
                    Pane::Diff => Pane::Logs,
                };
                self.scroll = 0;
            }
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::End | KeyCode::Char('G') => self.scroll = 0,
            _ => {}
        }
    }
}

/// What a question nobody answers gets: a manual approval waits for a
/// resume, the rest are refused.
pub fn unanswered(approval: &Approval) -> Decision {
    approval.unanswered()
}

fn push_line(lines: &mut Vec<String>, line: String) {
    lines.push(line);
    if lines.len() > MAX_LOG_LINES {
        let extra = lines.len() - MAX_LOG_LINES;
        lines.drain(..extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn started(view: &mut RunView) {
        view.apply(Event::RunStarted {
            run_id: "run-1".into(),
            workflow: "workflow.ts".into(),
            resumed: false,
            target: "target".into(),
            params: serde_json::Value::Null,
            dry_run: false,
            settings: codemod_workflow::events::Settings {
                capabilities: Vec::new(),
                managed_git: false,
                agent: None,
                host: None,
            },
        });
    }

    #[test]
    fn events_build_the_step_list() {
        let mut view = RunView::new("demo", false);
        started(&mut view);
        view.apply(Event::StepReplayed {
            at: StepAt::top(0),
            kind: None,
            label: "Find".into(),
        });
        view.apply(Event::StepStarted {
            at: StepAt::top(1),
            kind: None,
            label: "Rename".into(),
        });
        view.apply(Event::StepProgress {
            at: StepAt::top(1),
            processed: 3,
            total: 10,
        });
        view.apply(Event::StepLog {
            at: StepAt::top(1),
            line: "renamed 2× in a.ts".into(),
        });
        view.apply(Event::RunLog {
            step: None,
            line: "hello\n  world\n".into(),
        });

        assert_eq!(view.run_id.as_deref(), Some("run-1"));
        assert_eq!(view.phase, Phase::Running);
        assert_eq!(view.steps.len(), 2);
        assert_eq!(view.steps[0].state, StepState::Replayed);
        assert_eq!(view.steps[1].state, StepState::Running);
        assert_eq!((view.steps[1].processed, view.steps[1].total), (3, 10));
        assert_eq!(view.steps[1].logs, vec!["renamed 2× in a.ts"]);
        assert_eq!(view.output, vec!["renamed 2× in a.ts", "hello", "  world"]);

        view.apply(Event::StepRetrying {
            at: StepAt::top(1),
            label: "Rename".into(),
            attempt: 1,
            attempts: 3,
            error: "boom".into(),
        });
        assert_eq!(view.steps[1].retry, Some((1, 3)));

        view.apply(Event::StepFinished {
            at: StepAt::top(1),
            kind: None,
            label: "Rename".into(),
            status: StepStatus::Partial,
            error: Some("1 of 10 files failed".into()),
            duration_ms: 1500,
        });
        let step = &view.steps[1];
        assert_eq!(step.state, StepState::Partial);
        assert_eq!(step.error.as_deref(), Some("1 of 10 files failed"));
        assert_eq!(step.duration_ms, Some(1500));
        assert_eq!(step.retry, None);

        view.apply(Event::RunFinished {
            awaiting: None,
            question: None,
            run_id: "run-1".into(),
            status: RunStatus::Succeeded,
            error: None,
            failed_files: 0,
            partial_steps: Vec::new(),
            output: serde_json::Value::Null,
            output_omitted: None,
            steps: Vec::new(),
        });
        assert!(view.is_finished());
    }

    #[test]
    fn a_finished_step_counts_all_its_files() {
        let mut view = RunView::new("demo", false);
        view.apply(Event::StepStarted {
            at: StepAt::top(4),
            kind: None,
            label: "Migrate".into(),
        });
        view.apply(Event::StepProgress {
            at: StepAt::top(4),
            processed: 9,
            total: 10,
        });
        view.apply(Event::StepFinished {
            at: StepAt::top(4),
            kind: None,
            label: "Migrate".into(),
            status: StepStatus::Succeeded,
            error: None,
            duration_ms: 10,
        });
        assert_eq!(view.steps[0].processed, 10);
    }

    #[test]
    fn streamed_agent_text_becomes_whole_lines() {
        let mut view = RunView::new("demo", false);
        view.apply(Event::StepStarted {
            at: StepAt::top(2),
            kind: None,
            label: "Agent".into(),
        });
        let text = |t: &str| Event::StepAgent {
            at: StepAt::top(2),
            agent: "claude-code".into(),
            activity: codemod_agents::AgentEvent::Text {
                text: t.into(),
                delta: true,
            },
        };
        view.apply(text("Looking at "));
        view.apply(text("the files\nDone"));
        assert_eq!(view.steps[0].agent.as_deref(), Some("claude-code"));
        assert_eq!(view.steps[0].logs.len(), 1);
        assert!(view.steps[0].logs[0].contains("Looking at the files"));
        view.apply(Event::StepFinished {
            at: StepAt::top(2),
            kind: None,
            label: "Agent".into(),
            status: StepStatus::Succeeded,
            error: None,
            duration_ms: 1,
        });
        assert!(view.steps[0].logs[1].contains("Done"));
    }

    #[test]
    fn diffs_show_per_step_or_for_the_whole_run() {
        let mut view = RunView::new("demo", false);
        for (position, label) in [(0, "A"), (1, "B")] {
            view.apply(Event::StepStarted {
                at: StepAt::top(position),
                kind: None,
                label: label.into(),
            });
        }
        let diff = |path: &str, step: &str| FileDiff {
            path: path.into(),
            diff_text: format!("--- a/{path}\n+++ b/{path}\n"),
            additions: 1,
            deletions: 1,
            step_id: Some(step.into()),
            step_name: None,
            parent_step_id: None,
            parent_step_name: None,
        };
        let recorded = vec![diff("a.ts", "0"), diff("b.ts", "1")];
        view.add_recorded_diffs(&recorded[..1]);
        view.add_recorded_diffs(&recorded);
        assert_eq!(view.diffs.len(), 2);

        assert_eq!(view.visible_diffs().len(), 2);
        view.key(key(KeyCode::Down));
        assert_eq!(view.visible_diffs()[0].path, "a.ts");
        view.key(key(KeyCode::Down));
        assert_eq!(view.visible_diffs()[0].path, "b.ts");
    }

    #[test]
    fn a_dry_run_reports_its_diff_and_can_be_applied() {
        let mut view = RunView::new("demo", true);
        started(&mut view);
        view.apply(Event::RunDiff {
            path: "a.ts".into(),
            diff: "-a\n+b\n".into(),
            additions: 1,
            deletions: 1,
        });
        assert_eq!(view.key(key(KeyCode::Char('a'))), Action::None);
        view.apply(Event::RunFinished {
            awaiting: None,
            question: None,
            run_id: "run-1".into(),
            status: RunStatus::Succeeded,
            error: None,
            failed_files: 0,
            partial_steps: Vec::new(),
            output: serde_json::Value::Null,
            output_omitted: None,
            steps: Vec::new(),
        });
        assert_eq!(view.diffs[0].step, None);
        assert_eq!(view.key(key(KeyCode::Char('a'))), Action::Apply);
    }

    #[test]
    fn a_failed_dry_run_cannot_be_applied() {
        let mut view = RunView::new("demo", true);
        view.apply(Event::RunFinished {
            awaiting: None,
            question: None,
            run_id: "run-1".into(),
            status: RunStatus::Failed,
            error: Some("boom".into()),
            failed_files: 0,
            partial_steps: Vec::new(),
            output: serde_json::Value::Null,
            output_omitted: None,
            steps: Vec::new(),
        });
        assert_eq!(view.key(key(KeyCode::Char('a'))), Action::None);
    }

    #[test]
    fn approvals_are_answered_with_keys() {
        let manual = || Approval::Manual {
            message: "Rewrite packages/api?".into(),
        };
        for (code, expected) in [
            (KeyCode::Char('y'), Decision::Approved),
            (KeyCode::Char('n'), Decision::Denied),
            (KeyCode::Char('l'), Decision::Deferred),
            (KeyCode::Esc, Decision::Deferred),
        ] {
            let mut view = RunView::new("demo", false);
            let (reply, answer) = channel();
            view.ask_approval(manual(), reply);
            assert!(view.is_asking());
            // Moving around does not answer.
            view.key(key(KeyCode::Down));
            assert!(answer.try_recv().is_err());
            view.key(key(code));
            assert_eq!(answer.try_recv().unwrap(), expected, "{code:?}");
            assert!(view.modal.is_none());
        }
    }

    #[test]
    fn only_manual_approvals_can_wait_for_later() {
        let mut view = RunView::new("demo", false);
        let (reply, answer) = channel();
        view.ask_approval(
            Approval::ShellCommand {
                command: "rm -rf build".into(),
            },
            reply,
        );
        view.key(key(KeyCode::Char('l')));
        assert!(answer.try_recv().is_err());
        view.key(key(KeyCode::Esc));
        assert_eq!(answer.try_recv().unwrap(), Decision::Denied);
    }

    #[test]
    fn stopping_abandons_the_question_and_asks_first() {
        let mut view = RunView::new("demo", false);
        started(&mut view);
        let (reply, answer) = channel();
        view.ask_approval(
            Approval::Manual {
                message: "go?".into(),
            },
            reply,
        );
        // Asking to stop keeps the question open; saying no goes back to it.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(view.key(ctrl_c), Action::None);
        assert!(matches!(view.modal, Some(Modal::ConfirmStop { .. })));
        assert!(answer.try_recv().is_err());
        assert_eq!(view.key(key(KeyCode::Char('n'))), Action::None);
        assert!(matches!(view.modal, Some(Modal::Approval { .. })));

        // Saying yes answers it as nobody would, and stops.
        view.key(key(KeyCode::Char('q')));
        assert_eq!(view.key(key(KeyCode::Char('y'))), Action::Stop);
        assert_eq!(answer.try_recv().unwrap(), Decision::Deferred);
        assert!(view.modal.is_none());
        assert!(view.stopping);
    }

    #[test]
    fn quitting_a_running_run_needs_a_yes() {
        let mut view = RunView::new("demo", false);
        started(&mut view);
        assert_eq!(view.key(key(KeyCode::Char('q'))), Action::None);
        assert_eq!(view.key(key(KeyCode::Char('n'))), Action::None);
        assert!(view.modal.is_none());
        view.key(key(KeyCode::Char('q')));
        assert_eq!(view.key(key(KeyCode::Char('y'))), Action::Stop);

        view.apply(Event::RunFinished {
            awaiting: None,
            question: None,
            run_id: "run-1".into(),
            status: RunStatus::Canceled,
            error: None,
            failed_files: 0,
            partial_steps: Vec::new(),
            output: serde_json::Value::Null,
            output_omitted: None,
            steps: Vec::new(),
        });
        assert_eq!(view.key(key(KeyCode::Char('q'))), Action::Exit);
    }

    #[test]
    fn the_agent_picker_returns_the_choice_or_the_built_in_agent() {
        let choices = vec![
            AgentChoice {
                name: "claude-code".into(),
                label: "Claude Code".into(),
                available: true,
            },
            AgentChoice {
                name: "gemini".into(),
                label: "Gemini CLI".into(),
                available: false,
            },
            AgentChoice {
                name: "codex".into(),
                label: "Codex".into(),
                available: true,
            },
        ];
        let mut view = RunView::new("demo", false);
        let (reply, answer) = channel();
        view.ask_agent(choices.clone(), reply);
        view.key(key(KeyCode::Down));
        view.key(key(KeyCode::Enter));
        assert_eq!(answer.try_recv().unwrap().as_deref(), Some("codex"));

        let (reply, answer) = channel();
        view.ask_agent(choices, reply);
        for _ in 0..5 {
            view.key(key(KeyCode::Down));
        }
        view.key(key(KeyCode::Enter));
        assert_eq!(answer.try_recv().unwrap(), None);
    }
}
