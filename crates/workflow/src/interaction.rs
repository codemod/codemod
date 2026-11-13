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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approved,
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

pub trait Interaction: Send + Sync {
    fn is_interactive(&self) -> bool;

    fn approve(&self, approval: &Approval) -> Decision;

    /// Pick a coding agent for an `agent(...)` step; `None` uses the built-in
    /// one.
    fn choose_agent(&self, _choices: &[AgentChoice]) -> Option<String> {
        None
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
            Approval::Capabilities { .. } => Decision::Denied,
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
