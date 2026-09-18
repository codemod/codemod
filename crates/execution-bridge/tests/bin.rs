//! The binary over real files: exit codes and one request of each kind.

use std::path::Path;
use std::process::Command;

use butterflow_execution_bridge::{CompletionStatus, OperationCompletion, PROTOCOL_VERSION};
use serde_json::json;
use sha2::{Digest, Sha256};

const BIN: &str = env!("CARGO_BIN_EXE_butterflow-execution-bridge");

fn run_bridge(dir: &Path, request: &str) -> (i32, Option<OperationCompletion>) {
    let request_path = dir.join("request.json");
    let response_path = dir.join("response.json");
    std::fs::write(&request_path, request).expect("write request");
    let status = Command::new(BIN)
        .arg(&request_path)
        .arg(&response_path)
        .current_dir(dir)
        .status()
        .expect("bridge binary runs");
    let completion = std::fs::read_to_string(&response_path)
        .ok()
        .map(|text| serde_json::from_str(&text).expect("response is a completion"));
    (status.code().unwrap_or(-1), completion)
}

#[cfg(unix)]
#[test]
fn shell_writes_a_succeeded_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "commandId": "hello",
        "operation": { "kind": "shell", "command": "printf ok" },
    });
    let (code, completion) = run_bridge(dir.path(), &request.to_string());
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.command_id, "hello");
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(completion.output.unwrap()["stdout"], "ok\n");
}

#[test]
fn jssg_transforms_the_supplied_files_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    let source = r#"export default async function transform(root) {
  return { content: root.root().text().replaceAll("old", "new"), output: { file: root.relativeFilename() } };
}"#;
    let hash = format!("{:x}", Sha256::digest(source.as_bytes()));
    std::fs::write(repo.join("a.ts"), "old();\n").expect("a");
    let request = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "commandId": "migrate",
        "operation": {
            "kind": "jssg",
            "transform": { "name": "migrate", "hash": hash },
            "language": "typescript",
        },
        "context": {
            "targetRoot": repo,
            "files": [{ "path": "a.ts", "content": "old();\n" }],
            "artifact": { "source": source },
        },
    });
    let (code, completion) = run_bridge(dir.path(), &request.to_string());
    let completion = completion.expect("response written");
    assert_eq!(code, 0);
    assert_eq!(completion.status, CompletionStatus::Succeeded);
    assert_eq!(
        completion.output.unwrap()["files"],
        json!([{
            "path": "a.ts",
            "edits": [{ "path": "a.ts", "content": "new();\n" }],
            "output": { "file": "a.ts" },
        }])
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("a.ts")).unwrap(),
        "old();\n"
    );
}

#[test]
fn malformed_requests_and_wrong_arguments_exit_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let unsupported = PROTOCOL_VERSION + 1;
    let request = format!(
        r#"{{"protocolVersion":{unsupported},"commandId":"bad","operation":{{"kind":"shell","command":"true"}}}}"#
    );
    let (code, completion) = run_bridge(dir.path(), &request);
    let completion = completion.expect("error response written");
    assert_eq!(code, 3);
    assert_eq!(completion.command_id, "bad");
    assert_eq!(completion.status, CompletionStatus::Failed);
    assert!(completion
        .error
        .unwrap()
        .message
        .contains(&format!("unsupported protocolVersion {unsupported}")));

    assert_eq!(Command::new(BIN).status().expect("runs").code(), Some(2));
    assert_eq!(
        Command::new(BIN)
            .arg("--nope")
            .status()
            .expect("runs")
            .code(),
        Some(2)
    );
}

/// The agent path of the binary: settings are read before the runtime starts,
/// a missing key fails before any request, and an agent that could not finish
/// reports that the repository may have been modified.
#[test]
fn agent_failures_carry_their_phase() {
    let request = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/orchestration/fixtures/protocol/agent-request.json"
    ))
    .expect("agent fixture");
    // (environment, stdin, expected phase, repositoryMayBeModified)
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
            &[("CODEMOD_BRIDGE_SECRETS", "stdin"), unreachable],
            Some(r#"{"LLM_API_KEY":"test-key"}"#),
            "execute",
            true,
        ),
        (
            &[("CODEMOD_BRIDGE_SECRETS", "stdin"), unreachable],
            Some("{}"),
            "config",
            false,
        ),
    ];
    for (env, stdin, phase, may_be_modified) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let request_path = dir.path().join("request.json");
        let response_path = dir.path().join("response.json");
        std::fs::write(&request_path, &request).expect("write request");
        let mut command = Command::new(BIN);
        command
            .arg(&request_path)
            .arg(&response_path)
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(if stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            });
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("runs");
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            std::io::Write::write_all(&mut pipe, text.as_bytes()).expect("write stdin");
        }
        assert_eq!(child.wait().expect("exits").code(), Some(0), "{phase}");
        let completion: OperationCompletion =
            serde_json::from_str(&std::fs::read_to_string(&response_path).expect("response"))
                .expect("completion");
        assert_eq!(completion.status, CompletionStatus::Failed, "{phase}");
        assert_eq!(
            completion.error.expect("error").details,
            Some(json!({ "phase": phase, "repositoryMayBeModified": may_be_modified })),
        );
    }
}

