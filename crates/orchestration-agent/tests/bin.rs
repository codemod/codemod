//! The binary over its streams: exit codes, the key channel, and that stdout
//! carries exactly one completion.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use orchestration_agent::{CompletionStatus, OperationCompletion, PROTOCOL_VERSION};
use serde_json::json;

const BIN: &str = env!("CARGO_BIN_EXE_codemod-agent");

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../packages/orchestration/fixtures/protocol/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture")
}

/// One compact line, as the TypeScript launcher writes it.
fn line(request: &str) -> String {
    serde_json::to_string(&serde_json::from_str::<serde_json::Value>(request).unwrap()).unwrap()
}

fn run_agent(
    cwd: &Path,
    env: &[(&str, &str)],
    path: &str,
    stdin: &str,
) -> (Option<i32>, Option<OperationCompletion>) {
    let mut command = Command::new(BIN);
    command
        .current_dir(cwd)
        .env_clear()
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("exits");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8");
    let completion = (!stdout.is_empty())
        .then(|| serde_json::from_str(&stdout).expect("stdout is exactly one completion"));
    (output.status.code(), completion)
}

fn path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

#[test]
fn arguments_and_other_operation_kinds_are_refused() {
    let status = Command::new(BIN)
        .arg("request.json")
        .stdin(Stdio::null())
        .status()
        .expect("runs");
    assert_eq!(status.code(), Some(2));

    let dir = tempfile::tempdir().expect("tempdir");
    let request = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "commandId": "lint",
        "operation": { "kind": "shell", "command": "touch ran.marker" },
    });
    let (code, completion) = run_agent(dir.path(), &[], &path(), &request.to_string());
    assert_eq!(code, Some(3));
    assert!(!dir.path().join("ran.marker").exists());
    let completion = completion.expect("completion");
    assert_eq!(completion.command_id, "lint");
    assert_eq!(completion.status, CompletionStatus::Failed);
    assert!(completion
        .error
        .unwrap()
        .message
        .contains("unknown variant `shell`"));

    let (code, completion) = run_agent(dir.path(), &[], &path(), "");
    assert_eq!(code, Some(3));
    assert!(completion
        .unwrap()
        .error
        .unwrap()
        .message
        .contains("invalid request JSON"));
}

#[test]
fn agent_failures_carry_their_phase() {
    let request = line(&fixture("agent-request.json"));
    // (environment, secrets line, expected phase, repositoryMayBeModified)
    type Case<'a> = (&'a [(&'a str, &'a str)], Option<&'a str>, &'a str, bool);
    // Nothing listens on the discard port: the first model request fails.
    let unreachable = ("LLM_BASE_URL", "http://127.0.0.1:9/v1");
    let cases: [Case; 4] = [
        (&[], None, "config", false),
        (
            &[("LLM_API_KEY", "test-key"), unreachable],
            None,
            "execute",
            true,
        ),
        // The TypeScript host's channel: the key only arrives on stdin.
        (
            &[("CODEMOD_AGENT_SECRETS", "stdin"), unreachable],
            Some(r#"{"LLM_API_KEY":"test-key"}"#),
            "execute",
            true,
        ),
        (
            &[("CODEMOD_AGENT_SECRETS", "stdin"), unreachable],
            Some("{}"),
            "config",
            false,
        ),
    ];
    for (env, secrets, phase, may_be_modified) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let stdin = match secrets {
            Some(secrets) => format!("{request}\n{secrets}"),
            None => request.clone(),
        };
        let (code, completion) = run_agent(dir.path(), env, &path(), &stdin);
        assert_eq!(code, Some(0), "{phase}");
        let completion = completion.expect("completion");
        assert_eq!(completion.status, CompletionStatus::Failed, "{phase}");
        assert_eq!(
            completion.error.expect("error").details,
            Some(json!({ "phase": phase, "repositoryMayBeModified": may_be_modified })),
        );
    }
}

/// External backends: no `LLM_API_KEY` is needed, and a CLI missing from
/// PATH is a config failure before anything runs.
#[test]
fn external_backend_without_its_cli_is_a_config_failure() {
    for (name, message) in [
        (
            "agent-claude-code-request.json",
            "claude-code backend requires the `claude` executable on an absolute PATH entry",
        ),
        (
            "agent-codex-request.json",
            "codex backend requires the `codex` executable on an absolute PATH entry",
        ),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        // Codex checks for a repository before looking for its CLI.
        std::fs::create_dir(dir.path().join(".git")).expect("git");
        let empty = tempfile::tempdir().expect("empty PATH");
        let (code, completion) = run_agent(
            dir.path(),
            &[],
            &empty.path().to_string_lossy(),
            &line(&fixture(name)),
        );
        assert_eq!(code, Some(0), "{name}");
        let completion = completion.expect("completion");
        assert_eq!(completion.status, CompletionStatus::Failed, "{name}");
        let error = completion.error.expect("error");
        assert_eq!(error.message, message);
        assert_eq!(
            error.details,
            Some(json!({ "phase": "config", "repositoryMayBeModified": false }))
        );
    }
}

/// A fake `claude` CLI that writes to every inherited descriptor it can find
/// and reports which fds 0-2 it inherited: none of them is the runner's
/// request or response stream.
#[cfg(unix)]
#[test]
fn processes_the_agent_starts_never_see_the_request_or_response_streams() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = tempfile::tempdir().expect("bin");
    let claude = bin.path().join("claude");
    let report = dir.path().join("fds.txt");
    std::fs::write(
        &claude,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = auth ]; then echo '{{\"loggedIn\":true}}'; exit 0; fi\n\
             for fd in 3 4 5 6 7 8 9; do (printf 'planted' >&$fd) 2>/dev/null && echo \"open $fd\" >> {report}; done\n\
             ls -l /proc/$$/fd >> {report} 2>&1\n\
             printf '%s' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"done\"}}'\n",
            report = report.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.path().display(), path());
    let (code, completion) = run_agent(
        dir.path(),
        &[("HOME", &dir.path().to_string_lossy())],
        &path,
        &line(&fixture("agent-claude-code-request.json")),
    );
    assert_eq!(code, Some(0));
    // The completion parsed as exactly one JSON document: nothing was planted.
    let completion = completion.expect("completion");
    let report = std::fs::read_to_string(&report).unwrap_or_default();
    assert_eq!(
        completion.status,
        CompletionStatus::Succeeded,
        "{report}\n{completion:?}"
    );
    assert!(report.contains("1 -> pipe:"), "the fake CLI ran: {report}");
    assert!(!report.contains("open "), "{report}\n{completion:?}");
}
