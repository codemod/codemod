//! Each adapter, run against a stand-in executable that records how it was
//! called and replays output recorded from the real agent.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use codemod_agents::{
    agents, find, AgentEvent, Autonomy, FileChangeKind, Outcome, ReadOnly, Run, RunError, Task,
};
use tempfile::TempDir;

/// What the stand-in prints for `--help`: every flag an adapter looks for.
const HELP: &str = "--trust --no-ask-user --allow-all-paths --output-format stream-json \
                    --skip-trust --approval-mode --dangerously-skip-permissions --auto";

struct Fake {
    dir: TempDir,
}

impl Fake {
    /// An executable that records its arguments, environment and stdin,
    /// then runs `body`. Asked for `--help`, it prints `help` instead.
    fn with_help(help: &str, body: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let script = format!(
            "#!/bin/sh\n\
             case \" $* \" in *\" --help \"*) echo '{help}'; exit 0;; esac\n\
             printf '%s\\0' \"$@\" > \"{dir}/args\"\n\
             env > \"{dir}/env\"\n\
             cat > \"{dir}/stdin\"\n\
             {body}\n",
            dir = dir.path().display()
        );
        let path = dir.path().join("agent");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn new(body: &str) -> Self {
        Self::with_help(HELP, body)
    }

    /// One that prints a recorded fixture.
    fn replaying(fixture: &str) -> Self {
        Self::new(&format!("cat \"{}\"", fixture_path(fixture).display()))
    }

    fn executable(&self) -> PathBuf {
        self.dir.path().join("agent")
    }

    /// Whether the executable ran, other than for `--help`.
    fn ran(&self) -> bool {
        self.dir.path().join("args").exists()
    }

    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("args"))
            .unwrap()
            .split_terminator('\0')
            .map(str::to_string)
            .collect()
    }

    fn env(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("env")).unwrap()
    }

    fn stdin(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("stdin")).unwrap()
    }
}

fn fixture_path(fixture: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture)
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
    let mut events = Vec::new();
    let result = Run {
        adapter: find(agent).unwrap(),
        executable: fake.executable(),
        task,
        cancel: None,
        timeout: Some(Duration::from_secs(30)),
    }
    .run(&mut |event| events.push(event));
    (result, events)
}

/// Run `agent` against a stand-in that runs `body`, with the usual task.
fn run_script(agent: &str, body: &str) -> (Result<Outcome, RunError>, Vec<AgentEvent>) {
    let fake = Fake::new(body);
    let cwd = TempDir::new().unwrap();
    run_with(agent, &fake, task(cwd.path()))
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

fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let index = args
        .iter()
        .position(|arg| arg == flag)
        .unwrap_or_else(|| panic!("{flag} missing from {args:?}"));
    &args[index + 1]
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
    let (result, events) = run_script("claude", "echo 'invalid API key' >&2\nexit 3");
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
    let (result, _) = run_script(
        "claude",
        r#"echo '{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s"}'"#,
    );
    let error = result.unwrap_err();
    assert!(
        error.to_string().contains("error_max_turns"),
        "the result subtype is the message"
    );
    assert_eq!(
        error.partial().and_then(|p| p.session_id.as_deref()),
        Some("s"),
        "what the agent reported comes with the failure"
    );
}

#[test]
fn an_error_the_agent_recovers_from_does_not_fail_a_run_that_ends_well() {
    let (result, events) = run_script(
        "copilot",
        r#"echo '{"type":"session.error","data":{"message":"rate limited"}}'
echo '{"type":"assistant.message","data":{"content":"DONE"}}'
echo '{"type":"result","exitCode":0}'"#,
    );
    assert_eq!(result.unwrap().result.as_deref(), Some("DONE"));
    assert!(events.contains(&AgentEvent::Error {
        message: "rate limited".into()
    }));

    let (result, _) = run_script(
        "copilot",
        r#"echo '{"type":"session.error","data":{"message":"rate limited"}}'
echo '{"type":"result","exitCode":1}'"#,
    );
    let error = result.unwrap_err().to_string();
    assert!(error.contains("rate limited"), "{error}");
}

#[test]
fn output_after_the_final_report_is_not_part_of_the_run() {
    let (result, events) = run_script(
        "claude",
        r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"DONE"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"late"}]}}'
