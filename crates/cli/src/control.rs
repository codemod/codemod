//! The control channel: another program (a TUI, an agent, a script) runs
//! `codemod … --format jsonl --control stdio`, reads events on stdout, and
//! sends commands on stdin, one JSON object per line. The run waits at a
//! question (an approval, an agent to pick) until a command answers it,
//! instead of exiting to be resumed.
//!
//! Out: a [`Hello`] line first, then the run's events (`codemod_workflow::events::Event`),
//! each with `seq` and `ts`. In: [`Command`]s. A command that cannot be
//! followed gets a `control_error` event. Closing stdin pauses the run.
//! `crates/cli/npm/types/protocol.d.ts` declares all of it for TypeScript.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use codemod_workflow::events::{ClosedBy, Event, EventSink, QuestionKind};
use codemod_workflow::interaction::{AgentChoice, Approval, Ask, Decision, Interaction};
use codemod_workflow::{StopHandle, StopMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol's version, in `hello`. It goes up when a change would
/// break a client; new events and fields do not.
pub const PROTOCOL: u32 = 1;

/// `--control`: how the run is driven.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlMode {
    /// Commands as JSON lines on stdin (needs `--format jsonl`)
    Stdio,
}

/// Which approvals are given before anyone is asked: `--approve` gives all
/// of them, `workflow resume --approve <question id>` the ones it names.
/// Capabilities and writing over uncommitted changes are never given this
/// way (see `Approval::given_by_approve_all`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Approvals {
    pub all: bool,
    pub ids: Vec<String>,
}

impl Approvals {
    pub fn gives(&self, id: &str, approval: &Approval) -> bool {
        approval.given_by_approve_all() && (self.all || self.ids.iter().any(|given| given == id))
    }
}

/// The first line a client reads: from a run with `--control stdio`, or
/// from `workflow attach`. It is not one of the run's events: it has no
/// `seq`, and the journal does not keep it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hello {
    /// Always `"hello"`.
    pub event: &'static str,
    pub protocol: u32,
    /// The CLI's version.
    pub cli: &'static str,
    pub run_id: String,
    /// `"run"`, `"resume"` or `"attach"`.
    pub mode: &'static str,
    pub dry_run: bool,
    pub workflow: PathBuf,
    pub target: PathBuf,
    pub settings: HelloSettings,
    /// The commands this process reads: none for `attach`.
    pub commands: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HelloSettings {
    pub capabilities: Vec<String>,
    pub managed_git: bool,
    pub agent: Option<String>,
    /// `provider/model`, for `codemod/assess`.
    pub decision_model: Option<String>,
    /// `--approve`: every approval is given.
    pub approve_all: bool,
    /// The questions `workflow resume --approve <id>` gave.
    pub approved: Vec<String>,
}

impl Hello {
    pub fn print(&self) {
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(
            stdout,
            "{}",
            serde_json::to_string(self).unwrap_or_default()
        );
        let _ = stdout.flush();
    }
}

/// The commands a run with a control channel reads.
pub const COMMANDS: &[&str] = &["answer", "approve", "reject", "stop"];

/// One line on stdin. `questionId` is accepted for `question_id`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Answer a question: `"approve"`, `"reject"` or `"later"` (`true` and
    /// `false` too) for an approval; an agent's id, or `null` for the
    /// built-in one, for an agent question.
    Answer {
        #[serde(alias = "questionId")]
        question_id: String,
        value: Value,
    },
    Approve {
        #[serde(alias = "questionId")]
        question_id: String,
    },
    Reject {
        #[serde(alias = "questionId")]
        question_id: String,
    },
    /// Stop the run: `"pause"` (the default) leaves it to resume,
    /// `"cancel"` ends it.
    Stop {
        #[serde(default)]
        mode: Mode,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Pause,
    Cancel,
}

/// How a question was answered.
enum Reply {
    Decision(Decision, ClosedBy),
    Agent(Option<String>, ClosedBy),
}

