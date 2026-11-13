//! Each adapter, run against a stand-in executable that records how it was
//! called and replays output recorded from the real agent.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use codemod_agents::{find, AgentEvent, Autonomy, Outcome, Run, RunError, Task};
use tempfile::TempDir;

struct Fake {
    dir: TempDir,
}

impl Fake {
    /// An executable that records its arguments and stdin, then runs `body`.
    fn new(body: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{dir}/args\"\ncat > \"{dir}/stdin\"\n{body}\n",
            dir = dir.path().display()
        );
        let path = dir.path().join("agent");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    /// One that prints a recorded fixture.
    fn replaying(fixture: &str) -> Self {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        Self::new(&format!("cat \"{}\"", fixture.display()))
    }

    fn executable(&self) -> PathBuf {
        self.dir.path().join("agent")
    }

    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("args"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn stdin(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("stdin")).unwrap()
    }
}

fn task(cwd: &Path) -> Task<'_> {
    Task {
        prompt: "Create hello.txt containing hi.",
        system_prompt: Some("Be brief."),
        model: Some("small-model"),
        cwd,
        autonomy: Autonomy::Full,
    }
}

fn run_with(
    agent: &str,
    fake: &Fake,
    task: Task<'_>,
) -> (Result<Outcome, RunError>, Vec<AgentEvent>) {
    let adapter = find(agent).unwrap().adapter;
    let mut events = Vec::new();
    let result = Run {
        adapter,
        executable: fake.executable(),
        task,
        cancel: None,
        timeout: Some(Duration::from_secs(30)),
    }
    .run(&mut |event| events.push(event));
    (result, events)
}

