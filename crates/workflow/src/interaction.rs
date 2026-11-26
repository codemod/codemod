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
    /// An `agent(...)` step is about to hand its prompt to a coding agent
    /// (or to the built-in one, at `endpoint`): with full autonomy the agent
    /// edits files and runs commands without asking.
    #[serde(rename_all = "camelCase")]
    Agent {
        /// The agent's label ("Claude Code", "the built-in agent").
        agent: String,
        prompt: String,
        /// `"full"` or `"read-only"`.
        autonomy: String,
        /// Where the built-in agent sends the prompt and its API key.
        #[serde(skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
    },
    /// An `http(...)` step is about to send a request. `env` names the
    /// environment variables its headers read.
    HttpRequest {
        method: String,
        url: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        env: Vec<String>,
    },
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
            // `agent:<step>` is the question that picks the agent.
            Approval::Agent { .. } => format!("agent-run:{step}"),
            Approval::HttpRequest { .. } => format!("http:{step}"),
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
            Approval::Agent {
                agent,
                prompt,
                autonomy,
                endpoint,
            } => {
                let access = if autonomy == "read-only" {
                    "read-only"
                } else {
                    "it edits files and runs commands without asking"
                };
                let at = endpoint
                    .as_ref()
                    .map(|endpoint| format!(" at {endpoint}"))
                    .unwrap_or_default();
                format!(
                    "Run {agent}{at} ({access}) with this prompt: {}?",
                    excerpt(prompt, PROMPT_EXCERPT)
                )
            }
            Approval::HttpRequest { method, url, env } => {
                if env.is_empty() {
                    format!("Send {method} {url}?")
                } else {
                    format!(
                        "Send {method} {url}, with the environment variable{} {} in its headers?",
                        if env.len() == 1 { "" } else { "s" },
                        env.join(", ")
                    )
                }
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

    /// What `--approve=<scope>` names it by: `shell`, `agent`, `http`,
    /// `pull-request`, and `approval` for manual approvals. `None` for what
    /// `--approve` never gives.
    pub fn scope(&self) -> Option<&'static str> {
        match self {
            Approval::Manual { .. } => Some("approval"),
            Approval::ShellCommand { .. } => Some("shell"),
            Approval::Agent { .. } => Some("agent"),
            Approval::HttpRequest { .. } => Some("http"),
            Approval::PullRequest { .. } => Some("pull-request"),
            Approval::Capabilities { .. }
            | Approval::UncommittedChanges { .. }
            | Approval::InterruptedStep { .. } => None,
        }
    }

    /// A step that runs or sends something asks it: a shell command, a
    /// coding agent, a request, a pull request. An unattended run of the
    /// person's own workflow does these without asking
    /// ([`Ask::trusted`]); a registry package's need `--approve`.
    pub fn runs_or_sends(&self) -> bool {
        matches!(
            self,
            Approval::ShellCommand { .. }
                | Approval::Agent { .. }
                | Approval::HttpRequest { .. }
                | Approval::PullRequest { .. }
        )
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

/// How much of an agent's prompt a question shows.
const PROMPT_EXCERPT: usize = 300;

/// The first `limit` characters of `text` on one line, with `…` when cut.
pub fn excerpt(text: &str, limit: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= limit {
        return line;
    }
    let mut cut: String = line.chars().take(limit).collect();
    cut.push('…');
    cut
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
    /// The step is of the person's own workflow (a local file or directory
    /// they ran), not of a package from a registry, nor of a registry package
    /// nested in theirs. With nobody to ask, what such a step runs or sends
    /// ([`Approval::runs_or_sends`]) goes ahead; a host that can ask still
    /// asks.
    pub trusted: bool,
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

/// Answers without asking: manual approvals are deferred, and everything
/// else is refused: capability requests, and what a workflow would do to the
/// machine or send from it (shell commands, coding agents, HTTP requests,
/// pull requests). `approve_all` gives the latter and manual approvals.
pub struct NonInteractive {
    /// Treat approvals as given (`--approve`, or pro dry runs). Capabilities
    /// are the exception: they are granted up front, never by a blanket yes.
    pub approve_all: bool,
}

impl Interaction for NonInteractive {
    fn is_interactive(&self) -> bool {
        false
    }

    fn ask(&self, ask: &Ask, approval: &Approval) -> Decision {
        if ask.trusted && approval.runs_or_sends() {
            return Decision::Approved;
        }
        self.approve(approval)
    }

    fn approve(&self, approval: &Approval) -> Decision {
        // Nobody is here to see what a package's workflow is about to run:
        // without `--approve`, a registry package must not get to run
        // commands or send requests on a CI machine.
        if self.approve_all && approval.given_by_approve_all() {
            Decision::Approved
        } else {
            approval.unanswered()
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
        for approval in [
            Approval::ShellCommand {
                command: "ls".into(),
            },
            Approval::PullRequest {
                title: "Update".into(),
                branch: "update".into(),
            },
            Approval::Agent {
                agent: "Claude Code".into(),
                prompt: "Migrate".into(),
                autonomy: "full".into(),
                endpoint: None,
            },
            Approval::HttpRequest {
                method: "POST".into(),
                url: "https://example.com/".into(),
                env: vec!["TOKEN".into()],
            },
        ] {
            // What a package would run or send is never done unattended
            // without `--approve`.
            assert_eq!(none.approve(&approval), Decision::Denied, "{approval:?}");
            assert_eq!(
                NonInteractive { approve_all: true }.approve(&approval),
                Decision::Approved,
                "{approval:?}"
            );
        }
    }

    #[test]
    fn an_unattended_run_of_ones_own_workflow_runs_its_steps() {
        let none = NonInteractive { approve_all: false };
        let own = Ask {
            trusted: true,
            ..Ask::default()
        };
        let shell = Approval::ShellCommand {
            command: "ls".into(),
        };
        let http = Approval::HttpRequest {
            method: "GET".into(),
            url: "https://example.com/".into(),
            env: Vec::new(),
        };
        for approval in [&shell, &http] {
            assert_eq!(none.ask(&own, approval), Decision::Approved);
            // A package's steps, and a registry package nested in one's own.
            assert_eq!(none.ask(&Ask::default(), approval), Decision::Denied);
        }
        // Trust is not an answer to a manual approval, nor a capability.
        let manual = Approval::Manual {
            message: "go?".into(),
        };
        assert_eq!(none.ask(&own, &manual), Decision::Deferred);
        assert_eq!(none.ask(&own, &capabilities()), Decision::Denied);
        assert_eq!(shell.scope(), Some("shell"));
        assert_eq!(http.scope(), Some("http"));
        assert_eq!(capabilities().scope(), None);
    }

    #[test]
    fn agent_and_http_questions_say_what_would_run() {
        let agent = Approval::Agent {
            agent: "the built-in agent".into(),
            prompt: format!("Do\n  this {}", "x".repeat(400)),
            autonomy: "full".into(),
            endpoint: Some("https://llm.example".into()),
        };
        assert_eq!(agent.question_id(Some("2")), "agent-run:2");
        let prompt = agent.prompt();
        assert!(
            prompt.starts_with("Run the built-in agent at https://llm.example (it edits files and runs commands without asking) with this prompt: Do this xxx"),
            "{prompt}"
        );
        assert!(prompt.ends_with("…?"), "{prompt}");
        assert_eq!(
            serde_json::to_value(&agent).unwrap()["kind"],
            serde_json::json!("agent")
        );

        let http = Approval::HttpRequest {
            method: "POST".into(),
            url: "https://example.com/hook".into(),
            env: vec!["GITHUB_TOKEN".into()],
        };
        assert_eq!(http.question_id(Some("0")), "http:0");
        assert_eq!(
            http.prompt(),
            "Send POST https://example.com/hook, with the environment variable GITHUB_TOKEN in its headers?"
        );
        assert_eq!(
            serde_json::to_value(&http).unwrap(),
            serde_json::json!({
                "kind": "http-request",
                "method": "POST",
                "url": "https://example.com/hook",
                "env": ["GITHUB_TOKEN"],
            })
        );
    }
}