echo 'not json'"#,
    );
    assert_eq!(result.unwrap().result.as_deref(), Some("DONE"));
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn an_empty_final_answer_falls_back_to_the_last_text() {
    let (result, _) = run_script(
        "claude",
        r#"echo '{"type":"assistant","message":{"content":[{"type":"text","text":"The answer."}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":""}'"#,
    );
    assert_eq!(result.unwrap().result.as_deref(), Some("The answer."));
}

#[test]
fn text_agents_keep_their_paragraphs() {
    let (result, _) = run_script("aider", "printf 'One.\\n\\nTwo.\\n'");
    assert_eq!(result.unwrap().result.as_deref(), Some("One.\n\nTwo."));
}

const SESSION_STARTED: &str = r#"echo '{"type":"thread.started","thread_id":"t-1"}'"#;

/// Runs Codex against `fake`, canceling a moment after it reports its
/// session if asked to: a process the shell has only just forked can miss
/// the signal that stops it. Returns when the session was reported too: a
/// stand-in can take a while to start, so the tests time from there.
fn run_codex(
    fake: &Fake,
    cwd: &Path,
    cancel_when_started: bool,
    timeout: Option<Duration>,
) -> (Result<Outcome, RunError>, Instant) {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut session_at = None;
    let result = Run {
        adapter: find("codex").unwrap(),
        executable: fake.executable(),
        task: task(cwd),
        cancel: Some(Arc::clone(&cancel)),
        timeout,
    }
    .run(&mut |event| {
        if let AgentEvent::Started { .. } = event {
            if cancel_when_started {
                std::thread::sleep(Duration::from_millis(500));
                cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            session_at = Some(Instant::now());
        }
    });
    (result, session_at.expect("the agent reported its session"))
}

#[test]
fn canceling_stops_the_agent_and_what_it_started() {
    let cwd = TempDir::new().unwrap();
    let marker = cwd.path().join("still-running");
    let fake = Fake::new(&format!(
        "{SESSION_STARTED}\n(sleep 2; touch \"{}\") &\nsleep 30",
        marker.display()
    ));
    let (result, canceled_at) = run_codex(&fake, cwd.path(), true, None);
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::Canceled { .. }), "{error}");
    assert_eq!(
        error.partial().and_then(|p| p.session_id.as_deref()),
        Some("t-1"),
        "the session it had started comes with the error"
    );
    assert!(canceled_at.elapsed() < Duration::from_secs(2));
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "the agent's child process was stopped too"
    );
}

#[test]
fn stopping_kills_what_ignores_the_request_to_stop() {
    let cwd = TempDir::new().unwrap();
    let marker = cwd.path().join("still-running");
    // The background shell ignores SIGTERM, and so does the sleep it starts.
    let fake = Fake::new(&format!(
        "(trap '' TERM; {SESSION_STARTED}; sleep 5; touch \"{}\") &\nsleep 30",
        marker.display()
    ));
    let (result, canceled_at) = run_codex(&fake, cwd.path(), true, None);
    assert!(matches!(result, Err(RunError::Canceled { .. })));
    assert!(
        canceled_at.elapsed() >= Duration::from_secs(3),
        "the run gives the group time to end"
    );
    std::thread::sleep(Duration::from_secs(7).saturating_sub(canceled_at.elapsed()));
    assert!(!marker.exists(), "what ignored SIGTERM was killed");
}

#[test]
fn an_agent_that_runs_too_long_times_out_with_what_it_reported() {
    let fake = Fake::new(&format!(
        r#"{SESSION_STARTED}
echo '{{"type":"item.completed","item":{{"id":"i","type":"agent_message","text":"Working on it."}}}}'
echo '{{"type":"turn.completed","usage":{{"input_tokens":10,"output_tokens":2}}}}'
sleep 30"#
    ));
    let cwd = TempDir::new().unwrap();
    let (result, _) = run_codex(&fake, cwd.path(), false, Some(Duration::from_secs(4)));
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::TimedOut { .. }), "{error}");
    let partial = error.partial().unwrap();
    assert_eq!(partial.session_id.as_deref(), Some("t-1"));
    assert_eq!(partial.usage.input_tokens, Some(10));
    assert_eq!(partial.result.as_deref(), Some("Working on it."));
}