struct Pending {
    asked: Asked,
    reply: Sender<Reply>,
}

#[derive(Clone)]
enum Asked {
    Approval(Approval),
    Agent(Vec<AgentChoice>),
}

impl Asked {
    /// The reply to a question nobody answers.
    fn unanswered(&self, by: ClosedBy) -> Reply {
        match self {
            Asked::Approval(approval) => Reply::Decision(approval.unanswered(), by),
            Asked::Agent(_) => Reply::Agent(None, by),
        }
    }
}

/// Asks over the control channel: emits a `question`, then waits on the
/// engine's thread for the command that answers it.
pub(crate) struct Control {
    events: Arc<dyn EventSink>,
    approvals: Approvals,
    pending: Mutex<HashMap<String, Pending>>,
    /// Set once the run was told to stop or stdin closed: what comes up
    /// after that is not asked.
    closed: Mutex<Option<ClosedBy>>,
    /// Steps running at the same time ask one at a time.
    asking: Mutex<()>,
    stop: Mutex<Option<StopHandle>>,
}

impl Control {
    pub fn new(events: Arc<dyn EventSink>, approvals: Approvals) -> Self {
        Self {
            events,
            approvals,
            pending: Mutex::new(HashMap::new()),
            closed: Mutex::new(None),
            asking: Mutex::new(()),
            stop: Mutex::new(None),
        }
    }

    /// Read commands from stdin, for the engine `stop` stops.
    pub fn listen(self: &Arc<Self>, stop: StopHandle) {
        *self.stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
        let control = Arc::clone(self);
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if !line.trim().is_empty() {
                    control.handle(&line);
                }
            }
            control.halt(StopMode::Pause, ClosedBy::Eof);
        });
    }

    /// Follow one command line.
    pub fn handle(&self, line: &str) {
        let command = match serde_json::from_str::<Command>(line) {
            Ok(command) => command,
            Err(error) => return self.error(format!("not a command: {error}"), line),
        };
        let (id, answer) = match command {
            Command::Stop { mode } => {
                let mode = match mode {
                    Mode::Pause => StopMode::Pause,
                    Mode::Cancel => StopMode::Cancel,
                };
                return self.halt(mode, ClosedBy::Stop);
            }
            Command::Approve { question_id } => (question_id, Value::from("approve")),
            Command::Reject { question_id } => (question_id, Value::from("reject")),
            Command::Answer { question_id, value } => (question_id, value),
        };
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let Some(question) = pending.get(&id) else {
            drop(pending);
            return self.error(format!("no question '{id}' is open"), line);
        };
        let reply = match reply_to(&question.asked, &answer) {
            Ok(reply) => reply,
            Err(message) => {
                drop(pending);
                return self.error(format!("cannot answer '{id}': {message}"), line);
            }
        };
        if let Some(question) = pending.remove(&id) {
            let _ = question.reply.send(reply);
        }
    }

    /// Stop the run, then close the questions open, unanswered.
    fn halt(&self, mode: StopMode, by: ClosedBy) {
        // The stop comes first, so a question closed by it ends the run as
        // stopped, not as waiting for an approval.
        if let Some(stop) = self.stop.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            stop.stop_with(mode);
        }
        self.closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(by);
        let open: Vec<Pending> = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, pending)| pending)
            .collect();
        for pending in open {
            let _ = pending.reply.send(pending.asked.unanswered(by));
        }
    }

    fn error(&self, message: String, line: &str) {
        self.events.emit(Event::ControlError {
            message,
            line: line.to_string(),
        });
    }

    /// Ask `asked` as question `ask`, and wait for the reply.
    fn question(&self, ask: &Ask, asked: Asked) -> Reply {
        let _turn = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(by) = *self.closed.lock().unwrap_or_else(|e| e.into_inner()) {
            return asked.unanswered(by);
        }
        let (reply, answer) = channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                ask.id.clone(),
                Pending {
                    asked: asked.clone(),
                    reply,
                },
            );
        let (kind, message, approval, choices) = match &asked {
            Asked::Approval(approval) => (
                QuestionKind::Approval,
                approval.prompt(),
                Some(approval.clone()),
                None,
            ),
            Asked::Agent(choices) => (
                QuestionKind::Agent,
                "Which coding agent should run this step?".to_string(),
                None,
                Some(choices.clone()),
            ),
        };
        self.events.emit(Event::Question {
            id: ask.id.clone(),
            kind,
            step: ask.step.clone(),
            message,
            approval,
            choices,
        });
        // Also stop waiting when the run is stopped from elsewhere
        // (`workflow cancel` in another terminal).
        let reply = loop {
            match answer.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(reply) => break reply,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !ask.stopped() => {}
                Err(_) => {
                    self.pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&ask.id);
                    break asked.unanswered(ClosedBy::Stop);
                }
            }
        };
        let (answer, by) = match &reply {
            Reply::Decision(decision, by) => (
                Value::from(match decision {
                    Decision::Approved => "approved",
                    Decision::Denied => "rejected",
                    Decision::Deferred => "later",
                }),
                *by,
            ),
            Reply::Agent(agent, by) => (agent.clone().map_or(Value::Null, Value::from), *by),
        };
        self.events.emit(Event::QuestionClosed {
            id: ask.id.clone(),
            answer,
            by,
        });
        reply
    }
}

