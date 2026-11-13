//! Coding agents, launched headlessly.
//!
//! Each agent (Claude Code, Codex, Gemini CLI, ...) has an [`Adapter`] that
//! knows how to start it for one task without asking questions, and how to
//! read its output. A [`Run`] starts the agent, streams what it does as
//! [`AgentEvent`]s, and returns its final answer, session and usage.
//!
//! ```no_run
//! use codemod_agents::{find, Autonomy, Run, Task};
//!
//! let agent = find("codex").unwrap();
//! let installed = agent.installed().expect("codex is not installed");
//! let outcome = Run {
//!     adapter: agent.adapter,
//!     executable: installed,
//!     task: Task {
//!         prompt: "Rename foo to bar in src/",
//!         system_prompt: None,
//!         model: None,
//!         cwd: std::path::Path::new("."),
//!         autonomy: Autonomy::Full,
//!     },
//!     cancel: None,
//!     timeout: None,
//! }
//! .run(&mut |event| println!("{event:?}"))
//! .unwrap();
//! println!("{}", outcome.result.unwrap_or_default());
//! ```
//!
//! [`parent_agent`] tells whether this program itself runs inside one.

mod adapter;
pub mod adapters;
mod detect;
mod event;
pub mod probe;
mod run;
mod which;

use std::path::PathBuf;

pub use adapter::{
    json_line, str_at, with_instructions, Adapter, Autonomy, Launch, Parser, Task, TextParser,
    Transcript,
};
pub use adapters::ADAPTERS;
pub use detect::{parent_agent, Evidence, ParentAgent};
pub use event::{AgentEvent, Stream, Usage};
pub use run::{Outcome, Run, RunError};
pub use which::which;

/// A supported agent.
#[derive(Clone, Copy)]
pub struct Agent {
    pub adapter: &'static dyn Adapter,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Agent").field(&self.adapter.id()).finish()
    }
}

impl Agent {
    pub fn id(&self) -> &'static str {
        self.adapter.id()
    }

    pub fn label(&self) -> &'static str {
        self.adapter.label()
    }

    /// Where the agent's executable is, when it is installed.
    pub fn installed(&self) -> Option<PathBuf> {
        self.adapter
            .executables()
            .iter()
            .find_map(|name| which(name))
    }
}

/// Every supported agent.
pub fn agents() -> impl Iterator<Item = Agent> {
    ADAPTERS.iter().map(|adapter| Agent { adapter: *adapter })
}

/// The agent with this id or alias (case-insensitive).
pub fn find(name: &str) -> Option<Agent> {
    let name = name.trim().to_ascii_lowercase();
    agents().find(|agent| {
        agent.id() == name
            || agent.adapter.aliases().contains(&name.as_str())
            || agent.adapter.executables().contains(&name.as_str())
    })
}

/// The supported agents that are installed, with their executables.
pub fn installed() -> Vec<(Agent, PathBuf)> {
    agents()
        .filter_map(|agent| agent.installed().map(|path| (agent, path)))
        .collect()
}