#[test]
fn an_agent_that_exits_ends_the_run_though_something_holds_its_output() {
    let cwd = TempDir::new().unwrap();
    let marker = cwd.path().join("still-running");
    // The background process inherits stdout and stderr and outlives the agent.
    let fake = Fake::new(&format!(
        "cat \"{}\"\n(sleep 6; touch \"{}\") &\nexit 0",
        fixture_path("codex.jsonl").display(),
        marker.display()
    ));
    let (result, started_at) = run_codex(&fake, cwd.path(), false, None);
    assert_eq!(result.unwrap().result.as_deref(), Some("DONE"));
    assert!(
        started_at.elapsed() < Duration::from_secs(4),
        "the run did not wait for the background process"
    );
    std::thread::sleep(Duration::from_secs(8).saturating_sub(started_at.elapsed()));
    assert!(!marker.exists(), "what the agent left behind was stopped");
}

// ----------------------------------------------------- every agent ---

enum Prompt {
    Stdin,
    /// `flag=prompt`.
    Joined(&'static str),
    /// `-- prompt`.
    Positional,
    /// `flag prompt`, the prompt never starting with `-`.
    After(&'static str),
}

struct Case {
    agent: &'static str,
    fixture: &'static str,
    result: &'static str,
    session: Option<&'static str>,
    tools: &'static [&'static str],
    prompt: Prompt,
    /// How a read-only task is kept read-only, with arguments it then gets
    /// and arguments it must not get.
    read_only: (ReadOnly, &'static [&'static str], &'static [&'static str]),
}

const TEXT_RESULT: &str = "Added hello.txt to the chat.\nApplied edit to hello.txt\nDONE";
const UNSUPPORTED: (ReadOnly, &[&str], &[&str]) = (ReadOnly::Unsupported, &[], &[]);

#[rustfmt::skip]
const CASES: &[Case] = &[
    Case { agent: "claude-code", fixture: "claude-code.jsonl", result: "DONE", session: Some("03bcde8f-18b6-4aa5-86c9-2eedf52664a5"), tools: &["Bash"], prompt: Prompt::Stdin,
           read_only: (ReadOnly::Enforced, &["--permission-mode", "plan"], &["--dangerously-skip-permissions"]) },
    Case { agent: "codex", fixture: "codex.jsonl", result: "DONE", session: Some("01a1d35d-50f3-7402-ab1b-93e50584c118"), tools: &["shell"], prompt: Prompt::Stdin,
           read_only: (ReadOnly::Enforced, &["--sandbox", "read-only"], &["--dangerously-bypass-approvals-and-sandbox"]) },
    Case { agent: "cursor", fixture: "cursor.jsonl", result: "DONE", session: Some("c-123"), tools: &["write"], prompt: Prompt::Positional,
           read_only: (ReadOnly::Enforced, &["--mode", "ask"], &["--force"]) },
    Case { agent: "gemini", fixture: "gemini.jsonl", result: "DONE", session: Some("g-123"), tools: &["write_file"], prompt: Prompt::Joined("--prompt"),
           read_only: (ReadOnly::AgentDefaults, &[], &["--approval-mode", "yolo", "--yolo"]) },
    Case { agent: "agy", fixture: "antigravity.jsonl", result: "DONE", session: Some("a-123"), tools: &["write_file"], prompt: Prompt::After("--print"),
           read_only: (ReadOnly::AgentDefaults, &[], &["--dangerously-skip-permissions"]) },
    Case { agent: "opencode", fixture: "opencode.jsonl", result: "DONE", session: Some("ses_e2ca1c5ebffeEV1fEMHFocPOHt"), tools: &["write"], prompt: Prompt::Stdin,
           read_only: (ReadOnly::Enforced, &["--agent", "plan"], &["--auto", "--dangerously-skip-permissions"]) },
    Case { agent: "copilot", fixture: "copilot.jsonl", result: "DONE", session: Some("cp-123"), tools: &["bash"], prompt: Prompt::Joined("--prompt"),
           read_only: (ReadOnly::AgentDefaults, &[], &["--allow-all-tools", "--allow-all-paths"]) },
    Case { agent: "amp", fixture: "amp.jsonl", result: "DONE", session: Some("T-123"), tools: &["create_file"], prompt: Prompt::Stdin,
           read_only: UNSUPPORTED },
    Case { agent: "qwen", fixture: "qwen-code.jsonl", result: "DONE", session: Some("q-123"), tools: &["write_file"], prompt: Prompt::Joined("--prompt"),
           read_only: (ReadOnly::Enforced, &["--approval-mode", "plan"], &["yolo"]) },
    Case { agent: "goose", fixture: "goose.jsonl", result: "DONE", session: None, tools: &["developer__text_editor"], prompt: Prompt::Stdin,
           read_only: (ReadOnly::Enforced, &[], &[]) },
    Case { agent: "droid", fixture: "droid.jsonl", result: "DONE", session: Some("d-123"), tools: &["Create"], prompt: Prompt::Stdin,
           read_only: (ReadOnly::AgentDefaults, &[], &["--skip-permissions-unsafe"]) },
    Case { agent: "kimi", fixture: "kimi.jsonl", result: "DONE", session: Some("k-123"), tools: &["WriteFile"], prompt: Prompt::Joined("--prompt"),
           read_only: UNSUPPORTED },
    Case { agent: "crush", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Positional,
           read_only: UNSUPPORTED },
    Case { agent: "openclaw", fixture: "openclaw.json", result: "DONE", session: Some("019-oc"), tools: &[], prompt: Prompt::Stdin,
           read_only: UNSUPPORTED },
    Case { agent: "aider", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Joined("--message"),
           read_only: (ReadOnly::Enforced, &["--chat-mode", "ask", "--dry-run"], &["--yes-always"]) },
    Case { agent: "kiro", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Positional,
           read_only: (ReadOnly::AgentDefaults, &[], &["--trust-all-tools"]) },
    Case { agent: "auggie", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::After("--print"),
           read_only: UNSUPPORTED },
    Case { agent: "cline", fixture: "text.txt", result: TEXT_RESULT, session: None, tools: &[], prompt: Prompt::Positional,
           read_only: (ReadOnly::Enforced, &["--auto-approve", "false"], &[]) },
];

