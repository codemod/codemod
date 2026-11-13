//! Agents whose headless mode prints text only (or whose structured output
//! is undocumented): their output is the answer. They differ only in their
//! command lines, which are data here.

use std::path::Path;

use crate::adapter::{Adapter, Autonomy, Launch, ReadOnly, Task};
use crate::parse::{not_a_flag, with_instructions, TextParser};

/// An agent that prints text. Its command line is
/// `[cwd_flag <cwd>] <args> [--model <model>] <full | read_only> <prompt>`.
pub(crate) struct TextAgent {
    id: &'static str,
    label: &'static str,
    aliases: &'static [&'static str],
    executables: &'static [&'static str],
    env_markers: &'static [&'static str],
    /// The flag that takes the directory to work in, when there is one.
    cwd_flag: Option<&'static str>,
    args: &'static [&'static str],
    /// What lets it edit files and run commands without asking.
    full: &'static [&'static str],
    /// How it is kept read-only, and with which arguments.
    read_only: (ReadOnly, &'static [&'static str]),
    prompt: Prompt,
}

/// Where the prompt goes on the command line.
enum Prompt {
    /// `-- <prompt>`.
    Positional,
    /// `<flag>=<prompt>`.
    Joined(&'static str),
    /// `<flag> <prompt>`, for a flag not known to take `=`: a prompt that
    /// starts with `-` gets a space in front.
    After(&'static str),
}

impl Adapter for TextAgent {
    fn id(&self) -> &'static str {
        self.id
    }

    fn label(&self) -> &'static str {
        self.label
    }

    fn aliases(&self) -> &'static [&'static str] {
        self.aliases
    }

    fn executables(&self) -> &'static [&'static str] {
        self.executables
    }

    fn env_markers(&self) -> &'static [&'static str] {
        self.env_markers
    }

    fn read_only(&self) -> ReadOnly {
        self.read_only.0
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let mut launch = Launch::new(TextParser);
        if let Some(flag) = self.cwd_flag {
            launch = launch.arg(flag).arg(task.cwd);
        }
        let launch =
            launch
                .args(self.args)
                .option("--model", task.model)
                .args(match task.autonomy {
                    Autonomy::Full => self.full,
                    Autonomy::ReadOnly => self.read_only.1,
                });
        let prompt = with_instructions(task.prompt, task.system_prompt);
        match self.prompt {
            Prompt::Positional => launch.positional(prompt),
            Prompt::Joined(flag) => launch.joined(flag, &prompt),
            Prompt::After(flag) => launch.arg(flag).arg(not_a_flag(prompt)),
        }
    }
}

/// Aider: `aider --message=<prompt> --yes-always`.
///
/// With `--yes-always` Aider never runs the shell commands it suggests, and
/// only edits files it decides to add to the chat. Read-only is its ask
/// mode, which answers without editing, as a dry run and without the yes to
/// every question.
pub(crate) static AIDER: TextAgent = TextAgent {
    id: "aider",
    label: "Aider",
    aliases: &[],
    executables: &["aider"],
    env_markers: &[],
    cwd_flag: None,
    args: &[
        "--no-pretty",
        "--no-stream",
        "--no-auto-commits",
        "--no-check-update",
        "--no-show-release-notes",
    ],
    full: &["--yes-always"],
    read_only: (
        ReadOnly::Enforced,
        &["--chat-mode", "ask", "--dry-run", "--no-gitignore"],
    ),
    prompt: Prompt::Joined("--message"),
};

/// Crush: `crush run -- <prompt>`, which approves every action itself.
pub(crate) static CRUSH: TextAgent = TextAgent {
    id: "crush",
    label: "Crush",
    aliases: &[],
    executables: &["crush"],
    env_markers: &["CRUSH"],
    cwd_flag: Some("--cwd"),
    args: &["run", "--quiet"],
    full: &[],
    read_only: (ReadOnly::Unsupported, &[]),
    prompt: Prompt::Positional,
};

/// Kiro CLI: `kiro-cli chat --no-interactive -- <prompt>`. Without
/// `--trust-all-tools` it asks before a tool writes or runs a command.
pub(crate) static KIRO: TextAgent = TextAgent {
    id: "kiro",
    label: "Kiro CLI",
    aliases: &["kiro-cli"],
    executables: &["kiro-cli"],
    env_markers: &[],
    cwd_flag: None,
    args: &["chat", "--no-interactive"],
    full: &["--trust-all-tools"],
    read_only: (ReadOnly::AgentDefaults, &[]),
    prompt: Prompt::Positional,
};

/// Auggie (Augment): `auggie --print <prompt>`. Its tool permissions come
/// from its settings, so nothing here can keep it read-only.
pub(crate) static AUGGIE: TextAgent = TextAgent {
    id: "auggie",
    label: "Auggie",
    aliases: &["augment"],
    executables: &["auggie"],
    env_markers: &["AUGMENT_AGENT"],
    cwd_flag: Some("--workspace-root"),
    args: &[],
    full: &[],
    read_only: (ReadOnly::Unsupported, &[]),
    prompt: Prompt::After("--print"),
};

/// Cline CLI: `cline -- <prompt>`, which approves actions itself by default.
pub(crate) static CLINE: TextAgent = TextAgent {
    id: "cline",
    label: "Cline",
    aliases: &[],
    executables: &["cline"],
    env_markers: &[],
    cwd_flag: Some("--cwd"),
    args: &[],
    full: &[],
    read_only: (ReadOnly::Enforced, &["--auto-approve", "false"]),
    prompt: Prompt::Positional,
};
