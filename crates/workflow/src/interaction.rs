//! Questions the engine asks the person running a workflow. The CLI
//! implements this with terminal prompts; [`NonInteractive`] answers without
//! asking, matching the old engine's non-interactive behaviour.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Approval {
    /// `approval(message)` in a workflow (`trigger: manual` in YAML).
    Manual { message: String },
    /// A shell command is about to run.
    ShellCommand { command: String },
    /// A transform asks for runtime capabilities that were not granted.
    Capabilities { modules: Vec<String> },
    /// A pull request is about to be opened.
    PullRequest { title: String, branch: String },
    /// An interrupted step may already have had external effects.
    InterruptedStep { label: String, position: u64 },
    /// The run is about to change the target for the first time, and the
    /// target has uncommitted changes (or is not in a git repository).
    #[serde(rename_all = "camelCase")]
    UncommittedChanges {
        path: String,
        not_a_repository: bool,
    },
}

impl Approval {
    /// The id of the question asking for it: stable across resumes, so
    /// `workflow resume --approve <id>` can name it. `step` is the key of
    /// the step asking.
    pub fn question_id(&self, step: Option<&str>) -> String {
        let step = step.unwrap_or("run");
        match self {
            Approval::Manual { .. } => format!("approval:{step}"),
            Approval::ShellCommand { .. } => format!("shell:{step}"),
            Approval::Capabilities { .. } => format!("capabilities:{step}"),
            Approval::PullRequest { .. } => format!("pull-request:{step}"),
            Approval::InterruptedStep { position, .. } => format!("interrupted-step:{position}"),
            Approval::UncommittedChanges { .. } => "uncommitted-changes".to_string(),
        }
    }

    /// The question to show a person.
    pub fn prompt(&self) -> String {
        match self {
            Approval::Manual { message } => message.clone(),
            Approval::ShellCommand { command } => format!("Run shell command: {command}?"),
            Approval::Capabilities { modules } => {
                format!("Allow the transform to use {}?", modules.join(", "))
            }
            Approval::PullRequest { title, branch } => {
                format!("Push '{branch}' and open pull request '{title}'?")
            }
            Approval::InterruptedStep { label, position } => {
                format!("Step {position} ({label}) may already have run. Rerun it or skip it?")
            }
            Approval::UncommittedChanges {
                path,
                not_a_repository: false,
            } => format!("You have uncommitted changes in {path}. Do you want to proceed anyway?"),
            Approval::UncommittedChanges {
                path,
                not_a_repository: true,
            } => format!(
                "The target path '{path}' is not tracked by Git. Do you want to proceed anyway?"
            ),
        }
    }

    /// What a question nobody answers gets: a manual approval waits for a
    /// resume, the rest are refused.
    pub fn unanswered(&self) -> Decision {
        match self {
            Approval::Manual { .. } | Approval::InterruptedStep { .. } => Decision::Deferred,
            _ => Decision::Denied,
        }
    }

    /// Whether `--approve` gives it. Capabilities are granted up front, and
    /// writing over uncommitted changes takes `--allow-dirty`.
    pub fn given_by_approve_all(&self) -> bool {
        !matches!(
            self,
            Approval::Capabilities { .. }
                | Approval::UncommittedChanges { .. }
                | Approval::InterruptedStep { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approved,
    /// No. For a manual approval, the run ends as rejected.
    Denied,
    /// Nobody can answer now: the run stops and waits to be resumed.
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentChoice {
    pub name: String,
    pub label: String,
    pub available: bool,
}

/// Which question the engine asks, and the step asking it.
#[derive(Debug, Clone, Default)]
pub struct Ask {
    /// See [`Approval::question_id`]; `agent:<step>` for agent questions.
    pub id: String,
    /// The key of the step asking (`"3"`, `"3/0"`).
    pub step: Option<String>,
    /// Set when the asking step must stop (the run was canceled from
    /// elsewhere): a host waiting for an answer gives up then.
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Ask {
    pub fn stopped(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|stop| stop.load(std::sync::atomic::Ordering::SeqCst))
    }
}

pub trait Interaction: Send + Sync {
    fn is_interactive(&self) -> bool;

    fn approve(&self, approval: &Approval) -> Decision;

    /// [`Self::approve`], knowing which question it is: the engine asks
    /// through this. Hosts that track questions by id override it.
    fn ask(&self, _ask: &Ask, approval: &Approval) -> Decision {
        self.approve(approval)
    }

    /// Pick a coding agent for an `agent(...)` step; `None` uses the built-in
    /// one.
    fn choose_agent(&self, _choices: &[AgentChoice]) -> Option<String> {
        None
    }

    /// [`Self::choose_agent`], knowing which question it is: the engine asks
    /// through this.
    fn pick_agent(&self, _ask: &Ask, choices: &[AgentChoice]) -> Option<String> {
        self.choose_agent(choices)
    }
}

/// Answers without asking: manual approvals are deferred, capability
/// requests denied, and shell commands and pull requests allowed.
pub struct NonInteractive {
    /// Treat approvals as given (`--approve`, or pro dry runs). Capabilities
    /// are the exception: they are granted up front, never by a blanket yes.
    pub approve_all: bool,
}

impl Interaction for NonInteractive {
    fn is_interactive(&self) -> bool {
        false
    }

    fn approve(&self, approval: &Approval) -> Decision {
        match approval {
            Approval::InterruptedStep { .. } => Decision::Deferred,
            Approval::Capabilities { .. } | Approval::UncommittedChanges { .. } => Decision::Denied,
            Approval::Manual { .. } if !self.approve_all => Decision::Deferred,
            Approval::Manual { .. }
            | Approval::ShellCommand { .. }
            | Approval::PullRequest { .. } => Decision::Approved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities() -> Approval {
        Approval::Capabilities {
            modules: vec!["child_process".into()],
        }
    }

    #[test]
    fn a_blanket_approval_does_not_grant_capabilities() {
        let all = NonInteractive { approve_all: true };
        assert_eq!(all.approve(&capabilities()), Decision::Denied);
        assert_eq!(
            all.approve(&Approval::Manual {
                message: "go?".into()
            }),
            Decision::Approved
        );
    }

    #[test]
    fn question_ids_name_the_kind_and_the_step() {
        let manual = Approval::Manual {
            message: "go?".into(),
        };
        assert_eq!(manual.question_id(Some("3/0")), "approval:3/0");
        assert_eq!(capabilities().question_id(Some("1")), "capabilities:1");
        assert_eq!(
            Approval::UncommittedChanges {
                path: ".".into(),
                not_a_repository: false
            }
            .question_id(Some("2")),
            "uncommitted-changes"
        );
        assert_eq!(manual.unanswered(), Decision::Deferred);
        assert_eq!(capabilities().unanswered(), Decision::Denied);
    }

    #[test]
    fn unattended_runs_defer_manual_approvals() {
        let none = NonInteractive { approve_all: false };
        assert_eq!(
            none.approve(&Approval::Manual {
                message: "go?".into()
            }),
            Decision::Deferred
        );
        assert_eq!(none.approve(&capabilities()), Decision::Denied);
        assert_eq!(
            none.approve(&Approval::ShellCommand {
                command: "ls".into()
            }),
            Decision::Approved
        );
    }
}