/// Runs `check` for every case and reports every failure at once.
fn for_every_case(mut check: impl FnMut(&Case, &mut dyn FnMut(&str, bool, String))) {
    let mut failures = Vec::new();
    for case in CASES {
        check(case, &mut |what, ok, detail| {
            if !ok {
                failures.push(format!("{}: {what}: {detail}", case.agent));
            }
        });
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn every_agent_reads_its_output_and_gets_the_prompt() {
    for_every_case(|case, check| {
        let fake = Fake::replaying(case.fixture);
        let cwd = TempDir::new().unwrap();
        let (outcome, events) = run_with(case.agent, &fake, task(cwd.path()));
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return check("run", false, error.to_string()),
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
            _ => check(
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
    });
}

#[test]
fn a_prompt_that_looks_like_a_flag_is_not_passed_as_one() {
    let prompt = "--force is the flag to remove.\n-- and this is its second line";
    for_every_case(|case, check| {
        let fake = Fake::replaying(case.fixture);
        let cwd = TempDir::new().unwrap();
        let task = Task {
            prompt,
            system_prompt: None,
            ..task(cwd.path())
        };
        if let (Err(error), _) = run_with(case.agent, &fake, task) {
            return check("run", false, error.to_string());
        }
        let args = fake.args();
        let ok = match case.prompt {
            Prompt::Stdin => fake.stdin().contains(prompt) && !args.iter().any(|a| a == prompt),
            Prompt::Joined(flag) => args.contains(&format!("{flag}={prompt}")),
            Prompt::Positional => args.ends_with(&["--".to_string(), prompt.to_string()]),
            Prompt::After(flag) => args.ends_with(&[flag.to_string(), format!(" {prompt}")]),
        };
        check("prompt", ok, format!("{args:?}"));
    });
}

#[test]
fn instructions_that_look_like_a_flag_are_joined_to_theirs() {
    let fake = Fake::replaying("claude-code.jsonl");
    let cwd = TempDir::new().unwrap();
    let task = Task {
        system_prompt: Some("- Be brief.\n- Be kind."),
        ..task(cwd.path())
    };
    run_with("claude", &fake, task).0.unwrap();
    let args = fake.args();
    let joined = "--append-system-prompt=- Be brief.\n- Be kind.".to_string();
    assert!(args.contains(&joined), "{args:?}");
}

#[test]
fn a_prompt_too_large_for_the_command_line_is_refused_before_the_agent_starts() {
    let prompt = "x".repeat(200 * 1024);
    for_every_case(|case, check| {
        let fake = Fake::replaying(case.fixture);
        let cwd = TempDir::new().unwrap();
        let task = Task {
            prompt: &prompt,
            ..task(cwd.path())
        };
        let (result, _) = run_with(case.agent, &fake, task);
        match case.prompt {
            Prompt::Stdin => check(
                "takes a large prompt on stdin",
                result.is_ok() && fake.stdin().len() >= prompt.len(),
                format!("{:?}", result.err().map(|e| e.to_string())),
            ),
            _ => {
                let label = find(case.agent).unwrap().label();
                let refused = matches!(
                    &result,
                    Err(error @ RunError::PromptTooLarge { .. }) if error.to_string().contains(label)
                );
                check(
                    "refuses a large prompt",
                    refused && !fake.ran(),
                    format!("{:?}", result.map_err(|e| e.to_string())),
                );
            }
        }
    });
}

#[test]
fn read_only_tasks_are_kept_read_only_or_refused() {
    for_every_case(|case, check| {
        let (how, with, without) = case.read_only;
        let adapter = find(case.agent).unwrap();
        check(
            "read_only",
            adapter.read_only() == how,
            format!("{:?}", adapter.read_only()),
        );
        let fake = Fake::replaying(case.fixture);
        let cwd = TempDir::new().unwrap();
        let task = Task {
            autonomy: Autonomy::ReadOnly,
            ..task(cwd.path())
        };
        let (result, _) = run_with(case.agent, &fake, task);
        if how == ReadOnly::Unsupported {
            let refused = matches!(
                &result,
                Err(error @ RunError::ReadOnlyUnsupported { .. })
                    if error.to_string().contains(adapter.label())
            );
            return check(
                "refused before it starts",
                refused && !fake.ran(),
                format!("{:?}", result.map_err(|e| e.to_string())),
            );
        }
        if let Err(error) = result {
            return check("run", false, error.to_string());
        }
        let args = fake.args();
        check(
            "read-only arguments",
            with.iter().all(|arg| args.iter().any(|a| a == arg))
                && !without.iter().any(|arg| args.iter().any(|a| a == arg)),
            format!("{args:?}"),
        );
    });
}

#[test]
fn goose_is_kept_read_only_by_its_mode() {
    let fake = Fake::replaying("goose.jsonl");
    let cwd = TempDir::new().unwrap();
    let task = Task {
        autonomy: Autonomy::ReadOnly,
        ..task(cwd.path())
    };
    run_with("goose", &fake, task).0.unwrap();
    assert!(fake.env().lines().any(|line| line == "GOOSE_MODE=chat"));
}

#[test]
fn versions_without_structured_output_are_read_as_text() {
    for agent in ["gemini", "goose"] {
        let fake = Fake::with_help(
            "Usage: agent [options]",
            &format!("cat \"{}\"", fixture_path("text.txt").display()),
        );
        let cwd = TempDir::new().unwrap();
        let (outcome, _) = run_with(agent, &fake, task(cwd.path()));
        assert_eq!(outcome.unwrap().result.as_deref(), Some(TEXT_RESULT));
        let args = fake.args();
        assert!(!args.iter().any(|arg| arg == "stream-json"), "{args:?}");
    }
}

#[test]
fn a_stray_line_does_not_turn_structured_output_into_text() {
    for (agent, fixture) in [("gemini", "gemini.jsonl"), ("goose", "goose.jsonl")] {
        let fake = Fake::new(&format!(
            "echo 'Loaded cached credentials.'\ncat \"{}\"",
            fixture_path(fixture).display()
        ));
        let cwd = TempDir::new().unwrap();
        let (outcome, events) = run_with(agent, &fake, task(cwd.path()));
        assert_eq!(outcome.unwrap().result.as_deref(), Some("DONE"), "{agent}");
        assert!(
            matches!(&events[0], AgentEvent::Output { line, .. } if line == "Loaded cached credentials."),
            "{agent}: {:?}",
            events[0]
        );
    }
}

#[test]
fn tool_calls_that_change_files_say_which_and_how() {
    let changes = |agent: &str, fixture: &str| {
        let fake = Fake::replaying(fixture);
        let cwd = TempDir::new().unwrap();
        let (_, events) = run_with(agent, &fake, task(cwd.path()));
        events
            .into_iter()
            .find_map(|event| match event {
                AgentEvent::ToolCall { changes, .. } => Some(changes),
                _ => None,
            })
            .unwrap_or_default()
    };
    for (agent, fixture) in [
        ("amp", "amp.jsonl"),
        ("agy", "antigravity.jsonl"),
        ("cursor", "cursor.jsonl"),
        ("droid", "droid.jsonl"),
        ("gemini", "gemini.jsonl"),
        ("goose", "goose.jsonl"),
        ("kimi", "kimi.jsonl"),
        ("qwen", "qwen-code.jsonl"),
    ] {
        let changes = changes(agent, fixture);
        assert_eq!(changes.len(), 1, "{agent}");
        assert_eq!(changes[0].path, "hello.txt", "{agent}");
        assert_eq!(changes[0].kind, FileChangeKind::Write, "{agent}");
        assert_eq!(changes[0].new.as_deref(), Some("hi"), "{agent}");
    }
    assert!(changes("claude", "claude-code.jsonl").is_empty());

    let (_, events) = run_script(
        "codex",
        r#"echo '{"type":"item.completed","item":{"id":"i","type":"file_change","status":"completed","changes":[{"path":"a.txt","kind":"add"},{"path":"b.txt","kind":"delete"}]}}'"#,
    );
    let AgentEvent::ToolCall { changes, .. } = &events[0] else {
        panic!("{events:?}");
    };
    let kinds: Vec<_> = changes.iter().map(|c| (c.path.as_str(), c.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("a.txt", FileChangeKind::Create),
            ("b.txt", FileChangeKind::Delete)
        ]
    );
}

// ------------------------------------------- the lists of agents ---

fn ids() -> BTreeSet<String> {
    agents().map(|agent| agent.id().to_string()).collect()
}

fn repo_file(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn ids_aliases_and_executables_name_one_agent_each() {
    let mut seen = std::collections::HashMap::new();
    for agent in agents() {
        let names = std::iter::once(agent.id())
            .chain(agent.aliases().iter().copied())
            .chain(agent.executables().iter().copied());
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

#[test]
fn every_agent_has_a_case() {
    let cases: BTreeSet<String> = CASES
        .iter()
        .map(|case| find(case.agent).unwrap().id().to_string())
        .collect();
    assert_eq!(cases, ids());
    assert_eq!(CASES.len(), ids().len(), "one case per agent");
}

#[test]
fn the_coding_agent_type_lists_every_agent() {
    let types = repo_file("crates/cli/npm/types/modules/workflow.d.ts");
    let union = types
        .split_once("export type CodingAgent =")
        .expect("the CodingAgent type")
        .1
        .split_once(';')
        .unwrap()
        .0;
    let listed: BTreeSet<String> = union
        .split('|')
        .map(|member| member.trim().trim_matches('"').to_string())
        .filter(|member| !member.is_empty())
        .collect();
    assert_eq!(listed, ids());
}

#[test]
fn the_docs_list_every_agent_and_how_it_is_kept_read_only() {
    let docs = repo_file("docs/community/workflows/reference.mdx");
    let table = docs
        .split_once("| Agent | Id | Read-only | Notes |")
        .expect("the agents table")
        .1;
    let rows: Vec<Vec<&str>> = table
        .lines()
        .skip(2)
        .take_while(|line| line.starts_with('|'))
        .map(|line| line.split('|').map(str::trim).collect())
        .collect();
    let listed: BTreeSet<String> = rows
        .iter()
        .map(|row| row[2].split('`').nth(1).unwrap().to_string())
        .collect();
    assert_eq!(listed, ids());
    for row in rows {
        let agent = find(row[2].split('`').nth(1).unwrap()).unwrap();
        let expected = match agent.read_only() {
            ReadOnly::Enforced => "Yes",
            ReadOnly::AgentDefaults => "Its defaults",
            ReadOnly::Unsupported => "No",
        };
        assert!(
            row[3] == expected || row[3].starts_with(&format!("{expected}:")),
            "{}: the docs say {:?}, the adapter {expected:?}",
            agent.id(),
            row[3]
        );
    }
}