fn tool_names(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCall { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn tool_results(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------- agents ---

#[test]
fn claude_code() {
    let fake = Fake::replaying("claude-code.jsonl");
    let cwd = TempDir::new().unwrap();
    let (outcome, events) = run_with("claude", &fake, task(cwd.path()));
    let outcome = outcome.unwrap();

    assert_eq!(outcome.agent, "claude-code");
    assert_eq!(outcome.result.as_deref(), Some("DONE"));
    assert_eq!(
        outcome.session_id.as_deref(),
        Some("03bcde8f-18b6-4aa5-86c9-2eedf52664a5")
    );
    assert_eq!(outcome.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(outcome.usage.input_tokens, Some(4 + 25668 + 46848));
    assert_eq!(outcome.usage.output_tokens, Some(254));
    assert_eq!(outcome.usage.cost_usd, Some(0.00572948));
    assert_eq!(tool_names(&events), ["Bash"]);
    assert_eq!(tool_results(&events), ["hi"]);

    let args = fake.args();
    for flag in [
        "--print",
        "stream-json",
        "--verbose",
        "--dangerously-skip-permissions",
    ] {
        assert!(args.contains(&flag.to_string()), "{flag} in {args:?}");
    }
    assert_eq!(after(&args, "--model"), "small-model");
    assert_eq!(after(&args, "--append-system-prompt"), "Be brief.");
    assert_eq!(fake.stdin().trim(), "Create hello.txt containing hi.");
}

#[test]
fn codex() {
    let fake = Fake::replaying("codex.jsonl");
    let cwd = TempDir::new().unwrap();
    let (outcome, events) = run_with("codex", &fake, task(cwd.path()));
    let outcome = outcome.unwrap();

    assert_eq!(outcome.result.as_deref(), Some("DONE"));
    assert_eq!(
        outcome.session_id.as_deref(),
        Some("01a1d35d-50f3-7402-ab1b-93e50584c118")
    );
    assert_eq!(outcome.usage.input_tokens, Some(55949));
    assert_eq!(outcome.usage.output_tokens, Some(58));
    assert_eq!(tool_names(&events), ["shell"]);
    assert_eq!(tool_results(&events), ["hi"]);

    let args = fake.args();
    assert_eq!(args[0], "exec");
    for flag in [
        "--json",
        "--skip-git-repo-check",
        "--dangerously-bypass-approvals-and-sandbox",
    ] {
        assert!(args.contains(&flag.to_string()), "{flag} in {args:?}");
    }
    assert_eq!(after(&args, "--cd"), cwd.path().to_str().unwrap());
    assert_eq!(after(&args, "--model"), "small-model");
    assert_eq!(args.last().unwrap(), "-");
    let stdin = fake.stdin();
    assert!(
        stdin.contains("<instructions>\nBe brief.\n</instructions>"),
        "{stdin}"
    );
    assert!(stdin.contains("Create hello.txt containing hi."), "{stdin}");
}

#[test]
fn opencode() {
    let fake = Fake::replaying("opencode.jsonl");
    let cwd = TempDir::new().unwrap();
    let (outcome, events) = run_with("opencode", &fake, task(cwd.path()));
    let outcome = outcome.unwrap();

    assert_eq!(outcome.result.as_deref(), Some("DONE"));
    assert_eq!(
        outcome.session_id.as_deref(),
        Some("ses_e2ca1c5ebffeEV1fEMHFocPOHt")
    );
    assert_eq!(outcome.usage.input_tokens, Some(19265 + 1728 + 392 + 20864));
    assert_eq!(outcome.usage.output_tokens, Some(115));
    assert_eq!(tool_names(&events), ["write"]);

    let args = fake.args();
    assert_eq!(args[..3], ["run", "--format", "json"]);
    assert_eq!(after(&args, "--model"), "small-model");
    assert!(fake.stdin().contains("Create hello.txt containing hi."));
}

// ------------------------------------------------------------- runner ---

#[test]
fn a_failing_agent_reports_its_stderr() {
    let fake = Fake::new("echo 'invalid API key' >&2\nexit 3");
    let cwd = TempDir::new().unwrap();
    let (result, events) = run_with("claude", &fake, task(cwd.path()));
    let error = result.unwrap_err().to_string();
    assert!(
        error.starts_with("Claude Code failed: exited with code 3"),
        "{error}"
    );
    assert!(error.contains("invalid API key"), "{error}");
    assert!(matches!(&events[..], [AgentEvent::Output { line, .. }] if line == "invalid API key"));
}

#[test]
fn an_error_the_agent_reports_fails_the_run_even_when_it_exits_zero() {
    let fake = Fake::new(
        r#"echo '{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s"}'"#,
    );
    let cwd = TempDir::new().unwrap();
    let (result, _) = run_with("claude", &fake, task(cwd.path()));
    assert!(
        result.unwrap_err().to_string().contains("error_max_turns"),
        "the result subtype is the message"
    );
}

#[test]
fn canceling_stops_the_agent_and_what_it_started() {
    let cwd = TempDir::new().unwrap();
    let marker = cwd.path().join("still-running");
    let fake = Fake::new(&format!(
        "(sleep 2; touch \"{}\") &\nsleep 30",
        marker.display()
    ));
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let result = Run {
        adapter: find("codex").unwrap().adapter,
        executable: fake.executable(),
        task: task(cwd.path()),
        cancel: Some(cancel),
        timeout: None,
    }
    .run(&mut |_| {});
    assert!(matches!(result, Err(RunError::Canceled { .. })));
    assert!(started.elapsed() < Duration::from_secs(10));
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "the agent's child process was stopped too"
    );
}

#[test]
fn an_agent_that_runs_too_long_times_out() {
    let fake = Fake::new("sleep 30");
    let cwd = TempDir::new().unwrap();
    let result = Run {
        adapter: find("codex").unwrap().adapter,
        executable: fake.executable(),
        task: task(cwd.path()),
        cancel: None,
        timeout: Some(Duration::from_millis(300)),
    }
    .run(&mut |_| {});
    assert!(matches!(result, Err(RunError::TimedOut { .. })));
}

fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let index = args
        .iter()
        .position(|arg| arg == flag)
        .unwrap_or_else(|| panic!("{flag} missing from {args:?}"));
    &args[index + 1]
}

// ------------------------------------------------- every other agent ---

enum Prompt {
    Stdin,
    Argument,
}

struct Case {
    agent: &'static str,
    fixture: &'static str,
    result: &'static str,
    session: Option<&'static str>,
    tools: &'static [&'static str],
    prompt: Prompt,
}

const TEXT_RESULT: &str = "Added hello.txt to the chat.\nApplied edit to hello.txt\nDONE";

#[rustfmt::skip]
const CASES: &[Case] = &[
    Case { agent: "cursor", fixture: "cursor.jsonl", result: "DONE", session: Some("c-123"), tools: &["write"], prompt: Prompt::Argument },
    Case { agent: "gemini", fixture: "gemini.jsonl", result: "DONE", session: Some("g-123"), tools: &["write_file"], prompt: Prompt::Argument },
    Case { agent: "agy", fixture: "antigravity.jsonl", result: "DONE", session: Some("a-123"), tools: &["write_file"], prompt: Prompt::Argument },
    Case { agent: "qwen", fixture: "qwen-code.jsonl", result: "DONE", session: Some("q-123"), tools: &["write_file"], prompt: Prompt::Argument },
    Case { agent: "amp", fixture: "amp.jsonl", result: "DONE", session: Some("T-123"), tools: &["create_file"], prompt: Prompt::Stdin },
    Case { agent: "copilot", fixture: "copilot.jsonl", result: "DONE", session: Some("cp-123"), tools: &["bash"], prompt: Prompt::Argument },
    Case { agent: "goose", fixture: "goose.jsonl", result: "DONE", session: None, tools: &["developer__text_editor"], prompt: Prompt::Stdin },
    Case { agent: "droid", fixture: "droid.jsonl", result: "DONE", session: Some("d-123"), tools: &["Create"], prompt: Prompt::Stdin },
    Case { agent: "kimi", fixture: "kimi.jsonl", result: "DONE", session: Some("k-123"), tools: &["WriteFile"], prompt: Prompt::Argument },
    Case { agent: "openclaw", fixture: "openclaw.json", result: "DONE", session: Some("019-oc"), tools: &[], prompt: Prompt::Stdin },
    Case { agent: "aider", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Argument },
    Case { agent: "crush", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Argument },
    Case { agent: "kiro", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Argument },
    Case { agent: "auggie", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Argument },
    Case { agent: "cline", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Argument },
];

#[test]
fn every_agent_reads_its_output_and_gets_the_prompt() {
    let mut failures = Vec::new();
    for case in CASES {
        let fake = Fake::replaying(case.fixture);
        let cwd = TempDir::new().unwrap();
        let (outcome, events) = run_with(case.agent, &fake, task(cwd.path()));
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                failures.push(format!("{}: {error}", case.agent));
                continue;
            }
        };
        let mut check = |what: &str, ok: bool, detail: String| {
            if !ok {
                failures.push(format!("{}: {what}: {detail}", case.agent));
            }
        };
        check(
            "result",
            outcome.result.as_deref() == Some(case.result),
            format!("{:?}", outcome.result),
        );
        check(
            "session",
            outcome.session_id.as_deref() == case.session,
            format!("{:?}", outcome.session_id),
        );
        check(
            "tools",
            tool_names(&events) == case.tools,
            format!("{:?}", tool_names(&events)),
        );
        let args = fake.args().join("\n");
        let stdin = fake.stdin();
        let prompt = "Create hello.txt containing hi.";
        match case.prompt {
            Prompt::Stdin => check(
                "prompt on stdin",
                stdin.contains(prompt) && !args.contains(prompt),
                format!("stdin {stdin:?}, args {args:?}"),
            ),
            Prompt::Argument => check(
                "prompt as an argument",
                args.contains(prompt),
                format!("args {args:?}"),
            ),
        }
        check(
            "instructions",
            args.contains("Be brief.") || stdin.contains("Be brief."),
            format!("args {args:?}"),
        );
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn ids_aliases_and_executables_name_one_agent_each() {
    let mut seen = std::collections::HashMap::new();
    for agent in codemod_agents::agents() {
        let names = std::iter::once(agent.id())
            .chain(agent.adapter.aliases().iter().copied())
            .chain(agent.adapter.executables().iter().copied());
        for name in names {
            if let Some(other) = seen.insert(name, agent.id()) {
                assert_eq!(
                    other,
                    agent.id(),
                    "'{name}' names both {other} and {}",
                    agent.id()
                );
            }
            assert_eq!(find(name).unwrap().id(), agent.id(), "find({name})");
        }
    }
    assert!(find("Claude").is_some(), "names are case-insensitive");
    assert!(find("nope").is_none());
}
