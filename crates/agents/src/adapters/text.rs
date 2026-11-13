//! Agents whose headless mode prints text only (or whose structured output
//! is undocumented): their output is the answer.

use std::path::Path;

use crate::adapter::{with_instructions, Adapter, Autonomy, Launch, Parser, Task, TextParser};

/// Aider: `aider --message <prompt> --yes-always`.
///
/// Aider never runs the shell commands it suggests in this mode, and only
/// edits files it decides to add to the chat.
pub struct Aider;

impl Adapter for Aider {
    fn id(&self) -> &'static str {
        "aider"
    }

    fn label(&self) -> &'static str {
        "Aider"
    }

    fn executables(&self) -> &'static [&'static str] {
        &["aider"]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::default()
            .args([
                "--yes-always",
                "--no-pretty",
                "--no-stream",
                "--no-auto-commits",
                "--no-check-update",
                "--no-show-release-notes",
            ])
            .option("--model", task.model);
        match task.autonomy {
            Autonomy::Full => launch,
            Autonomy::ReadOnly => launch.args(["--chat-mode", "ask"]),
        }
        .arg("--message")
        .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(TextParser::default())
    }
}

/// Crush: `crush run <prompt>`, which approves every action itself.
pub struct Crush;

impl Adapter for Crush {
    fn id(&self) -> &'static str {
        "crush"
    }

    fn label(&self) -> &'static str {
        "Crush"
    }

    fn executables(&self) -> &'static [&'static str] {
        &["crush"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &["CRUSH"]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        Launch::default()
            .arg("--cwd")
            .arg(task.cwd)
            .args(["run", "--quiet"])
            .option("--model", task.model)
            .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(TextParser::default())
    }
}

/// Kiro CLI: `kiro-cli chat --no-interactive <prompt>`.
pub struct Kiro;

impl Adapter for Kiro {
    fn id(&self) -> &'static str {
        "kiro"
    }

    fn label(&self) -> &'static str {
        "Kiro CLI"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["kiro-cli"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["kiro-cli"]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::default()
            .args(["chat", "--no-interactive"])
            .option("--model", task.model);
        match task.autonomy {
            Autonomy::Full => launch.arg("--trust-all-tools"),
            Autonomy::ReadOnly => launch,
        }
        .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(TextParser::default())
    }
}

/// Auggie (Augment): `auggie --print <prompt>`. Its tool permissions come
/// from its settings.
pub struct Auggie;

impl Adapter for Auggie {
    fn id(&self) -> &'static str {
        "auggie"
    }

    fn label(&self) -> &'static str {
        "Auggie"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["augment"]
    }

    fn executables(&self) -> &'static [&'static str] {
        &["auggie"]
    }

    fn session_env(&self) -> &'static [&'static str] {
        &["AUGMENT_AGENT"]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        Launch::default()
            .arg("--workspace-root")
            .arg(task.cwd)
            .option("--model", task.model)
            .arg("--print")
            .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(TextParser::default())
    }
}

/// Cline CLI: `cline <prompt>`, which approves actions itself by default.
pub struct Cline;

impl Adapter for Cline {
    fn id(&self) -> &'static str {
        "cline"
    }

    fn label(&self) -> &'static str {
        "Cline"
    }

    fn executables(&self) -> &'static [&'static str] {
        &["cline"]
    }

    fn launch(&self, task: &Task<'_>, _executable: &Path) -> Launch {
        let launch = Launch::default()
            .arg("--cwd")
            .arg(task.cwd)
            .option("--model", task.model);
        match task.autonomy {
            Autonomy::Full => launch,
            Autonomy::ReadOnly => launch.args(["--auto-approve", "false"]),
        }
        .arg(with_instructions(task.prompt, task.system_prompt))
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::new(TextParser::default())
    }
}
