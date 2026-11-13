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
//! let outcome = Run {
//!     adapter: agent,
//!     executable: agent.installed().expect("codex is not installed"),
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
mod adapters;
mod detect;
mod event;
mod parse;
mod probe;
mod run;
mod transcript;
mod which;

use std::path::PathBuf;

pub use adapter::{Adapter, Autonomy, ReadOnly, Task};
pub use detect::{agents_in_env, parent_agent, Evidence, ParentAgent};
pub use event::{AgentEvent, FileChange, FileChangeKind, Stream, Usage};
pub use run::{Outcome, Run, RunError};

/// Every supported agent.
pub fn agents() -> impl Iterator<Item = &'static dyn Adapter> {
    adapters::ADAPTERS.iter().copied()
}

/// The agent with this id or alias (case-insensitive).
pub fn find(name: &str) -> Option<&'static dyn Adapter> {
    let name = name.trim().to_ascii_lowercase();
    agents().find(|agent| {
        agent.id() == name
            || agent.aliases().contains(&name.as_str())
            || agent.executables().contains(&name.as_str())
    })
}

/// The supported agents that are installed, with their executables.
pub fn installed() -> Vec<(&'static dyn Adapter, PathBuf)> {
    agents()
        .filter_map(|agent| agent.installed().map(|path| (agent, path)))
        .collect()
}
