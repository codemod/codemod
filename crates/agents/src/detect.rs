//! Whether this program runs inside a coding agent: in a command the agent
//! started, directly or through a shell.
//!
//! Two kinds of evidence, either of which is enough: an environment variable
//! the agent sets for the processes it starts, or the agent's executable
//! among this process's ancestors.

use crate::adapter::Adapter;
use crate::adapters::ADAPTERS;

/// The agent this program runs inside, and how that was found.
#[derive(Clone, Copy)]
pub struct ParentAgent {
    pub adapter: &'static dyn Adapter,
    pub evidence: Evidence,
}

impl std::fmt::Debug for ParentAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParentAgent")
            .field("agent", &self.adapter.id())
            .field("evidence", &self.evidence)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// The agent's session variable is set.
    Environment(&'static str),
    /// An ancestor process is the agent (its process id).
    Process(u32),
}

/// The coding agent this program runs inside, if any.
///
/// The nearest agent among this process's ancestors wins, since agents can
/// run inside each other (and pass their variables down); the variables
/// decide only when no ancestor is recognized.
pub fn parent_agent() -> Option<ParentAgent> {
    from_processes(&ancestors(MAX_DEPTH))
        .or_else(|| from_env(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty())))
}

const MAX_DEPTH: usize = 12;

fn from_env(is_set: impl Fn(&str) -> bool) -> Option<ParentAgent> {
    ADAPTERS.iter().find_map(|adapter| {
        adapter
            .env_markers()
            .iter()
            .find(|key| is_set(key))
            .map(|key| ParentAgent {
                adapter: *adapter,
                evidence: Evidence::Environment(key),
            })
    })
}

/// The agents the environment points at, the surest first: those whose own
/// variable is set, then those whose editor's variable is, which says only
/// that this runs in that editor's terminal. For callers that ask which
/// agent a person works with, not whether one runs this program.
pub fn agents_in_env(is_set: impl Fn(&str) -> bool) -> Vec<&'static dyn Adapter> {
    let any_set = |keys: &[&str]| keys.iter().any(|key| is_set(key));
    let own = ADAPTERS
        .iter()
        .filter(|adapter| any_set(adapter.env_markers()));
    let editor = ADAPTERS
        .iter()
        .filter(|adapter| any_set(adapter.editor_env_markers()));
    let mut found: Vec<&'static dyn Adapter> = Vec::new();
    for adapter in own.chain(editor) {
        if !found.iter().any(|seen| seen.id() == adapter.id()) {
            found.push(*adapter);
        }
    }
    found
}

/// A process in this one's ancestry.
#[derive(Debug, Clone)]
pub(crate) struct Process {
    pub pid: u32,
    /// The executable's name or path.
    pub command: String,
    /// The full command line.
    pub args: String,
}

fn from_processes(chain: &[Process]) -> Option<ParentAgent> {
    chain.iter().find_map(|process| {
        adapter_for_process(process).map(|adapter| ParentAgent {
            adapter,
            evidence: Evidence::Process(process.pid),
        })
    })
}

/// The agent a process is: its executable, or the script a runtime runs
/// (`node /usr/local/bin/codex`), named like one of the agent's executables.
fn adapter_for_process(process: &Process) -> Option<&'static dyn Adapter> {
    let mut names = vec![basename(&process.command)];
    names.extend(process.args.split_whitespace().take(2).map(basename));
    ADAPTERS.iter().copied().find(|adapter| {
        names
            .iter()
            .any(|name| adapter.executables().iter().any(|exe| name == exe))
    })
}

fn basename(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.strip_suffix(".exe")
        .unwrap_or(name)
        .to_ascii_lowercase()
}

#[cfg(unix)]
fn ancestors(max_depth: usize) -> Vec<Process> {
    use std::process::Command;

    let field = |pid: u32, column: &str| -> Option<String> {
        let output = Command::new("ps")
            .args(["-ww", "-o", &format!("{column}="), "-p", &pid.to_string()])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    let mut chain = Vec::new();
    let mut pid = std::os::unix::process::parent_id();
    while chain.len() < max_depth && pid > 1 {
        let (Some(command), Some(args)) = (field(pid, "comm"), field(pid, "args")) else {
            break;
        };
        chain.push(Process { pid, command, args });
        match field(pid, "ppid").and_then(|ppid| ppid.parse().ok()) {
            Some(parent) if parent != pid => pid = parent,
            _ => break,
        }
    }
    chain
}

#[cfg(not(unix))]
fn ancestors(_max_depth: usize) -> Vec<Process> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(command: &str, args: &str) -> Process {
        Process {
            pid: 42,
            command: command.to_string(),
            args: args.to_string(),
        }
    }

    #[test]
    fn a_session_variable_names_the_agent() {
        let found = from_env(|key| key == "CODEX_SANDBOX").unwrap();
        assert_eq!(found.adapter.id(), "codex");
        assert_eq!(found.evidence, Evidence::Environment("CODEX_SANDBOX"));
        assert!(from_env(|_| false).is_none());
    }

    #[test]
    fn an_editors_variable_counts_after_any_agents_own() {
        let ids = |set: &[&str]| -> Vec<&str> {
            agents_in_env(|key| set.contains(&key))
                .iter()
                .map(|agent| agent.id())
                .collect()
        };
        assert_eq!(
            ids(&["CURSOR_TRACE_ID", "GEMINI_CLI"]),
            ["gemini", "cursor"]
        );
        assert_eq!(ids(&["CURSOR_TRACE_ID", "CURSOR_AGENT"]), ["cursor"]);
        assert!(ids(&[]).is_empty());
        assert!(
            from_env(|key| key == "CURSOR_TRACE_ID").is_none(),
            "a terminal in the editor is not a session of its agent"
        );
    }

    #[test]
    fn an_ancestor_running_the_agent_names_it() {
        let chain = [
            process("/bin/zsh", "/bin/zsh -c codemod run"),
            process(
                "/opt/homebrew/bin/node",
                "node /opt/homebrew/bin/codex exec",
            ),
        ];
        let found = from_processes(&chain).unwrap();
        assert_eq!(found.adapter.id(), "codex");
        assert_eq!(found.evidence, Evidence::Process(42));

        let found = from_processes(&[process("/Users/me/.local/bin/claude", "claude")]).unwrap();
        assert_eq!(found.adapter.id(), "claude-code");
    }

    #[test]
    fn similar_names_are_not_agents() {
        let chain = [
            process("codec-service", "codec-service --codex"),
            process("/usr/bin/vim", "vim notes-about-claude.md"),
        ];
        assert!(from_processes(&chain).is_none());
    }
}
