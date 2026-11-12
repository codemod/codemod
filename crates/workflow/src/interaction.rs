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
    /// Treat every approval as given (`--approve`, or pro dry runs).
    pub approve_all: bool,
}

impl Interaction for NonInteractive {
    fn is_interactive(&self) -> bool {
        false
    }

    fn approve(&self, approval: &Approval) -> Decision {
        if self.approve_all {
            return Decision::Approved;
        }
        match approval {
            Approval::Manual { .. } => Decision::Deferred,
            Approval::Capabilities { .. } => Decision::Denied,
            Approval::ShellCommand { .. } | Approval::PullRequest { .. } => Decision::Approved,
        }
    }
}