/// The reply `answer` gives a question, or why it cannot.
fn reply_to(asked: &Asked, answer: &Value) -> Result<Reply, String> {
    let by = ClosedBy::Answer;
    match asked {
        Asked::Approval(approval) => {
            let decision = match answer {
                Value::String(text) if text == "approve" => Decision::Approved,
                Value::Bool(true) => Decision::Approved,
                Value::String(text) if text == "reject" => Decision::Denied,
                Value::Bool(false) => Decision::Denied,
                Value::String(text) if text == "later" => {
                    if approval.unanswered() != Decision::Deferred {
                        return Err(
                            "only an approval(...) or interrupted step can wait until later".into(),
                        );
                    }
                    Decision::Deferred
                }
                _ => return Err(r#"answer "approve", "reject" or "later""#.into()),
            };
            Ok(Reply::Decision(decision, by))
        }
        Asked::Agent(choices) => match answer {
            Value::Null => Ok(Reply::Agent(None, by)),
            Value::String(name) if choices.iter().any(|c| c.available && c.name == *name) => {
                Ok(Reply::Agent(Some(name.clone()), by))
            }
            _ => Err("answer the id of an available agent, or null for the built-in one".into()),
        },
    }
}

impl Interaction for Control {
    fn is_interactive(&self) -> bool {
        true
    }

    fn approve(&self, approval: &Approval) -> Decision {
        let ask = Ask {
            id: approval.question_id(None),
            ..Ask::default()
        };
        self.ask(&ask, approval)
    }

    fn ask(&self, ask: &Ask, approval: &Approval) -> Decision {
        if self.approvals.gives(&ask.id, approval) {
            return Decision::Approved;
        }
        match self.question(ask, Asked::Approval(approval.clone())) {
            Reply::Decision(decision, _) => decision,
            Reply::Agent(..) => approval.unanswered(),
        }
    }

    fn pick_agent(&self, ask: &Ask, choices: &[AgentChoice]) -> Option<String> {
        if !choices.iter().any(|choice| choice.available) {
            return None;
        }
        match self.question(ask, Asked::Agent(choices.to_vec())) {
            Reply::Agent(agent, _) => agent,
            Reply::Decision(..) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemod_workflow::events::CollectEvents;

    fn manual() -> Approval {
        Approval::Manual {
            message: "Ship it?".into(),
        }
    }

    fn ask(id: &str) -> Ask {
        Ask {
            id: id.into(),
            step: Some("1".into()),
            stop: None,
        }
    }

    #[test]
    fn a_question_stops_waiting_when_its_step_is_stopped() {
        let (control, events) = control();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let asking = Arc::clone(&control);
        let flag = Arc::clone(&stop);
        let answer = std::thread::spawn(move || {
            asking.ask(
                &Ask {
                    stop: Some(flag),
                    ..ask("approval:1")
                },
                &manual(),
            )
        });
        while !control.pending.lock().unwrap().contains_key("approval:1") {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(answer.join().unwrap(), Decision::Deferred);
        assert!(control.pending.lock().unwrap().is_empty());
        assert!(matches!(
            events.take().last(),
            Some(Event::QuestionClosed {
                by: ClosedBy::Stop,
                ..
            })
        ));
    }

    /// Ask on another thread; answer with `commands` once the question is out.
    fn asked(
        control: &Arc<Control>,
        events: &CollectEvents,
        commands: &[&str],
    ) -> (Decision, Vec<Event>) {
        let asking = Arc::clone(control);
        let answer = std::thread::spawn(move || asking.ask(&ask("approval:1"), &manual()));
        while !control.pending.lock().unwrap().contains_key("approval:1") {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for command in commands {
            control.handle(command);
        }
        let decision = answer.join().unwrap();
        (decision, events.take())
    }

    fn control() -> (Arc<Control>, Arc<CollectEvents>) {
        let events = Arc::new(CollectEvents::default());
        (
            Arc::new(Control::new(events.clone(), Approvals::default())),
            events,
        )
    }

    #[test]
    fn an_approval_waits_for_its_answer() {
        let (control, events) = control();
        let (decision, events) = asked(
            &control,
            &events,
            &[
                r#"{"command":"approve","question_id":"nope"}"#,
                "not json",
                r#"{"command":"answer","questionId":"approval:1","value":"approve"}"#,
            ],
        );
        assert_eq!(decision, Decision::Approved);
        assert!(
            matches!(&events[0], Event::Question { id, step, .. } if id == "approval:1" && step.as_deref() == Some("1"))
        );
        assert!(
            matches!(&events[1], Event::ControlError { message, .. } if message.contains("'nope'"))
        );
        assert!(matches!(&events[2], Event::ControlError { .. }));
        assert!(matches!(
            &events[3],
            Event::QuestionClosed { answer, by: ClosedBy::Answer, .. } if answer == "approved"
        ));
    }

    #[test]
    fn reject_and_later_answer_an_approval() {
        let (control, events) = control();
        let (decision, _) = asked(
            &control,
            &events,
            &[r#"{"command":"reject","question_id":"approval:1"}"#],
        );
        assert_eq!(decision, Decision::Denied);
        let (decision, events) = asked(
            &control,
            &events,
            &[r#"{"command":"answer","question_id":"approval:1","value":"later"}"#],
        );
        assert_eq!(decision, Decision::Deferred);
        assert!(matches!(&events[1], Event::QuestionClosed { answer, .. } if answer == "later"));
    }

    #[test]
    fn stopping_closes_the_open_question_and_the_ones_after_it() {
        let (control, events) = control();
        let (decision, events_now) = asked(
            &control,
            &events,
            &[r#"{"command":"stop","mode":"cancel"}"#],
        );
        assert_eq!(decision, Decision::Deferred);
        assert!(matches!(
            &events_now[1],
            Event::QuestionClosed {
                by: ClosedBy::Stop,
                ..
            }
        ));
        // Asked after the stop: not asked at all.
        assert_eq!(
            control.ask(
                &ask("shell:2"),
                &Approval::ShellCommand {
                    command: "ls".into()
                }
            ),
            Decision::Denied
        );
        assert!(events.take().is_empty());
    }

    #[test]
    fn given_approvals_are_not_asked() {
        let events = Arc::new(CollectEvents::default());
        let control = Control::new(
            events.clone(),
            Approvals {
                all: false,
                ids: vec!["approval:1".into()],
            },
        );
        assert_eq!(
            control.ask(&ask("approval:1"), &manual()),
            Decision::Approved
        );
        assert!(events.take().is_empty());
    }

    #[test]
    fn an_agent_question_takes_an_available_agent_or_null() {
        let (control, events) = control();
        let choices = vec![
            AgentChoice {
                name: "codex".into(),
                label: "Codex".into(),
                available: true,
            },
            AgentChoice {
                name: "aider".into(),
                label: "Aider".into(),
                available: false,
            },
        ];
        let asking = Arc::clone(&control);
        let picked = std::thread::spawn(move || asking.pick_agent(&ask("agent:1"), &choices));
        while !control.pending.lock().unwrap().contains_key("agent:1") {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        control.handle(r#"{"command":"answer","question_id":"agent:1","value":"aider"}"#);
        control.handle(r#"{"command":"approve","question_id":"agent:1"}"#);
        control.handle(r#"{"command":"answer","question_id":"agent:1","value":"codex"}"#);
        assert_eq!(picked.join().unwrap().as_deref(), Some("codex"));
        let events = events.take();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::ControlError { .. }))
                .count(),
            2
        );
    }
}

/// Writes `npm/type-tests/protocol-samples.ts`: one of every event, a
/// `hello` and every command, as the CLI prints and reads them. The npm
/// package's type tests check them against `codemod/protocol`, so the
/// TypeScript declarations cannot drift from these types unnoticed.
#[cfg(test)]
mod protocol_samples {
    use std::sync::{Arc, Mutex};

    use butterflow_state::run_store::{MemoryStore, RunStore};
    use codemod_workflow::events::{
        ClosedBy, Event, EventSink, Omitted, PartialStep, QuestionKind, Settings, StepAt,
        StepResult,
    };
    use codemod_workflow::interaction::{AgentChoice, Approval};
    use codemod_workflow::{RunStatus, StepStatus};
    use serde_json::{json, Value};

    use super::{Command, Hello, HelloSettings, COMMANDS, PROTOCOL};
    use crate::journal::{Recorder, Stamped};

    const FILE: &str = "npm/type-tests/protocol-samples.ts";

    #[derive(Default)]
    struct Kept(Mutex<Vec<Value>>);

    impl Stamped for Kept {
        fn emit(&self, _event: Event, record: &Value) {
            let mut record = record.clone();
            record["ts"] = json!("2026-01-01T00:00:00+00:00");
            self.0.lock().unwrap().push(record);
        }
    }

    fn nested(position: u64) -> StepAt {
        StepAt {
            position,
            step: format!("2/{position}"),
            parent: Some("2".into()),
        }
    }

    /// One of every event. The match below names every variant: a new one
    /// does not compile until it has a sample here, and a declaration in
    /// `protocol.d.ts`.
    fn every_event() -> Vec<Event> {
        let events = vec![
            Event::RunStarted {
                run_id: "8c7000b1".into(),
                workflow: "/work/codemod/workflow.ts".into(),
                resumed: false,
                target: "/work/app".into(),
                params: json!({ "level": 2 }),
                dry_run: false,
                settings: Settings {
                    capabilities: vec!["fs".into()],
                    managed_git: false,
                    agent: Some("claude-code".into()),
                    host: Some("quickjs".into()),
                },
            },
            Event::StepStarted {
                at: StepAt::top(0),
                label: "Replace".into(),
                kind: Some("forEachFile".into()),
            },
            Event::StepReplayed {
                at: StepAt::top(1),
                label: "Find".into(),
                kind: None,
            },
            Event::StepProgress {
                at: StepAt::top(0),
                processed: 1,
                total: 2,
            },
            Event::StepLog {
                at: nested(0),
                line: "src/a.ts".into(),
            },
            Event::RunLog {
                line: "from the body".into(),
                step: None,
            },
            Event::StepAgent {
                at: StepAt::top(3),
                agent: "codex".into(),
                activity: codemod_agents::AgentEvent::Text {
                    text: "Done.".into(),
                    delta: false,
                },
            },
            Event::StepRetrying {
                at: StepAt::top(0),
                label: "Replace".into(),
                attempt: 1,
                attempts: 3,
                error: "boom".into(),
            },
            Event::StepDiff {
                at: StepAt::top(0),
                path: "src/a.ts".into(),
                diff: "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-var a = 1;\n+let a = 1;\n"
                    .into(),
                additions: 1,
                deletions: 1,
                diff_omitted: None,
            },
            Event::StepDiff {
                at: StepAt::top(0),
                path: "big.json".into(),
                diff: String::new(),
                additions: 9000,
                deletions: 0,
                diff_omitted: Some(Omitted {
                    bytes: 300_000,
                    limit: codemod_workflow::events::STEP_DIFF_LIMIT,
                }),
            },
            Event::StepFinished {
                at: StepAt::top(0),
                label: "Replace".into(),
                kind: Some("forEachFile".into()),
                status: StepStatus::Partial,
                error: Some("1 of 2 files failed".into()),
                duration_ms: 12,
            },
            Event::Question {
                id: "approval:1".into(),
                kind: QuestionKind::Approval,
                step: Some("1".into()),
                message: "Ship it?".into(),
                approval: Some(Approval::Manual {
                    message: "Ship it?".into(),
                }),
                choices: None,
            },
            Event::Question {
                id: "uncommitted-changes".into(),
                kind: QuestionKind::Approval,
                step: None,
                message:
                    "You have uncommitted changes in /work/app. Do you want to proceed anyway?"
                        .into(),
                approval: Some(Approval::UncommittedChanges {
                    path: "/work/app".into(),
                    not_a_repository: false,
                }),
                choices: None,
            },
            Event::Question {
                id: "agent:3".into(),
                kind: QuestionKind::Agent,
                step: Some("3".into()),
                message: "Which coding agent should run this step?".into(),
                approval: None,
                choices: Some(vec![AgentChoice {
                    name: "codex".into(),
                    label: "Codex".into(),
                    available: true,
                }]),
            },
            Event::QuestionClosed {
                id: "approval:1".into(),
                answer: json!("approved"),
                by: ClosedBy::Answer,
            },
            Event::QuestionClosed {
                id: "agent:3".into(),
                answer: Value::Null,
                by: ClosedBy::Eof,
            },
            Event::ControlError {
                message: "no question 'approval:9' is open".into(),
                line: r#"{"command":"approve","question_id":"approval:9"}"#.into(),
            },
            Event::RunDiff {
                path: "src/a.ts".into(),
                diff: "@@ -1 +1 @@\n-var a = 1;\n+let a = 1;\n".into(),
                additions: 1,
                deletions: 1,
            },
            Event::RunFinished {
                run_id: "8c7000b1".into(),
                status: RunStatus::Rejected,
                error: Some("rejected: Ship it?".into()),
                awaiting: None,
                question: None,
                failed_files: 1,
                partial_steps: vec![PartialStep {
                    position: 0,
                    label: "Replace".into(),
                    failed_files: 1,
                    files: 2,
                }],
                output: Value::Null,
                output_omitted: None,
                steps: vec![StepResult {
                    position: 0,
                    label: "Replace".into(),
                    status: StepStatus::Partial,
                    result: json!({ "written": ["src/a.ts"] }),
                    result_omitted: None,
                }],
            },
        ];
        for event in &events {
            match event {
                Event::RunStarted { .. }
                | Event::StepStarted { .. }
                | Event::StepReplayed { .. }
                | Event::StepProgress { .. }
                | Event::StepLog { .. }
                | Event::RunLog { .. }
                | Event::StepAgent { .. }
                | Event::StepRetrying { .. }
                | Event::StepFinished { .. }
                | Event::StepDiff { .. }
                | Event::Question { .. }
                | Event::QuestionClosed { .. }
                | Event::ControlError { .. }
                | Event::RunDiff { .. }
                | Event::RunFinished { .. } => {}
            }
        }
        events
    }

    fn every_command() -> Vec<Value> {
        let commands = vec![
            json!({ "command": "approve", "question_id": "approval:1" }),
            json!({ "command": "reject", "question_id": "approval:1" }),
            json!({ "command": "answer", "question_id": "approval:1", "value": "later" }),
            json!({ "command": "answer", "question_id": "agent:3", "value": "codex" }),
            json!({ "command": "answer", "question_id": "agent:3", "value": null }),
            json!({ "command": "stop", "mode": "cancel" }),
            json!({ "command": "stop" }),
        ];
        let mut names = Vec::new();
        for command in &commands {
            let parsed: Command = serde_json::from_value(command.clone()).unwrap();
            names.push(match parsed {
                Command::Answer { .. } => "answer",
                Command::Approve { .. } => "approve",
                Command::Reject { .. } => "reject",
                Command::Stop { .. } => "stop",
            });
        }
        names.sort();
        names.dedup();
        assert_eq!(names, COMMANDS, "a sample for every command");
        commands
    }

    fn generated() -> String {
        let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
        let kept = Arc::new(Kept::default());
        let recorder = Recorder::new(store, None, kept.clone());
        for event in every_event() {
            recorder.emit(event);
        }
        let hello = Hello {
            event: "hello",
            protocol: PROTOCOL,
            cli: "2.0.0",
            run_id: "8c7000b1".into(),
            mode: "run",
            dry_run: false,
            workflow: "/work/codemod/workflow.ts".into(),
            target: "/work/app".into(),
            settings: HelloSettings {
                capabilities: vec!["fs".into()],
                managed_git: false,
                agent: None,
                decision_model: Some("systemone/jev-latest".into()),
                approve_all: false,
                approved: vec!["approval:1".into()],
            },
            commands: COMMANDS.to_vec(),
        };
        let pretty = |value: &Value| serde_json::to_string_pretty(value).unwrap();
        format!(
            "// Generated from the Rust types by `cargo test -p codemod protocol_samples`\n\
             // (UPDATE_PROTOCOL_SAMPLES=1 rewrites it); do not edit. `tsc -p type-tests`\n\
             // checks it against `codemod/protocol`.\n\n\
             import type {{ Command, Event, Hello }} from \"codemod/protocol\";\n\n\
             export const hello: Hello = {};\n\n\
             export const events: Event[] = {};\n\n\
             export const commands: Command[] = {};\n",
            pretty(&serde_json::to_value(&hello).unwrap()),
            pretty(&Value::from(kept.0.lock().unwrap().clone())),
            pretty(&Value::from(every_command())),
        )
    }

    #[test]
    fn protocol_samples_match_the_rust_types() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FILE);
        let expected = generated();
        if std::env::var_os("UPDATE_PROTOCOL_SAMPLES").is_some() {
            std::fs::write(&path, &expected).unwrap();
            return;
        }
        let found = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            found == expected,
            "{FILE} is out of date: run UPDATE_PROTOCOL_SAMPLES=1 cargo test -p codemod protocol_samples, then update crates/cli/npm/types/modules/protocol.d.ts until `pnpm --filter codemod typecheck` passes"
        );
    }

    #[test]
    fn statuses_in_the_samples_are_the_ones_the_store_knows() {
        // Keeps `RunStatus` in protocol.d.ts honest too.
        for status in [
            RunStatus::Running,
            RunStatus::AwaitingApproval,
            RunStatus::Paused,
            RunStatus::Succeeded,
            RunStatus::Failed,
            RunStatus::Canceled,
            RunStatus::Rejected,
        ] {
            let declared = include_str!("../npm/types/modules/protocol.d.ts");
            assert!(
                declared.contains(&format!("\"{}\"", status.as_str())),
                "{} is not in protocol.d.ts",
                status.as_str()
            );
        }
    }
}