/// External backends through the binary: no `LLM_API_KEY` is needed, and a
/// CLI missing from PATH is a config failure before anything runs.
#[test]
fn external_backend_without_its_cli_is_a_config_failure() {
    for (fixture, message) in [
        (
            "agent-claude-code-request.json",
            "claude-code backend requires the `claude` executable on an absolute PATH entry",
        ),
        (
            "agent-codex-request.json",
            "codex backend requires the `codex` executable on an absolute PATH entry",
        ),
    ] {
        let request = std::fs::read_to_string(format!(
            "{}/../../packages/orchestration/fixtures/protocol/{fixture}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture");
        let dir = tempfile::tempdir().expect("tempdir");
        // Codex checks for a repository before looking for its CLI.
        std::fs::create_dir(dir.path().join(".git")).expect("git");
        let empty = tempfile::tempdir().expect("empty PATH");
        let request_path = dir.path().join("request.json");
        let response_path = dir.path().join("response.json");
        std::fs::write(&request_path, &request).expect("write request");
        let status = Command::new(BIN)
            .arg(&request_path)
            .arg(&response_path)
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", empty.path())
            .stdin(std::process::Stdio::null())
            .status()
            .expect("runs");
        assert_eq!(status.code(), Some(0), "{fixture}");
        let completion: OperationCompletion =
            serde_json::from_str(&std::fs::read_to_string(&response_path).expect("response"))
                .expect("completion");
        assert_eq!(completion.status, CompletionStatus::Failed, "{fixture}");
        let error = completion.error.expect("error");
        assert_eq!(error.message, message);
        assert_eq!(
            error.details,
            Some(json!({ "phase": "config", "repositoryMayBeModified": false }))
        );
    }
}

/// The response is created, never opened: a symlink or file planted at the
/// response path (for example by a sandboxed agent that can reach the
/// exchange) is neither written through nor overwritten, and a symlinked
/// request is refused.
#[cfg(unix)]
#[test]
fn planted_response_paths_are_never_written_through() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "commandId": "hello",
        "operation": { "kind": "shell", "command": "printf ok" },
    })
    .to_string();
    let request_path = dir.path().join("request.json");
    std::fs::write(&request_path, &request).expect("request");
    let outside = dir.path().join("outside.txt");
    std::fs::write(&outside, "untouched").expect("outside");

    let link = dir.path().join("link-response.json");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let status = Command::new(BIN)
        .arg(&request_path)
        .arg(&link)
        .current_dir(dir.path())
        .status()
        .expect("runs");
    assert_eq!(status.code(), Some(4));
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());

    let existing = dir.path().join("existing-response.json");
    std::fs::write(&existing, "planted").expect("existing");
    let status = Command::new(BIN)
        .arg(&request_path)
        .arg(&existing)
        .current_dir(dir.path())
        .status()
        .expect("runs");
    assert_eq!(status.code(), Some(4));
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "planted");

    // A fresh path is created private.
    let fresh = dir.path().join("fresh-response.json");
    let status = Command::new(BIN)
        .arg(&request_path)
        .arg(&fresh)
        .current_dir(dir.path())
        .status()
        .expect("runs");
    assert_eq!(status.code(), Some(0));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let request_link = dir.path().join("request-link.json");
    std::os::unix::fs::symlink(&request_path, &request_link).expect("symlink");
    let response = dir.path().join("linked-request-response.json");
    let status = Command::new(BIN)
        .arg(&request_link)
        .arg(&response)
        .current_dir(dir.path())
        .status()
        .expect("runs");
    assert_eq!(status.code(), Some(3));
    let completion: OperationCompletion =
        serde_json::from_str(&std::fs::read_to_string(&response).unwrap()).unwrap();
    assert!(completion
        .error
        .unwrap()
        .message
        .contains("request is not a regular file"));
}
